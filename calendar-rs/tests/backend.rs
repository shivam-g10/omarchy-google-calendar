use omarchy_calendar::{
    AuthResult, CalendarEntry, CalendarError, CalendarServer, CalendarService, Database, GoogleApi,
    OAuthAttempt, OAuthPhase, Result, SecretStoreApi, TaskListEntry,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

fn fixture(name: &str) -> Value {
    let content = match name {
        "calendar_list.json" => include_str!("fixtures/calendar_list.json"),
        "events_initial.json" => include_str!("fixtures/events_initial.json"),
        "events_incremental.json" => include_str!("fixtures/events_incremental.json"),
        "task_lists.json" => include_str!("fixtures/task_lists.json"),
        "tasks.json" => include_str!("fixtures/tasks.json"),
        _ => panic!("unknown fixture"),
    };
    serde_json::from_str(content).unwrap()
}

#[derive(Default)]
struct FakeSecrets {
    values: Mutex<HashMap<String, String>>,
    fail_store: Mutex<bool>,
    lookup_calls: AtomicUsize,
}

impl SecretStoreApi for FakeSecrets {
    fn store(&self, account_id: &str, refresh_token: &str) -> Result<()> {
        if *self.fail_store.lock().unwrap() {
            return Err(CalendarError::new(
                "secret_service_error",
                "Could not store Google credentials",
            ));
        }
        self.values
            .lock()
            .unwrap()
            .insert(account_id.to_owned(), refresh_token.to_owned());
        Ok(())
    }

    fn lookup(&self, account_id: &str) -> Result<String> {
        self.lookup_calls.fetch_add(1, Ordering::SeqCst);
        self.values
            .lock()
            .unwrap()
            .get(account_id)
            .cloned()
            .ok_or_else(|| {
                CalendarError::new("credentials_missing", "Google account needs reconnection")
            })
    }

    fn clear(&self, account_id: &str) -> Result<()> {
        self.values.lock().unwrap().remove(account_id);
        Ok(())
    }
}

struct FakeGoogle {
    calendar_payload: Value,
    initial_payload: Value,
    incremental_payload: Value,
    task_lists_payload: Value,
    tasks_payload: Value,
    sync_mode: Mutex<String>,
    seen_tokens: Mutex<Vec<Option<String>>>,
    revoked: Mutex<Vec<String>>,
    authorize_calls: AtomicUsize,
    refresh_calls: AtomicUsize,
    refresh_gate: Mutex<Option<AuthorizationGate>>,
    authorization_gate: Mutex<Option<AuthorizationGate>>,
    authorization_error: Mutex<Option<CalendarError>>,
    authorization_subject: Mutex<String>,
    authorization_email: Mutex<String>,
}

struct AuthorizationGate {
    started: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}

impl FakeGoogle {
    fn new() -> Self {
        Self {
            calendar_payload: fixture("calendar_list.json"),
            initial_payload: fixture("events_initial.json"),
            incremental_payload: fixture("events_incremental.json"),
            task_lists_payload: fixture("task_lists.json"),
            tasks_payload: fixture("tasks.json"),
            sync_mode: Mutex::new("initial".into()),
            seen_tokens: Mutex::new(Vec::new()),
            revoked: Mutex::new(Vec::new()),
            authorize_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
            refresh_gate: Mutex::new(None),
            authorization_gate: Mutex::new(None),
            authorization_error: Mutex::new(None),
            authorization_subject: Mutex::new("google-subject-1".into()),
            authorization_email: Mutex::new("person@example.com".into()),
        }
    }

    fn with_blocked_first_authorization() -> (Self, mpsc::Receiver<()>, mpsc::SyncSender<()>) {
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let google = Self::new();
        *google.authorization_gate.lock().unwrap() = Some(AuthorizationGate {
            started: started_sender,
            release: release_receiver,
        });
        (google, started_receiver, release_sender)
    }

    fn check_access(access_token: &str) -> Result<()> {
        if access_token == "ephemeral-access-token" {
            Ok(())
        } else {
            Err(CalendarError::new("test_error", "unexpected access token"))
        }
    }

    fn fail_next_authorization(&self) {
        *self.authorization_error.lock().unwrap() = Some(CalendarError::new(
            "oauth_denied",
            "Google sign-in was cancelled",
        ));
    }
}

impl GoogleApi for FakeGoogle {
    fn configured(&self) -> bool {
        true
    }

    fn authorize(&self, attempt: &OAuthAttempt) -> Result<AuthResult> {
        self.authorize_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.authorization_gate.lock().unwrap().take() {
            gate.started.send(()).unwrap();
            loop {
                if attempt.phase() == OAuthPhase::Cancelled {
                    return Err(CalendarError::new(
                        "oauth_cancelled",
                        "Google sign-in was cancelled",
                    ));
                }
                match gate.release.recv_timeout(Duration::from_millis(10)) {
                    Ok(()) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
        if let Some(error) = self.authorization_error.lock().unwrap().take() {
            return Err(error);
        }
        if !attempt.begin_finishing() {
            return Err(CalendarError::new(
                "oauth_cancelled",
                "Google sign-in was cancelled",
            ));
        }
        Ok(AuthResult {
            refresh_token: "refresh-token-never-cache".into(),
            access_token: "access-token-never-cache".into(),
            subject: self.authorization_subject.lock().unwrap().clone(),
            email: self.authorization_email.lock().unwrap().clone(),
        })
    }

    fn refresh_access_token(&self, refresh_token: &str) -> Result<String> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.refresh_gate.lock().unwrap().take() {
            gate.started.send(()).unwrap();
            gate.release.recv().unwrap();
        }
        if refresh_token != "refresh-token-never-cache" {
            return Err(CalendarError::new("test_error", "unexpected refresh token"));
        }
        Ok("ephemeral-access-token".into())
    }

    fn revoke(&self, refresh_token: &str) -> bool {
        self.revoked.lock().unwrap().push(refresh_token.to_owned());
        true
    }

    fn list_calendars(&self, access_token: &str) -> Result<Vec<CalendarEntry>> {
        Self::check_access(access_token)?;
        Ok(self.calendar_payload["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| CalendarEntry {
                id: item["id"].as_str().unwrap().into(),
                label: item["summary"].as_str().unwrap().into(),
                color: item["backgroundColor"].as_str().unwrap().into(),
                selected: true,
                primary: item["primary"].as_bool().unwrap_or(false),
                default_reminders: item
                    .get("defaultReminders")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            })
            .collect())
    }

    fn list_events(
        &self,
        access_token: &str,
        _calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<(Vec<Value>, String)> {
        Self::check_access(access_token)?;
        self.seen_tokens
            .lock()
            .unwrap()
            .push(sync_token.map(ToOwned::to_owned));
        let mut sync_mode = self.sync_mode.lock().unwrap();
        if *sync_mode == "gone_once" && sync_token.is_some() {
            *sync_mode = "initial".into();
            return Err(CalendarError::google_http(
                410,
                "Sync token is no longer valid",
            ));
        }
        let payload = if sync_token.is_none() {
            &self.initial_payload
        } else {
            &self.incremental_payload
        };
        Ok((
            payload["items"].as_array().unwrap().clone(),
            payload["nextSyncToken"].as_str().unwrap().into(),
        ))
    }

    fn list_task_lists(&self, access_token: &str) -> Result<Vec<TaskListEntry>> {
        Self::check_access(access_token)?;
        Ok(self.task_lists_payload["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| TaskListEntry {
                id: item["id"].as_str().unwrap().into(),
                label: item["title"].as_str().unwrap().into(),
                updated: item["updated"].as_str().map(ToOwned::to_owned),
            })
            .collect())
    }

    fn list_tasks(&self, access_token: &str, _task_list_id: &str) -> Result<Vec<Value>> {
        Self::check_access(access_token)?;
        Ok(self.tasks_payload["items"].as_array().unwrap().clone())
    }
}

struct Harness {
    _temporary: tempfile::TempDir,
    database: Arc<Database>,
    google: Arc<FakeGoogle>,
    secrets: Arc<FakeSecrets>,
    service: Arc<CalendarService>,
}

impl Harness {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::new(temporary.path().join("state/agenda.db")).unwrap());
        let google = Arc::new(FakeGoogle::new());
        let secrets = Arc::new(FakeSecrets::default());
        let google_api: Arc<dyn GoogleApi> = google.clone();
        let secret_api: Arc<dyn SecretStoreApi> = secrets.clone();
        let service = Arc::new(CalendarService::new(
            database.clone(),
            google_api,
            secret_api,
        ));
        Self {
            _temporary: temporary,
            database,
            google,
            secrets,
            service,
        }
    }

    fn add_database_account(&self, account_id: &str) {
        self.database
            .upsert_account(
                account_id,
                "google-subject-1",
                "person@example.com",
                "Personal",
            )
            .unwrap();
        self.secrets
            .store(account_id, "refresh-token-never-cache")
            .unwrap();
    }
}

#[test]
fn cached_reads_include_passive_context_and_never_refresh() {
    let harness = Harness::new();
    let query = json!({"start": "2026-08-31", "end": "2026-09-01"});
    let empty = harness.service.agenda(&query).unwrap();
    assert_eq!(empty["context"]["accounts"], json!([]));
    harness.add_database_account("account-1");
    let initial = harness.service.agenda(&query).unwrap();
    assert!(initial["context"]["accounts"][0]["lastSync"].is_null());
    assert_eq!(harness.secrets.lookup_calls.load(Ordering::SeqCst), 0);
    assert_eq!(harness.google.refresh_calls.load(Ordering::SeqCst), 0);

    harness.service.refresh(&json!({})).unwrap();
    let healthy = harness.service.agenda(&query).unwrap();
    assert_eq!(healthy["context"]["configured"], true);
    let last_sync = healthy["context"]["accounts"][0]["lastSync"].clone();
    assert!(last_sync.is_string());
    harness
        .database
        .set_sync_result(
            "account-1",
            Some("credentials_expired"),
            Some("Sign in again"),
        )
        .unwrap();
    harness
        .database
        .upsert_account("account-2", "subject-2", "two@example.com", "Work")
        .unwrap();
    let before = harness.secrets.lookup_calls.load(Ordering::SeqCst);
    for _ in 0..10 {
        let result = harness.service.agenda(&query).unwrap();
        assert_eq!(result["items"], healthy["items"]);
        assert_eq!(result["context"]["accounts"][0]["lastSync"], last_sync);
        assert_eq!(
            result["context"]["accounts"][0]["errorCode"],
            "credentials_expired"
        );
        assert_eq!(result["context"]["accounts"].as_array().unwrap().len(), 2);
    }
    assert_eq!(harness.secrets.lookup_calls.load(Ordering::SeqCst), before);
    assert_eq!(harness.google.refresh_calls.load(Ordering::SeqCst), 1);
    let filtered = harness
        .service
        .agenda(&json!({"start":"2026-08-31", "end":"2026-09-01", "account":"account-2"}))
        .unwrap();
    assert_eq!(filtered["items"], json!([]));
    assert_eq!(filtered["context"]["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["context"]["accounts"][0]["id"], "account-2");
    assert_eq!(
        harness
            .service
            .agenda(&json!({"start":"2026-08-31", "end":"2026-09-01", "account":"missing"}))
            .unwrap_err()
            .code,
        "account_not_found"
    );
}

#[test]
fn cached_read_does_not_wait_for_inflight_refresh() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::channel();
    *harness.google.refresh_gate.lock().unwrap() = Some(AuthorizationGate {
        started: started_tx,
        release: release_rx,
    });
    let service = harness.service.clone();
    let refresh = thread::spawn(move || service.refresh(&json!({})));
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let service = harness.service.clone();
    let (result_tx, result_rx) = mpsc::channel();
    let read = thread::spawn(move || {
        result_tx
            .send(service.agenda(&json!({"start":"2026-08-31", "end":"2026-09-01"})))
            .unwrap();
    });
    let result = result_rx.recv_timeout(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    refresh.join().unwrap().unwrap();
    read.join().unwrap();
    let result = result.expect("cached read waited for refresh").unwrap();
    assert_eq!(result["context"]["accounts"][0]["syncing"], true);
    assert_eq!(harness.google.refresh_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn database_and_socket_are_private() {
    let harness = Harness::new();
    assert_eq!(
        fs::metadata(harness.database.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let socket_path = harness
        ._temporary
        .path()
        .join("runtime/omarchy-calendar.sock");
    let server = CalendarServer::bind(&socket_path, harness.service).unwrap();
    assert_eq!(
        fs::metadata(&socket_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    server.cleanup();
}

#[test]
fn legacy_invalid_grant_is_migrated_to_reconnect_state() {
    let temporary = tempfile::tempdir().unwrap();
    let database_path = temporary.path().join("state/agenda.db");
    fs::create_dir_all(database_path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE accounts (
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
            INSERT INTO accounts VALUES (
                'account-1', 'google-subject-1', 'person@example.com', 'Personal',
                1, '2026-09-03T00:00:00Z', 'invalid_grant',
                '2026-09-01T00:00:00Z', '2026-09-03T00:00:00Z'
            );
            "#,
        )
        .unwrap();
    drop(connection);

    let database = Database::new(&database_path).unwrap();
    let account = database.account("account-1").unwrap().unwrap();
    assert_eq!(account.error_code.as_deref(), Some("credentials_expired"));
    assert_eq!(
        account.error.as_deref(),
        Some("Google access expired. Sign in again.")
    );
}

#[test]
fn initial_and_incremental_sync_normalize_agenda() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    let first = harness.service.refresh(&json!({})).unwrap();
    assert_eq!(first["accounts"][0]["ok"], true);
    let initial = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "all"}))
        .unwrap();
    let types = initial["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(types, ["event", "task", "event"]);
    let review = initial["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == "Design review")
        .unwrap();
    assert_eq!(review["icalUid"], "shared@example.com");
    assert_eq!(review["occurrenceStart"], "2026-08-31T10:00:00+05:30");
    assert_eq!(
        review["reminders"],
        json!([{"method": "popup", "minutes": 10}])
    );
    assert_eq!(review["accountId"], "account-1");
    let task = initial["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "task")
        .unwrap();
    assert_eq!(task["dueDate"], "2026-08-31");

    let second = harness.service.refresh(&json!({})).unwrap();
    assert_eq!(second["accounts"][0]["ok"], true);
    let changed = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "account-1"}))
        .unwrap();
    assert!(
        !changed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["title"] == "All-day plan")
    );
    let moved = changed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "event")
        .unwrap();
    assert_eq!(moved["title"], "Design review moved");
    assert_eq!(moved["occurrenceStart"], "2026-08-31T10:00:00+05:30");
    assert_eq!(
        *harness.google.seen_tokens.lock().unwrap(),
        vec![None, Some("sync-1".into())]
    );
}

#[test]
fn expired_sync_token_rebuilds_calendar() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    harness.service.refresh(&json!({})).unwrap();
    *harness.google.sync_mode.lock().unwrap() = "gone_once".into();
    let result = harness.service.refresh(&json!({})).unwrap();
    assert_eq!(result["accounts"][0]["ok"], true);
    let seen = harness.google.seen_tokens.lock().unwrap();
    assert_eq!(&seen[seen.len() - 2..], &[Some("sync-1".into()), None]);
    let agenda = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "all"}))
        .unwrap();
    assert!(
        agenda["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["title"] == "All-day plan")
    );
}

#[test]
fn multiple_accounts_merge_and_filter_without_id_collisions() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    harness
        .database
        .upsert_account("account-2", "google-subject-2", "work@example.com", "Work")
        .unwrap();
    harness
        .secrets
        .store("account-2", "refresh-token-never-cache")
        .unwrap();

    let refreshed = harness.service.refresh(&json!({})).unwrap();
    assert_eq!(refreshed["accounts"].as_array().unwrap().len(), 2);
    assert!(
        refreshed["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|account| account["ok"] == true)
    );

    let all = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "all"}))
        .unwrap();
    let all_items = all["items"].as_array().unwrap();
    assert_eq!(all_items.len(), 6);

    let personal = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "account-1"}))
        .unwrap();
    let work = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "account-2"}))
        .unwrap();
    assert_eq!(personal["items"].as_array().unwrap().len(), 3);
    assert_eq!(work["items"].as_array().unwrap().len(), 3);
    assert!(
        personal["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["accountId"] == "account-1")
    );
    assert!(
        work["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["accountId"] == "account-2")
    );

    let shared_copies = all_items
        .iter()
        .filter(|item| item["icalUid"] == "shared@example.com")
        .collect::<Vec<_>>();
    assert_eq!(shared_copies.len(), 2);
    assert_ne!(shared_copies[0]["id"], shared_copies[1]["id"]);
}

#[test]
fn add_remove_keeps_secrets_outside_database() {
    let harness = Harness::new();
    let result = harness
        .service
        .add_account(&json!({"label": "Work"}))
        .unwrap();
    let account_id = result["account"]["id"].as_str().unwrap().to_owned();
    assert_eq!(result["account"]["label"], "Work");
    assert_eq!(
        harness.secrets.values.lock().unwrap().get(&account_id),
        Some(&"refresh-token-never-cache".into())
    );
    let database_bytes = fs::read(harness.database.path()).unwrap();
    assert!(
        !database_bytes
            .windows(b"refresh-token-never-cache".len())
            .any(|window| window == b"refresh-token-never-cache")
    );
    assert!(
        !database_bytes
            .windows(b"access-token-never-cache".len())
            .any(|window| window == b"access-token-never-cache")
    );

    let removed = harness
        .service
        .remove_account(&json!({"accountId": account_id}))
        .unwrap();
    assert_eq!(removed["removed"], true);
    assert_eq!(removed["revoked"], true);
    assert_eq!(
        *harness.google.revoked.lock().unwrap(),
        vec!["refresh-token-never-cache"]
    );
    assert!(
        !harness
            .secrets
            .values
            .lock()
            .unwrap()
            .contains_key(&account_id)
    );
}

#[test]
fn reconnect_replaces_token_preserves_cache_and_targets_later_sync() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    harness.service.refresh(&json!({})).unwrap();
    harness
        .secrets
        .values
        .lock()
        .unwrap()
        .insert("account-1".into(), "expired-token".into());
    harness
        .database
        .set_sync_result(
            "account-1",
            Some("credentials_expired"),
            Some("Google access expired. Sign in again."),
        )
        .unwrap();
    let cached_before = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "account-1"}))
        .unwrap();

    let reconnected = harness
        .service
        .reconnect_account(&json!({"accountId": "account-1"}))
        .unwrap();
    assert_eq!(reconnected["reconnected"], true);
    assert_eq!(reconnected["account"]["needsReconnect"], false);
    assert_eq!(reconnected["account"]["connected"], true);
    assert_eq!(reconnected["account"]["label"], "Personal");
    assert_eq!(
        harness
            .secrets
            .values
            .lock()
            .unwrap()
            .get("account-1")
            .map(String::as_str),
        Some("refresh-token-never-cache")
    );
    let cached_after = harness
        .service
        .agenda(&json!({"start": "2026-08-31", "end": "2026-09-01", "account": "account-1"}))
        .unwrap();
    assert_eq!(cached_after["items"], cached_before["items"]);

    let refreshed = harness
        .service
        .refresh(&json!({"accountId": "account-1"}))
        .unwrap();
    assert_eq!(refreshed["accounts"][0]["ok"], true);
}

#[test]
fn reconnect_rejects_wrong_google_identity_and_preserves_old_token() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    harness
        .secrets
        .values
        .lock()
        .unwrap()
        .insert("account-1".into(), "expired-token".into());
    *harness.google.authorization_subject.lock().unwrap() = "wrong-subject".into();
    *harness.google.authorization_email.lock().unwrap() = "wrong@example.com".into();

    let error = harness
        .service
        .reconnect_account(&json!({"accountId": "account-1"}))
        .expect_err("wrong Google identity must be rejected");
    assert_eq!(error.code, "oauth_account_mismatch");
    assert_eq!(
        harness
            .secrets
            .values
            .lock()
            .unwrap()
            .get("account-1")
            .map(String::as_str),
        Some("expired-token")
    );
    assert_eq!(
        *harness.google.revoked.lock().unwrap(),
        vec!["refresh-token-never-cache"]
    );
}

#[test]
fn reconnect_oauth_and_secret_failures_preserve_old_token() {
    let harness = Harness::new();
    harness.add_database_account("account-1");
    harness
        .secrets
        .values
        .lock()
        .unwrap()
        .insert("account-1".into(), "expired-token".into());

    harness.google.fail_next_authorization();
    let oauth_error = harness
        .service
        .reconnect_account(&json!({"accountId": "account-1"}))
        .expect_err("OAuth failure must be reported");
    assert_eq!(oauth_error.code, "oauth_denied");
    assert_eq!(
        harness
            .secrets
            .values
            .lock()
            .unwrap()
            .get("account-1")
            .map(String::as_str),
        Some("expired-token")
    );

    *harness.secrets.fail_store.lock().unwrap() = true;
    let store_error = harness
        .service
        .reconnect_account(&json!({"accountId": "account-1"}))
        .expect_err("secret-store failure must be reported");
    assert_eq!(store_error.code, "secret_service_error");
    assert_eq!(
        harness
            .secrets
            .values
            .lock()
            .unwrap()
            .get("account-1")
            .map(String::as_str),
        Some("expired-token")
    );
}

#[test]
fn concurrent_add_account_is_rejected_and_guard_is_released() {
    let temporary = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::new(temporary.path().join("state/agenda.db")).unwrap());
    let (google, authorization_started, release_authorization) =
        FakeGoogle::with_blocked_first_authorization();
    let google = Arc::new(google);
    let secrets = Arc::new(FakeSecrets::default());
    let service = Arc::new(CalendarService::new(database, google.clone(), secrets));

    let first_service = service.clone();
    let first = thread::spawn(move || first_service.add_account(&json!({})));
    authorization_started
        .recv_timeout(Duration::from_secs(2))
        .expect("first authorization did not start");

    let concurrent = service.add_account(&json!({}));
    release_authorization
        .send(())
        .expect("first authorization stopped waiting");
    let first_result = first.join().expect("first add-account thread panicked");

    let error = concurrent.expect_err("concurrent OAuth should be rejected");
    assert_eq!(error.code, "oauth_in_progress");
    assert_eq!(error.message, "Google sign-in is already in progress");
    assert!(first_result.is_ok());
    assert_eq!(google.authorize_calls.load(Ordering::SeqCst), 1);

    let retry = service.add_account(&json!({}));
    assert!(retry.is_ok());
    assert_eq!(google.authorize_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn abandoned_authorization_can_be_cancelled_and_retried() {
    let temporary = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::new(temporary.path().join("state/agenda.db")).unwrap());
    let (google, authorization_started, _release_authorization) =
        FakeGoogle::with_blocked_first_authorization();
    let google = Arc::new(google);
    let secrets = Arc::new(FakeSecrets::default());
    let service = Arc::new(CalendarService::new(database, google.clone(), secrets));

    let first_service = service.clone();
    let first = thread::spawn(move || first_service.add_account(&json!({})));
    authorization_started
        .recv_timeout(Duration::from_secs(2))
        .expect("authorization did not start");

    let waiting = service.state().unwrap();
    assert_eq!(waiting["oauthInProgress"], true);
    assert_eq!(waiting["oauthCancelable"], true);
    let cancelled = service.cancel_add_account().unwrap();
    assert_eq!(cancelled["cancelled"], true);

    let error = first
        .join()
        .expect("add-account thread panicked")
        .expect_err("cancelled OAuth should fail");
    assert_eq!(error.code, "oauth_cancelled");
    let idle = service.state().unwrap();
    assert_eq!(idle["oauthInProgress"], false);
    assert_eq!(idle["oauthCancelable"], false);

    let retry = service.add_account(&json!({}));
    assert!(retry.is_ok());
    assert_eq!(google.authorize_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn reconnect_authorization_can_be_cancelled_without_losing_token() {
    let temporary = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::new(temporary.path().join("state/agenda.db")).unwrap());
    database
        .upsert_account(
            "account-1",
            "google-subject-1",
            "person@example.com",
            "Personal",
        )
        .unwrap();
    let (google, authorization_started, _release_authorization) =
        FakeGoogle::with_blocked_first_authorization();
    let google = Arc::new(google);
    let secrets = Arc::new(FakeSecrets::default());
    secrets
        .values
        .lock()
        .unwrap()
        .insert("account-1".into(), "expired-token".into());
    let service = Arc::new(CalendarService::new(database, google, secrets.clone()));

    let reconnecting_service = service.clone();
    let reconnect = thread::spawn(move || {
        reconnecting_service.reconnect_account(&json!({"accountId": "account-1"}))
    });
    authorization_started
        .recv_timeout(Duration::from_secs(2))
        .expect("reconnect authorization did not start");
    assert_eq!(service.cancel_add_account().unwrap()["cancelled"], true);
    let error = reconnect
        .join()
        .expect("reconnect thread panicked")
        .expect_err("cancelled reconnect should fail");
    assert_eq!(error.code, "oauth_cancelled");
    assert_eq!(
        secrets
            .values
            .lock()
            .unwrap()
            .get("account-1")
            .map(String::as_str),
        Some("expired-token")
    );
}

#[test]
fn cancelling_without_active_authorization_is_idempotent() {
    let harness = Harness::new();
    let result = harness.service.cancel_add_account().unwrap();
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["reason"], "not_in_progress");
}

#[test]
fn add_account_guard_is_released_after_authorization_failure() {
    let harness = Harness::new();
    harness.google.fail_next_authorization();

    let error = harness
        .service
        .add_account(&json!({}))
        .expect_err("first authorization should fail");
    assert_eq!(error.code, "oauth_denied");

    let retry = harness.service.add_account(&json!({}));
    assert!(retry.is_ok());
    assert_eq!(harness.google.authorize_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn newline_json_protocol_returns_empty_state() {
    let harness = Harness::new();
    let socket_path = harness
        ._temporary
        .path()
        .join("runtime/omarchy-calendar.sock");
    let server = Arc::new(CalendarServer::bind(&socket_path, harness.service).unwrap());
    let server_thread = {
        let server = server.clone();
        thread::spawn(move || server.serve().unwrap())
    };
    let mut client = UnixStream::connect(&socket_path).unwrap();
    client
        .write_all(b"{\"id\":\"test-1\",\"method\":\"get_state\",\"params\":{}}\n")
        .unwrap();
    let mut line = String::new();
    BufReader::new(client.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["id"], "test-1");
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["status"], "empty");
    assert_eq!(response["result"]["accounts"], json!([]));
    drop(client);
    server.shutdown();
    server_thread.join().unwrap();
    server.cleanup();
}

#[test]
fn same_socket_cancel_interrupts_blocked_add_account() {
    let temporary = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::new(temporary.path().join("state/agenda.db")).unwrap());
    let (google, authorization_started, _release_authorization) =
        FakeGoogle::with_blocked_first_authorization();
    let service = Arc::new(CalendarService::new(
        database,
        Arc::new(google),
        Arc::new(FakeSecrets::default()),
    ));
    let socket_path = temporary.path().join("runtime/omarchy-calendar.sock");
    let server = Arc::new(CalendarServer::bind(&socket_path, service).unwrap());
    let server_thread = {
        let server = server.clone();
        thread::spawn(move || server.serve().unwrap())
    };
    let mut client = UnixStream::connect(&socket_path).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(b"{\"id\":\"add\",\"method\":\"add_account\",\"params\":{}}\n")
        .unwrap();
    client.flush().unwrap();
    authorization_started
        .recv_timeout(Duration::from_secs(2))
        .expect("authorization did not start");
    client
        .write_all(b"{\"id\":\"cancel\",\"method\":\"cancel_add_account\",\"params\":{}}\n")
        .unwrap();
    client.flush().unwrap();

    let mut reader = BufReader::new(client.try_clone().unwrap());
    let mut responses = HashMap::new();
    while responses.len() < 2 {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            responses.insert(id.to_owned(), message);
        }
    }
    assert_eq!(responses["cancel"]["ok"], true);
    assert_eq!(responses["cancel"]["result"]["cancelled"], true);
    assert_eq!(responses["add"]["ok"], false);
    assert_eq!(responses["add"]["error"]["code"], "oauth_cancelled");

    drop(reader);
    drop(client);
    server.shutdown();
    server_thread.join().unwrap();
    server.cleanup();
}
