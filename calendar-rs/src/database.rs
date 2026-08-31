use crate::{CalendarEntry, CalendarError, Result, TaskListEntry};
use chrono::{DateTime, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct AccountRow {
    pub account_id: String,
    pub google_sub: String,
    pub email: String,
    pub label: String,
    pub enabled: bool,
    pub last_sync: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CalendarRow {
    pub calendar_id: String,
    pub sync_token: Option<String>,
    pub full_sync_at: Option<String>,
}

#[derive(Debug)]
pub struct Database {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Database {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path.parent().ok_or_else(|| {
            CalendarError::new("database_error", "Calendar database path has no parent")
        })?;
        fs::create_dir_all(parent).map_err(database_io)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(database_io)?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(database_io)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(database_io)?;
        let database = Self {
            path,
            lock: Mutex::new(()),
        };
        database.initialize()?;
        Ok(database)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path).map_err(database_sql)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 15000;")
            .map_err(database_sql)?;
        Ok(connection)
    }

    fn initialize(&self) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        connection
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS accounts (
                    account_id TEXT PRIMARY KEY,
                    google_sub TEXT NOT NULL UNIQUE,
                    email TEXT NOT NULL,
                    label TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    last_sync TEXT,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS calendars (
                    account_id TEXT NOT NULL,
                    calendar_id TEXT NOT NULL,
                    label TEXT NOT NULL,
                    color TEXT NOT NULL,
                    selected INTEGER NOT NULL DEFAULT 1,
                    is_primary INTEGER NOT NULL DEFAULT 0,
                    default_reminders TEXT NOT NULL DEFAULT '[]',
                    sync_token TEXT,
                    full_sync_at TEXT,
                    PRIMARY KEY (account_id, calendar_id),
                    FOREIGN KEY (account_id) REFERENCES accounts(account_id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS events (
                    item_id TEXT PRIMARY KEY,
                    account_id TEXT NOT NULL,
                    calendar_id TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    ical_uid TEXT,
                    occurrence_start TEXT,
                    title TEXT NOT NULL,
                    start_value TEXT NOT NULL,
                    end_value TEXT NOT NULL,
                    all_day INTEGER NOT NULL,
                    location TEXT NOT NULL DEFAULT '',
                    url TEXT NOT NULL DEFAULT '',
                    reminders TEXT NOT NULL DEFAULT '[]',
                    updated TEXT,
                    UNIQUE (account_id, calendar_id, event_id),
                    FOREIGN KEY (account_id, calendar_id)
                        REFERENCES calendars(account_id, calendar_id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS events_account_start
                    ON events(account_id, start_value);
                CREATE TABLE IF NOT EXISTS task_lists (
                    account_id TEXT NOT NULL,
                    task_list_id TEXT NOT NULL,
                    label TEXT NOT NULL,
                    updated TEXT,
                    PRIMARY KEY (account_id, task_list_id),
                    FOREIGN KEY (account_id) REFERENCES accounts(account_id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS tasks (
                    item_id TEXT PRIMARY KEY,
                    account_id TEXT NOT NULL,
                    task_list_id TEXT NOT NULL,
                    task_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    due_date TEXT,
                    status TEXT NOT NULL,
                    url TEXT NOT NULL DEFAULT '',
                    notes TEXT NOT NULL DEFAULT '',
                    updated TEXT,
                    UNIQUE (account_id, task_list_id, task_id),
                    FOREIGN KEY (account_id, task_list_id)
                        REFERENCES task_lists(account_id, task_list_id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS tasks_account_due
                    ON tasks(account_id, due_date);
                "#,
            )
            .map_err(database_sql)?;
        let has_full_sync = {
            let mut statement = connection
                .prepare("PRAGMA table_info(calendars)")
                .map_err(database_sql)?;
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(database_sql)?;
            let mut found = false;
            for column in columns {
                if column.map_err(database_sql)? == "full_sync_at" {
                    found = true;
                }
            }
            found
        };
        if !has_full_sync {
            connection
                .execute("ALTER TABLE calendars ADD COLUMN full_sync_at TEXT", [])
                .map_err(database_sql)?;
        }
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).map_err(database_io)?;
        Ok(())
    }

    pub fn configured_accounts(&self) -> Result<Vec<AccountRow>> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "SELECT account_id, google_sub, email, label, enabled, last_sync, error \
                 FROM accounts ORDER BY label COLLATE NOCASE",
            )
            .map_err(database_sql)?;
        let rows = statement
            .query_map([], account_from_row)
            .map_err(database_sql)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_sql)
    }

    pub fn account(&self, account_id: &str) -> Result<Option<AccountRow>> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        connection
            .query_row(
                "SELECT account_id, google_sub, email, label, enabled, last_sync, error \
                 FROM accounts WHERE account_id = ?1",
                [account_id],
                account_from_row,
            )
            .optional()
            .map_err(database_sql)
    }

    pub fn account_by_sub(&self, google_sub: &str) -> Result<Option<AccountRow>> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        connection
            .query_row(
                "SELECT account_id, google_sub, email, label, enabled, last_sync, error \
                 FROM accounts WHERE google_sub = ?1",
                [google_sub],
                account_from_row,
            )
            .optional()
            .map_err(database_sql)
    }

    pub fn upsert_account(
        &self,
        account_id: &str,
        google_sub: &str,
        email: &str,
        label: &str,
    ) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let now = iso_now();
        connection
            .execute(
                r#"
                INSERT INTO accounts (
                    account_id, google_sub, email, label, enabled,
                    last_sync, error, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, 1, NULL, NULL, ?5, ?5)
                ON CONFLICT(account_id) DO UPDATE SET
                    google_sub = excluded.google_sub,
                    email = excluded.email,
                    label = excluded.label,
                    enabled = 1,
                    error = NULL,
                    updated_at = excluded.updated_at
                "#,
                params![account_id, google_sub, email, label, now],
            )
            .map_err(database_sql)?;
        Ok(())
    }

    pub fn delete_account(&self, account_id: &str) -> Result<bool> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let changed = connection
            .execute("DELETE FROM accounts WHERE account_id = ?1", [account_id])
            .map_err(database_sql)?;
        Ok(changed > 0)
    }

    pub fn set_sync_result(&self, account_id: &str, error: Option<&str>) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let now = iso_now();
        connection
            .execute(
                r#"
                UPDATE accounts
                SET last_sync = CASE WHEN ?1 IS NULL THEN ?2 ELSE last_sync END,
                    error = ?1, updated_at = ?2
                WHERE account_id = ?3
                "#,
                params![error, now, account_id],
            )
            .map_err(database_sql)?;
        Ok(())
    }

    pub fn replace_calendars(&self, account_id: &str, calendars: &[CalendarEntry]) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let mut connection = self.connect()?;
        let transaction = connection.transaction().map_err(database_sql)?;
        let incoming: HashSet<&str> = calendars
            .iter()
            .map(|calendar| calendar.id.as_str())
            .collect();
        for calendar in calendars {
            transaction
                .execute(
                    r#"
                    INSERT INTO calendars (
                        account_id, calendar_id, label, color, selected,
                        is_primary, default_reminders, sync_token, full_sync_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)
                    ON CONFLICT(account_id, calendar_id) DO UPDATE SET
                        label = excluded.label,
                        color = excluded.color,
                        selected = excluded.selected,
                        is_primary = excluded.is_primary,
                        default_reminders = excluded.default_reminders
                    "#,
                    params![
                        account_id,
                        calendar.id,
                        calendar.label,
                        calendar.color,
                        calendar.selected as i64,
                        calendar.primary as i64,
                        compact_json(&calendar.default_reminders),
                    ],
                )
                .map_err(database_sql)?;
        }
        let existing = {
            let mut statement = transaction
                .prepare("SELECT calendar_id FROM calendars WHERE account_id = ?1")
                .map_err(database_sql)?;
            let rows = statement
                .query_map([account_id], |row| row.get::<_, String>(0))
                .map_err(database_sql)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(database_sql)?
        };
        for calendar_id in existing {
            if !incoming.contains(calendar_id.as_str()) {
                transaction
                    .execute(
                        "DELETE FROM calendars WHERE account_id = ?1 AND calendar_id = ?2",
                        params![account_id, calendar_id],
                    )
                    .map_err(database_sql)?;
            }
        }
        transaction.commit().map_err(database_sql)
    }

    pub fn calendars(&self, account_id: &str) -> Result<Vec<CalendarRow>> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "SELECT calendar_id, sync_token, full_sync_at FROM calendars \
                 WHERE account_id = ?1 ORDER BY is_primary DESC, label COLLATE NOCASE",
            )
            .map_err(database_sql)?;
        let rows = statement
            .query_map([account_id], |row| {
                Ok(CalendarRow {
                    calendar_id: row.get(0)?,
                    sync_token: row.get(1)?,
                    full_sync_at: row.get(2)?,
                })
            })
            .map_err(database_sql)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_sql)
    }

    pub fn apply_events(
        &self,
        account_id: &str,
        calendar_id: &str,
        events: &[Value],
        sync_token: &str,
        replace: bool,
    ) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let mut connection = self.connect()?;
        let transaction = connection.transaction().map_err(database_sql)?;
        if replace {
            transaction
                .execute(
                    "DELETE FROM events WHERE account_id = ?1 AND calendar_id = ?2",
                    params![account_id, calendar_id],
                )
                .map_err(database_sql)?;
        }
        for event in events {
            let event_id = string_field(event, "id", "");
            if event_id.is_empty() {
                continue;
            }
            if event.get("status").and_then(Value::as_str) == Some("cancelled") {
                transaction
                    .execute(
                        "DELETE FROM events WHERE account_id = ?1 AND calendar_id = ?2 AND event_id = ?3",
                        params![account_id, calendar_id, event_id],
                    )
                    .map_err(database_sql)?;
                continue;
            }
            let Some(event) = normalize_event(account_id, calendar_id, event) else {
                continue;
            };
            transaction
                .execute(
                    r#"
                    INSERT INTO events (
                        item_id, account_id, calendar_id, event_id, ical_uid,
                        occurrence_start, title, start_value, end_value,
                        all_day, location, url, reminders, updated
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                    ON CONFLICT(account_id, calendar_id, event_id) DO UPDATE SET
                        item_id = excluded.item_id,
                        ical_uid = excluded.ical_uid,
                        occurrence_start = excluded.occurrence_start,
                        title = excluded.title,
                        start_value = excluded.start_value,
                        end_value = excluded.end_value,
                        all_day = excluded.all_day,
                        location = excluded.location,
                        url = excluded.url,
                        reminders = excluded.reminders,
                        updated = excluded.updated
                    "#,
                    params![
                        event.item_id,
                        account_id,
                        calendar_id,
                        event.event_id,
                        event.ical_uid,
                        event.occurrence_start,
                        event.title,
                        event.start,
                        event.end,
                        event.all_day as i64,
                        event.location,
                        event.url,
                        compact_json(&event.reminders),
                        event.updated,
                    ],
                )
                .map_err(database_sql)?;
        }
        transaction
            .execute(
                r#"
                UPDATE calendars
                SET sync_token = ?1,
                    full_sync_at = CASE WHEN ?2 THEN ?3 ELSE full_sync_at END
                WHERE account_id = ?4 AND calendar_id = ?5
                "#,
                params![
                    sync_token,
                    replace as i64,
                    iso_now(),
                    account_id,
                    calendar_id
                ],
            )
            .map_err(database_sql)?;
        transaction.commit().map_err(database_sql)
    }

    pub fn clear_calendar_sync(&self, account_id: &str, calendar_id: &str) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let mut connection = self.connect()?;
        let transaction = connection.transaction().map_err(database_sql)?;
        transaction
            .execute(
                "DELETE FROM events WHERE account_id = ?1 AND calendar_id = ?2",
                params![account_id, calendar_id],
            )
            .map_err(database_sql)?;
        transaction
            .execute(
                "UPDATE calendars SET sync_token = NULL WHERE account_id = ?1 AND calendar_id = ?2",
                params![account_id, calendar_id],
            )
            .map_err(database_sql)?;
        transaction.commit().map_err(database_sql)
    }

    pub fn replace_tasks(
        &self,
        account_id: &str,
        task_lists: &[TaskListEntry],
        tasks_by_list: &std::collections::HashMap<String, Vec<Value>>,
    ) -> Result<()> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let mut connection = self.connect()?;
        let transaction = connection.transaction().map_err(database_sql)?;
        transaction
            .execute("DELETE FROM task_lists WHERE account_id = ?1", [account_id])
            .map_err(database_sql)?;
        for task_list in task_lists {
            transaction
                .execute(
                    "INSERT INTO task_lists (account_id, task_list_id, label, updated) VALUES (?1, ?2, ?3, ?4)",
                    params![account_id, task_list.id, task_list.label, task_list.updated],
                )
                .map_err(database_sql)?;
            if let Some(tasks) = tasks_by_list.get(&task_list.id) {
                for task in tasks {
                    let Some(task) = normalize_task(account_id, &task_list.id, task) else {
                        continue;
                    };
                    transaction
                        .execute(
                            r#"
                            INSERT INTO tasks (
                                item_id, account_id, task_list_id, task_id,
                                title, due_date, status, url, notes, updated
                            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                            "#,
                            params![
                                task.item_id,
                                account_id,
                                task_list.id,
                                task.task_id,
                                task.title,
                                task.due_date,
                                task.status,
                                task.url,
                                task.notes,
                                task.updated,
                            ],
                        )
                        .map_err(database_sql)?;
                }
            }
        }
        transaction.commit().map_err(database_sql)
    }

    pub fn state(&self, syncing: &HashSet<String>, configured: bool) -> Result<Value> {
        let accounts = self.configured_accounts()?;
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let mut output = Vec::new();
        for account in &accounts {
            let mut statement = connection
                .prepare(
                    "SELECT calendar_id, label, color, selected, is_primary FROM calendars \
                     WHERE account_id = ?1 ORDER BY is_primary DESC, label COLLATE NOCASE",
                )
                .map_err(database_sql)?;
            let calendars = statement
                .query_map([&account.account_id], |row| {
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "label": row.get::<_, String>(1)?,
                        "color": row.get::<_, String>(2)?,
                        "selected": row.get::<_, i64>(3)? != 0,
                        "primary": row.get::<_, i64>(4)? != 0,
                    }))
                })
                .map_err(database_sql)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(database_sql)?;
            let status = if syncing.contains(&account.account_id) {
                "syncing"
            } else if account.error.is_some() {
                "error"
            } else {
                "idle"
            };
            output.push(json!({
                "id": account.account_id,
                "email": account.email,
                "label": account.label,
                "enabled": account.enabled,
                "status": status,
                "error": account.error,
                "lastSync": account.last_sync,
                "calendars": calendars,
            }));
        }
        let overall = if !configured {
            "not_configured"
        } else if output.is_empty() {
            "empty"
        } else if output.iter().any(|item| item["status"] == "syncing") {
            "syncing"
        } else if output.iter().any(|item| item["status"] == "error") {
            "error"
        } else {
            "idle"
        };
        let error = output
            .iter()
            .find_map(|item| item.get("error").filter(|v| !v.is_null()).cloned());
        let last_sync = output
            .iter()
            .filter_map(|item| item.get("lastSync").and_then(Value::as_str))
            .max()
            .map(ToOwned::to_owned);
        Ok(json!({
            "status": overall,
            "error": error,
            "lastSync": last_sync,
            "accounts": output,
        }))
    }

    pub fn agenda(&self, start: &str, end: &str, account_filter: &str) -> Result<Value> {
        let bounds = range_bounds(start, end)?;
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        let mut event_statement = connection
            .prepare(
                r#"
                SELECT e.item_id, e.account_id, e.calendar_id, e.ical_uid,
                       e.occurrence_start, e.title, e.start_value, e.end_value,
                       e.all_day, e.location, e.url, e.reminders,
                       a.label, c.label, c.color, c.default_reminders
                FROM events e
                JOIN accounts a ON a.account_id = e.account_id
                JOIN calendars c ON c.account_id = e.account_id AND c.calendar_id = e.calendar_id
                WHERE a.enabled = 1 AND c.selected = 1
                  AND (?1 = 'all' OR e.account_id = ?1)
                "#,
            )
            .map_err(database_sql)?;
        let event_rows = event_statement
            .query_map([account_filter], |row| {
                Ok(EventAgendaRow {
                    item_id: row.get(0)?,
                    account_id: row.get(1)?,
                    calendar_id: row.get(2)?,
                    ical_uid: row.get(3)?,
                    occurrence_start: row.get(4)?,
                    title: row.get(5)?,
                    start: row.get(6)?,
                    end: row.get(7)?,
                    all_day: row.get::<_, i64>(8)? != 0,
                    location: row.get(9)?,
                    url: row.get(10)?,
                    reminders: row.get(11)?,
                    account_label: row.get(12)?,
                    calendar_label: row.get(13)?,
                    color: row.get(14)?,
                    default_reminders: row.get(15)?,
                })
            })
            .map_err(database_sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_sql)?;
        let mut task_statement = connection
            .prepare(
                r#"
                SELECT t.item_id, t.account_id, t.task_list_id, t.title,
                       t.due_date, t.url, a.label, l.label
                FROM tasks t
                JOIN accounts a ON a.account_id = t.account_id
                JOIN task_lists l ON l.account_id = t.account_id AND l.task_list_id = t.task_list_id
                WHERE a.enabled = 1 AND t.status != 'completed'
                  AND (?1 = 'all' OR t.account_id = ?1)
                "#,
            )
            .map_err(database_sql)?;
        let task_rows = task_statement
            .query_map([account_filter], |row| {
                Ok(TaskAgendaRow {
                    item_id: row.get(0)?,
                    account_id: row.get(1)?,
                    task_list_id: row.get(2)?,
                    title: row.get(3)?,
                    due_date: row.get(4)?,
                    url: row.get(5)?,
                    account_label: row.get(6)?,
                    list_label: row.get(7)?,
                })
            })
            .map_err(database_sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_sql)?;
        drop(task_statement);
        drop(event_statement);
        drop(connection);

        let mut items = Vec::new();
        for event in event_rows {
            let overlaps = if event.all_day {
                event.start.as_str() < bounds.end_date.as_str()
                    && event.end.as_str() > bounds.start_date.as_str()
            } else {
                let Ok(event_start) = DateTime::parse_from_rfc3339(&event.start) else {
                    continue;
                };
                let Ok(event_end) = DateTime::parse_from_rfc3339(&event.end) else {
                    continue;
                };
                event_start < bounds.end && event_end > bounds.start
            };
            if !overlaps {
                continue;
            }
            let mut reminders: Value =
                serde_json::from_str(&event.reminders).unwrap_or_else(|_| json!([]));
            if reminders == json!([{"useDefault": true}]) {
                reminders =
                    serde_json::from_str(&event.default_reminders).unwrap_or_else(|_| json!([]));
            }
            items.push(json!({
                "id": event.item_id,
                "type": "event",
                "accountId": event.account_id,
                "accountLabel": event.account_label,
                "calendarId": event.calendar_id,
                "calendarLabel": event.calendar_label,
                "color": event.color,
                "title": event.title,
                "start": event.start,
                "end": event.end,
                "allDay": event.all_day,
                "dueDate": Value::Null,
                "location": event.location,
                "url": event.url,
                "reminders": reminders,
                "icalUid": event.ical_uid,
                "occurrenceStart": event.occurrence_start,
            }));
        }
        for task in task_rows {
            let Some(due_date) = task.due_date else {
                continue;
            };
            if due_date < bounds.start_date || due_date >= bounds.end_date {
                continue;
            }
            items.push(json!({
                "id": task.item_id,
                "type": "task",
                "accountId": task.account_id,
                "accountLabel": task.account_label,
                "calendarId": task.task_list_id,
                "calendarLabel": task.list_label,
                "color": "#8ab4f8",
                "title": task.title,
                "start": due_date,
                "end": due_date,
                "allDay": true,
                "dueDate": due_date,
                "location": "",
                "url": task.url,
                "reminders": [],
                "icalUid": Value::Null,
                "occurrenceStart": Value::Null,
            }));
        }
        items.sort_by(|left, right| {
            let key = |item: &Value| {
                (
                    item["start"].as_str().unwrap_or("").to_owned(),
                    if item["allDay"].as_bool().unwrap_or(false) {
                        0
                    } else {
                        1
                    },
                    item["title"].as_str().unwrap_or("").to_lowercase(),
                    item["id"].as_str().unwrap_or("").to_owned(),
                )
            };
            key(left).cmp(&key(right))
        });
        Ok(json!({"start": start, "end": end, "items": items, "generatedAt": iso_now()}))
    }

    pub fn item_url(&self, item_id: &str) -> Result<Option<String>> {
        let _guard = self.lock.lock().expect("database mutex poisoned");
        let connection = self.connect()?;
        if let Some(url) = connection
            .query_row(
                "SELECT url FROM events WHERE item_id = ?1",
                [item_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_sql)?
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(url));
        }
        connection
            .query_row(
                "SELECT url FROM tasks WHERE item_id = ?1",
                [item_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|value| value.filter(|url| !url.is_empty()))
            .map_err(database_sql)
    }
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        account_id: row.get(0)?,
        google_sub: row.get(1)?,
        email: row.get(2)?,
        label: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        last_sync: row.get(5)?,
        error: row.get(6)?,
    })
}

#[derive(Debug)]
struct NormalizedEvent {
    item_id: String,
    event_id: String,
    ical_uid: Option<String>,
    occurrence_start: Option<String>,
    title: String,
    start: String,
    end: String,
    all_day: bool,
    location: String,
    url: String,
    reminders: Value,
    updated: Option<String>,
}

fn normalize_event(account_id: &str, calendar_id: &str, event: &Value) -> Option<NormalizedEvent> {
    let start_object = event.get("start")?.as_object()?;
    let end_object = event.get("end")?.as_object()?;
    let all_day = start_object.contains_key("date");
    let start = start_object
        .get(if all_day { "date" } else { "dateTime" })?
        .as_str()?
        .to_owned();
    let end = end_object
        .get(if all_day { "date" } else { "dateTime" })?
        .as_str()?
        .to_owned();
    let event_id = string_field(event, "id", "");
    if event_id.is_empty() {
        return None;
    }
    let recurrence_start = event
        .get("originalStartTime")
        .and_then(Value::as_object)
        .and_then(|value| value.get("dateTime").or_else(|| value.get("date")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let occurrence_start = Some(recurrence_start.unwrap_or_else(|| start.clone()));
    let reminders_object = event.get("reminders").and_then(Value::as_object);
    let reminders = if reminders_object
        .and_then(|object| object.get("useDefault"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        json!([{"useDefault": true}])
    } else {
        Value::Array(
            reminders_object
                .and_then(|object| object.get("overrides"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .map(|reminder| {
                    json!({
                        "method": reminder.get("method").and_then(Value::as_str).unwrap_or("popup"),
                        "minutes": reminder.get("minutes").and_then(Value::as_i64).unwrap_or(0),
                    })
                })
                .collect(),
        )
    };
    Some(NormalizedEvent {
        item_id: stable_item_id("event", &[account_id, calendar_id, &event_id]),
        event_id,
        ical_uid: optional_string_field(event, "iCalUID"),
        occurrence_start,
        title: string_field(event, "summary", "(Untitled event)"),
        start,
        end,
        all_day,
        location: string_field(event, "location", ""),
        url: string_field(event, "htmlLink", ""),
        reminders,
        updated: optional_string_field(event, "updated"),
    })
}

#[derive(Debug)]
struct NormalizedTask {
    item_id: String,
    task_id: String,
    title: String,
    due_date: Option<String>,
    status: String,
    url: String,
    notes: String,
    updated: Option<String>,
}

fn normalize_task(account_id: &str, task_list_id: &str, task: &Value) -> Option<NormalizedTask> {
    let task_id = string_field(task, "id", "");
    if task_id.is_empty()
        || task
            .get("deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || task.get("hidden").and_then(Value::as_bool).unwrap_or(false)
    {
        return None;
    }
    let due_date = optional_string_field(task, "due").map(|due| due.chars().take(10).collect());
    Some(NormalizedTask {
        item_id: stable_item_id("task", &[account_id, task_list_id, &task_id]),
        task_id,
        title: string_field(task, "title", "(Untitled task)"),
        due_date,
        status: string_field(task, "status", "needsAction"),
        url: string_field(task, "webViewLink", ""),
        notes: string_field(task, "notes", ""),
        updated: optional_string_field(task, "updated"),
    })
}

#[derive(Debug)]
struct EventAgendaRow {
    item_id: String,
    account_id: String,
    calendar_id: String,
    ical_uid: Option<String>,
    occurrence_start: Option<String>,
    title: String,
    start: String,
    end: String,
    all_day: bool,
    location: String,
    url: String,
    reminders: String,
    account_label: String,
    calendar_label: String,
    color: String,
    default_reminders: String,
}

#[derive(Debug)]
struct TaskAgendaRow {
    item_id: String,
    account_id: String,
    task_list_id: String,
    title: String,
    due_date: Option<String>,
    url: String,
    account_label: String,
    list_label: String,
}

struct RangeBounds {
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    start_date: String,
    end_date: String,
}

fn range_bounds(start: &str, end: &str) -> Result<RangeBounds> {
    let start_time = parse_boundary(start)?;
    let end_time = parse_boundary(end)?;
    if end_time <= start_time {
        return Err(CalendarError::new(
            "invalid_params",
            "end must be later than start",
        ));
    }
    Ok(RangeBounds {
        start: start_time,
        end: end_time,
        start_date: start_time.with_timezone(&Local).date_naive().to_string(),
        end_date: end_time.with_timezone(&Local).date_naive().to_string(),
    })
}

fn parse_boundary(value: &str) -> Result<DateTime<FixedOffset>> {
    if value.len() == 10 {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
            CalendarError::new("invalid_params", "start or end is not valid ISO 8601")
        })?;
        let naive = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
        let local = match Local.from_local_datetime(&naive) {
            LocalResult::Single(value) => value,
            LocalResult::Ambiguous(first, _) => first,
            LocalResult::None => {
                return Err(CalendarError::new(
                    "invalid_params",
                    "start or end is not valid local time",
                ));
            }
        };
        return Ok(local.fixed_offset());
    }
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Ok(timestamp);
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f"))
        .map_err(|_| CalendarError::new("invalid_params", "start or end is not valid ISO 8601"))?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Ok(value.fixed_offset()),
        LocalResult::Ambiguous(first, _) => Ok(first.fixed_offset()),
        LocalResult::None => Err(CalendarError::new(
            "invalid_params",
            "start or end is not valid local time",
        )),
    }
}

fn stable_item_id(kind: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            digest.update([0]);
        }
        digest.update(part.as_bytes());
    }
    format!("{kind}:{:x}", digest.finalize())
}

fn string_field(value: &Value, field: &str, fallback: &str) -> String {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn optional_string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(|value| {
        if value.is_null() {
            None
        } else {
            Some(match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
        }
    })
}

pub(crate) fn iso_now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON value serializes")
}

fn database_io(error: std::io::Error) -> CalendarError {
    CalendarError::new(
        "database_error",
        format!("Calendar database error: {error}"),
    )
}

fn database_sql(error: rusqlite::Error) -> CalendarError {
    CalendarError::new(
        "database_error",
        format!("Calendar database error: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_match_python_contract() {
        assert_eq!(
            stable_item_id("event", &["account-1", "primary@example.com", "event-1"]),
            "event:6ef791aa6311f4618f1a5363e63413f18837b1a01b4336b526d335cb0ccee53a"
        );
    }
}
