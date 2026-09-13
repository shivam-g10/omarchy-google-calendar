use crate::browser::{BrowserOpenResult, open_browser};
use crate::{CalendarError, Database, GoogleApi, OAuthAttempt, OAuthPhase, Result, safe_message};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, RwLock, TryLockError};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;
use uuid::Uuid;

const SECRET_SERVICE: &str = "omarchy-calendar";
const SECRET_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

pub trait SecretStoreApi: Send + Sync {
    fn store(&self, account_id: &str, refresh_token: &str) -> Result<()>;
    fn lookup(&self, account_id: &str) -> Result<String>;
    fn clear(&self, account_id: &str) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct SecretStore {
    executable: Option<PathBuf>,
}

impl SecretStore {
    pub fn new() -> Self {
        Self {
            executable: find_executable("secret-tool"),
        }
    }

    fn executable(&self) -> Result<&Path> {
        self.executable.as_deref().ok_or_else(|| {
            CalendarError::new("secret_service_unavailable", "secret-tool is not installed")
        })
    }
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStoreApi for SecretStore {
    fn store(&self, account_id: &str, refresh_token: &str) -> Result<()> {
        let executable = self.executable()?;
        let arguments = [
            "store",
            "--label=Omarchy Calendar Google account",
            "service",
            SECRET_SERVICE,
            "account-id",
            account_id,
        ];
        let output =
            run_secret_tool(executable, &arguments, Some(refresh_token), false).map_err(|_| {
                CalendarError::new("secret_service_error", "Could not store Google credentials")
            })?;
        if !output.status.success() {
            return Err(CalendarError::new(
                "secret_service_error",
                safe_message(output.stderr, "Could not store Google credentials"),
            ));
        }
        Ok(())
    }

    fn lookup(&self, account_id: &str) -> Result<String> {
        let executable = self.executable()?;
        let arguments = [
            "lookup",
            "service",
            SECRET_SERVICE,
            "account-id",
            account_id,
        ];
        let output = run_secret_tool(executable, &arguments, None, true).map_err(|_| {
            CalendarError::new("secret_service_error", "Could not read Google credentials")
        })?;
        let token = output.stdout.trim();
        if !output.status.success() || token.is_empty() {
            return Err(CalendarError::new(
                "credentials_missing",
                "Google account needs reconnection",
            ));
        }
        Ok(token.to_owned())
    }

    fn clear(&self, account_id: &str) -> Result<()> {
        let executable = self.executable()?;
        let arguments = ["clear", "service", SECRET_SERVICE, "account-id", account_id];
        let output = run_secret_tool(executable, &arguments, None, false).map_err(|_| {
            CalendarError::new(
                "secret_service_error",
                "Could not remove Google credentials",
            )
        })?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(CalendarError::new(
                "secret_service_error",
                safe_message(output.stderr, "Could not remove Google credentials"),
            ));
        }
        Ok(())
    }
}

type Notifier = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct CalendarService {
    database: Arc<Database>,
    google: Arc<dyn GoogleApi>,
    secret_store: Arc<dyn SecretStoreApi>,
    notifier: RwLock<Option<Notifier>>,
    syncing: Mutex<HashSet<String>>,
    refresh_lock: Mutex<()>,
    oauth_attempt: Mutex<Option<Arc<OAuthAttempt>>>,
}

impl CalendarService {
    pub fn new(
        database: Arc<Database>,
        google: Arc<dyn GoogleApi>,
        secret_store: Arc<dyn SecretStoreApi>,
    ) -> Self {
        Self {
            database,
            google,
            secret_store,
            notifier: RwLock::new(None),
            syncing: Mutex::new(HashSet::new()),
            refresh_lock: Mutex::new(()),
            oauth_attempt: Mutex::new(None),
        }
    }

    pub fn set_notifier(&self, notifier: Notifier) {
        *self.notifier.write().expect("notifier lock poisoned") = Some(notifier);
    }

    pub fn state(&self) -> Result<Value> {
        let syncing = self.syncing.lock().expect("sync state poisoned").clone();
        let mut state = self.database.state(&syncing, self.google.configured())?;
        let phase = self
            .oauth_attempt
            .lock()
            .expect("OAuth state poisoned")
            .as_ref()
            .map(|attempt| attempt.phase());
        if let Some(object) = state.as_object_mut() {
            object.insert("oauthInProgress".into(), json!(phase.is_some()));
            object.insert(
                "oauthCancelable".into(),
                json!(phase == Some(OAuthPhase::Waiting)),
            );
            object.insert(
                "oauthCancelling".into(),
                json!(phase == Some(OAuthPhase::Cancelled)),
            );
            object.insert(
                "oauthFinishing".into(),
                json!(phase == Some(OAuthPhase::Finishing)),
            );
        }
        Ok(state)
    }

    pub fn agenda(&self, parameters: &Value) -> Result<Value> {
        let start = parameters
            .get("start")
            .and_then(Value::as_str)
            .ok_or_else(|| CalendarError::new("invalid_params", "start and end must be strings"))?;
        let end = parameters
            .get("end")
            .and_then(Value::as_str)
            .ok_or_else(|| CalendarError::new("invalid_params", "start and end must be strings"))?;
        let account = parameters
            .get("account")
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    CalendarError::new("invalid_params", "account must be all or an account id")
                })
            })
            .transpose()?
            .unwrap_or("all");
        self.database.agenda(start, end, account)
    }

    pub fn add_account(&self, parameters: &Value) -> Result<Value> {
        let oauth_guard = OAuthGuard::acquire(self)?;
        self.notify_state();
        let result = self.add_account_inner(parameters, oauth_guard.attempt());
        drop(oauth_guard);
        result
    }

    fn add_account_inner(&self, parameters: &Value, attempt: &OAuthAttempt) -> Result<Value> {
        let authorization = self.google.authorize(attempt)?;
        if !attempt.begin_finishing() {
            return Err(CalendarError::new(
                "oauth_cancelled",
                "Google sign-in was cancelled",
            ));
        }
        let existing = self.database.account_by_sub(&authorization.subject)?;
        let account_id = existing
            .as_ref()
            .map(|account| account.account_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let label = parameters
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(100).collect::<String>())
            .unwrap_or_else(|| authorization.email.clone());
        self.secret_store
            .store(&account_id, &authorization.refresh_token)?;
        if let Err(error) = self.database.upsert_account(
            &account_id,
            &authorization.subject,
            &authorization.email,
            &label,
        ) {
            if existing.is_none() {
                let _ = self.secret_store.clear(&account_id);
            }
            return Err(error);
        }
        self.notify_state();
        let sync = self.refresh(&json!({"accountId": account_id}))?;
        let state = self.state()?;
        let account = state["accounts"]
            .as_array()
            .and_then(|accounts| accounts.iter().find(|account| account["id"] == account_id))
            .cloned()
            .ok_or_else(|| CalendarError::new("internal_error", "Connected account disappeared"))?;
        Ok(json!({"account": account, "sync": sync}))
    }

    pub fn cancel_add_account(&self) -> Result<Value> {
        let attempt = self
            .oauth_attempt
            .lock()
            .expect("OAuth state poisoned")
            .clone();
        let Some(attempt) = attempt else {
            return Ok(json!({"cancelled": false, "reason": "not_in_progress"}));
        };
        if attempt.cancel() {
            self.notify_state();
            return Ok(json!({"cancelled": true}));
        }
        match attempt.phase() {
            OAuthPhase::Cancelled => Ok(json!({"cancelled": true, "reason": "already_cancelled"})),
            OAuthPhase::Finishing => {
                self.notify_state();
                Ok(json!({"cancelled": false, "reason": "finishing"}))
            }
            OAuthPhase::Waiting => Ok(json!({"cancelled": false, "reason": "try_again"})),
        }
    }

    pub fn reconnect_account(&self, parameters: &Value) -> Result<Value> {
        let account_id = parameters
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CalendarError::new("invalid_params", "accountId is required"))?;
        let account = self.database.account(account_id)?.ok_or_else(|| {
            CalendarError::new("account_not_found", "Google account was not found")
        })?;
        let oauth_guard = OAuthGuard::acquire(self)?;
        self.notify_state();
        let result = self.reconnect_account_inner(&account, oauth_guard.attempt());
        drop(oauth_guard);
        result
    }

    fn reconnect_account_inner(
        &self,
        account: &crate::database::AccountRow,
        attempt: &OAuthAttempt,
    ) -> Result<Value> {
        let authorization = self.google.authorize(attempt)?;
        if authorization.subject != account.google_sub {
            let _ = self.google.revoke(&authorization.refresh_token);
            return Err(CalendarError::new(
                "oauth_account_mismatch",
                format!(
                    "Wrong Google account selected. Sign in as {}.",
                    account.email
                ),
            ));
        }
        self.secret_store
            .store(&account.account_id, &authorization.refresh_token)?;
        self.database.upsert_account(
            &account.account_id,
            &authorization.subject,
            &authorization.email,
            &account.label,
        )?;
        self.notify_state();
        let state = self.state()?;
        let updated = state["accounts"]
            .as_array()
            .and_then(|accounts| {
                accounts
                    .iter()
                    .find(|candidate| candidate["id"] == account.account_id)
            })
            .cloned()
            .ok_or_else(|| {
                CalendarError::new("internal_error", "Reconnected account disappeared")
            })?;
        Ok(json!({"account": updated, "reconnected": true}))
    }

    pub fn remove_account(&self, parameters: &Value) -> Result<Value> {
        let account_id = parameters
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CalendarError::new("invalid_params", "accountId is required"))?;
        if self.database.account(account_id)?.is_none() {
            return Err(CalendarError::new(
                "account_not_found",
                "Google account was not found",
            ));
        }
        let refresh_token = self.secret_store.lookup(account_id).ok();
        let revoked = refresh_token
            .as_deref()
            .map(|token| self.google.revoke(token))
            .unwrap_or(false);
        self.secret_store.clear(account_id)?;
        let removed = self.database.delete_account(account_id)?;
        self.notify_state();
        self.notify(
            "agenda_changed",
            json!({"start": Value::Null, "end": Value::Null}),
        );
        Ok(json!({"accountId": account_id, "removed": removed, "revoked": revoked}))
    }

    pub fn refresh(&self, parameters: &Value) -> Result<Value> {
        let requested = parameters.get("accountId").map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CalendarError::new("invalid_params", "accountId must be a string"))
        });
        let requested = requested.transpose()?;
        let _refresh_guard = match self.refresh_lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Ok(json!({"started": false, "reason": "already_syncing", "accounts": []}));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(CalendarError::new(
                    "internal_error",
                    "Calendar refresh lock failed",
                ));
            }
        };
        let mut accounts = self.database.configured_accounts()?;
        if let Some(account_id) = requested {
            accounts.retain(|account| account.account_id == account_id);
            if accounts.is_empty() {
                return Err(CalendarError::new(
                    "account_not_found",
                    "Google account was not found",
                ));
            }
        }
        let mut results = Vec::new();
        for account in accounts {
            if !account.enabled {
                continue;
            }
            {
                self.syncing
                    .lock()
                    .expect("sync state poisoned")
                    .insert(account.account_id.clone());
            }
            let syncing_guard = SyncingAccountGuard {
                service: self,
                account_id: account.account_id.clone(),
            };
            self.notify_state();
            let sync_result = self.sync_account(&account.account_id);
            match sync_result {
                Ok(()) => {
                    self.database
                        .set_sync_result(&account.account_id, None, None)?;
                    results
                        .push(json!({"id": account.account_id, "ok": true, "error": Value::Null}));
                }
                Err(error) => {
                    let message = safe_message(&error.message, "Operation failed");
                    self.database.set_sync_result(
                        &account.account_id,
                        Some(&error.code),
                        Some(&message),
                    )?;
                    results.push(json!({"id": account.account_id, "ok": false, "error": message}));
                }
            }
            drop(syncing_guard);
        }
        if !results.is_empty() {
            self.notify(
                "agenda_changed",
                json!({"start": Value::Null, "end": Value::Null}),
            );
        }
        Ok(json!({"started": true, "accounts": results}))
    }

    fn sync_account(&self, account_id: &str) -> Result<()> {
        let refresh_token = self.secret_store.lookup(account_id)?;
        let access_token = self.google.refresh_access_token(&refresh_token)?;
        let calendars = self.google.list_calendars(&access_token)?;
        self.database.replace_calendars(account_id, &calendars)?;
        for calendar in self.database.calendars(account_id)? {
            let mut sync_token = calendar.sync_token;
            if let Some(full_sync_at) = &calendar.full_sync_at
                && let Ok(timestamp) = DateTime::parse_from_rfc3339(full_sync_at)
                && Utc::now() - timestamp.with_timezone(&Utc) >= ChronoDuration::days(30)
            {
                sync_token = None;
            }
            let mut replace = sync_token.is_none();
            let events = match self.google.list_events(
                &access_token,
                &calendar.calendar_id,
                sync_token.as_deref(),
            ) {
                Ok(value) => value,
                Err(error) if error.http_status == Some(410) && sync_token.is_some() => {
                    replace = true;
                    self.google
                        .list_events(&access_token, &calendar.calendar_id, None)?
                }
                Err(error) => return Err(error),
            };
            self.database.apply_events(
                account_id,
                &calendar.calendar_id,
                &events.0,
                &events.1,
                replace,
            )?;
        }
        let task_lists = self.google.list_task_lists(&access_token)?;
        let mut tasks_by_list = HashMap::new();
        for task_list in &task_lists {
            tasks_by_list.insert(
                task_list.id.clone(),
                self.google.list_tasks(&access_token, &task_list.id)?,
            );
        }
        self.database
            .replace_tasks(account_id, &task_lists, &tasks_by_list)
    }

    pub fn open_item(&self, parameters: &Value) -> Result<Value> {
        let item_id = parameters
            .get("itemId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CalendarError::new("invalid_params", "itemId is required"))?;
        let url = self
            .database
            .item_url(item_id)?
            .ok_or_else(|| CalendarError::new("item_not_found", "Agenda item has no link"))?;
        let parsed = Url::parse(&url)
            .map_err(|_| CalendarError::new("unsafe_url", "Agenda item link is not HTTPS"))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(CalendarError::new(
                "unsafe_url",
                "Agenda item link is not HTTPS",
            ));
        }
        match open_browser(&url) {
            BrowserOpenResult::Opened => {}
            BrowserOpenResult::BraveHandoffFailed => {
                return Err(CalendarError::new(
                    "browser_handoff_failed",
                    "Could not open agenda item; fully exit and reopen Brave, then try again",
                ));
            }
            BrowserOpenResult::Failed => {
                return Err(CalendarError::new(
                    "browser_open_failed",
                    "Could not open agenda item",
                ));
            }
        }
        Ok(json!({"itemId": item_id, "opened": true}))
    }

    pub fn dispatch(&self, method: &str, parameters: &Value) -> Result<Value> {
        match method {
            "get_state" | "status" => self.state(),
            "get_agenda" | "list_agenda" => self.agenda(parameters),
            "refresh" => self.refresh(parameters),
            "add_account" | "add" => self.add_account(parameters),
            "reconnect_account" | "reconnect" => self.reconnect_account(parameters),
            "cancel_add_account" | "cancel" => self.cancel_add_account(),
            "remove_account" | "remove" => self.remove_account(parameters),
            "open_item" => self.open_item(parameters),
            _ => Err(CalendarError::new(
                "method_not_found",
                format!("Unknown method: {method}"),
            )),
        }
    }

    pub fn database(&self) -> &Arc<Database> {
        &self.database
    }

    fn notify_state(&self) {
        if let Ok(state) = self.state() {
            self.notify("state_changed", state);
        }
    }

    fn notify(&self, event: &str, data: Value) {
        let notifier = self
            .notifier
            .read()
            .expect("notifier lock poisoned")
            .clone();
        if let Some(notifier) = notifier {
            notifier(event, data);
        }
    }
}

struct OAuthGuard<'a> {
    service: &'a CalendarService,
    attempt: Arc<OAuthAttempt>,
}

impl<'a> OAuthGuard<'a> {
    fn acquire(service: &'a CalendarService) -> Result<Self> {
        let mut current = service.oauth_attempt.lock().expect("OAuth state poisoned");
        if current.is_some() {
            return Err(CalendarError::new(
                "oauth_in_progress",
                "Google sign-in is already in progress",
            ));
        }
        let attempt = Arc::new(OAuthAttempt::new());
        *current = Some(Arc::clone(&attempt));
        Ok(Self { service, attempt })
    }

    fn attempt(&self) -> &OAuthAttempt {
        &self.attempt
    }
}

impl Drop for OAuthGuard<'_> {
    fn drop(&mut self) {
        let removed = {
            let mut current = self
                .service
                .oauth_attempt
                .lock()
                .expect("OAuth state poisoned");
            if current
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(attempt, &self.attempt))
            {
                *current = None;
                true
            } else {
                false
            }
        };
        if removed {
            self.service.notify_state();
        }
    }
}

struct SyncingAccountGuard<'a> {
    service: &'a CalendarService,
    account_id: String,
}

impl Drop for SyncingAccountGuard<'_> {
    fn drop(&mut self) {
        self.service
            .syncing
            .lock()
            .expect("sync state poisoned")
            .remove(&self.account_id);
        self.service.notify_state();
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_secret_tool(
    executable: &Path,
    arguments: &[&str],
    input: Option<&str>,
    capture_stdout: bool,
) -> std::io::Result<ProcessOutput> {
    let mut child = Command::new(executable)
        .args(arguments)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(input.as_bytes())?;
    }
    let status = wait_for_child(&mut child, SECRET_TOOL_TIMEOUT)?;
    let mut stdout = String::new();
    if let Some(mut reader) = child.stdout.take() {
        reader.read_to_string(&mut stdout)?;
    }
    let mut stderr = String::new();
    if let Some(mut reader) = child.stderr.take() {
        reader.read_to_string(&mut stderr)?;
    }
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "secret-tool timed out",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_store_reports_missing_tool() {
        let store = SecretStore { executable: None };
        assert_eq!(
            store.lookup("missing").unwrap_err().code,
            "secret_service_unavailable"
        );
    }

    #[test]
    fn iso_timestamp_is_rfc3339() {
        assert!(DateTime::parse_from_rfc3339(&crate::database::iso_now()).is_ok());
    }
}
