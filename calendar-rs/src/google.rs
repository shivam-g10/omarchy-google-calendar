use crate::{AuthResult, CalendarEntry, CalendarError, Result, TaskListEntry, safe_message};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";
const TASKS_API: &str = "https://tasks.googleapis.com/tasks/v1";
const USERINFO_ENDPOINT: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const REVOCATION_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
const OAUTH_TIMEOUT_SECONDS: u64 = 300;
const MAX_GOOGLE_BODY: usize = 8 * 1024 * 1024;
const MAX_GOOGLE_ERROR_BODY: usize = 256 * 1024;
const MAX_CALLBACK_REQUEST: usize = 16 * 1024;
const SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "https://www.googleapis.com/auth/calendar.events.readonly",
    "https://www.googleapis.com/auth/tasks.readonly",
];

pub trait GoogleApi: Send + Sync {
    fn configured(&self) -> bool;
    fn authorize(&self) -> Result<AuthResult>;
    fn refresh_access_token(&self, refresh_token: &str) -> Result<String>;
    fn revoke(&self, refresh_token: &str) -> bool;
    fn list_calendars(&self, access_token: &str) -> Result<Vec<CalendarEntry>>;
    fn list_events(
        &self,
        access_token: &str,
        calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<(Vec<Value>, String)>;
    fn list_task_lists(&self, access_token: &str) -> Result<Vec<TaskListEntry>>;
    fn list_tasks(&self, access_token: &str, task_list_id: &str) -> Result<Vec<Value>>;
}

#[derive(Clone)]
pub struct GoogleClient {
    client_path: PathBuf,
    agent: ureq::Agent,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    browser_open: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

#[derive(Debug)]
struct ClientSettings {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}

impl GoogleClient {
    pub fn new(client_path: impl Into<PathBuf>) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Self {
            client_path: client_path.into(),
            agent: ureq::Agent::new_with_config(config),
            now: Arc::new(Utc::now),
            browser_open: Arc::new(open_browser),
        }
    }

    fn client(&self) -> Result<ClientSettings> {
        let content = fs::read_to_string(&self.client_path).map_err(|_| self.invalid_client())?;
        let payload: Value = serde_json::from_str(&content).map_err(|_| self.invalid_client())?;
        let installed = payload
            .get("installed")
            .and_then(Value::as_object)
            .ok_or_else(|| self.invalid_client())?;
        let client_id = installed
            .get("client_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| self.invalid_client())?;
        Ok(ClientSettings {
            client_id: client_id.to_owned(),
            client_secret: installed
                .get("client_secret")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            auth_uri: installed
                .get("auth_uri")
                .and_then(Value::as_str)
                .unwrap_or("https://accounts.google.com/o/oauth2/v2/auth")
                .to_owned(),
            token_uri: installed
                .get("token_uri")
                .and_then(Value::as_str)
                .unwrap_or("https://oauth2.googleapis.com/token")
                .to_owned(),
        })
    }

    fn invalid_client(&self) -> CalendarError {
        CalendarError::new(
            "google_client_invalid",
            format!(
                "Invalid Google Desktop OAuth client: {}",
                self.client_path.display()
            ),
        )
    }

    fn request_json(
        &self,
        method: &str,
        url: &str,
        access_token: Option<&str>,
        form: Option<&BTreeMap<String, String>>,
        allow_empty: bool,
    ) -> Result<Value> {
        let response = match method {
            "GET" => {
                let request = self.agent.get(url).header("Accept", "application/json");
                if let Some(token) = access_token {
                    request
                        .header("Authorization", &format!("Bearer {token}"))
                        .call()
                } else {
                    request.call()
                }
            }
            "POST" => {
                let request = self.agent.post(url).header("Accept", "application/json");
                let pairs = form
                    .into_iter()
                    .flat_map(|values| values.iter())
                    .map(|(key, value)| (key.as_str(), value.as_str()));
                request.send_form(pairs)
            }
            _ => {
                return Err(CalendarError::new(
                    "internal_error",
                    "Unsupported HTTP method",
                ));
            }
        }
        .map_err(|_| CalendarError::new("network_error", "Could not reach Google"))?;
        let status = response.status().as_u16();
        let mut response = response;
        let limit = if (200..300).contains(&status) {
            MAX_GOOGLE_BODY
        } else {
            MAX_GOOGLE_ERROR_BODY
        };
        let raw = response
            .body_mut()
            .with_config()
            .limit(limit as u64)
            .read_to_vec()
            .map_err(|_| CalendarError::new("network_error", "Could not read Google response"))?;
        if !(200..300).contains(&status) {
            let mut message = format!("Google API returned HTTP {status}");
            if let Ok(payload) = serde_json::from_slice::<Value>(&raw)
                && let Some(candidate) = google_error_message(&payload)
            {
                message = candidate;
            }
            return Err(CalendarError::google_http(
                status,
                safe_message(message, "Google API failed"),
            ));
        }
        if allow_empty && raw.is_empty() {
            return Ok(json!({}));
        }
        let value: Value = serde_json::from_slice(&raw).map_err(|_| {
            CalendarError::new("google_response_invalid", "Google returned invalid JSON")
        })?;
        if !value.is_object() {
            return Err(CalendarError::new(
                "google_response_invalid",
                "Google returned an invalid response",
            ));
        }
        Ok(value)
    }

    fn paged_items(
        &self,
        url: &str,
        access_token: &str,
        parameters: &BTreeMap<String, String>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let mut items = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut parsed = Url::parse(url).map_err(|_| {
                CalendarError::new("google_response_invalid", "Google API URL is invalid")
            })?;
            {
                let mut query = parsed.query_pairs_mut();
                for (key, value) in parameters {
                    query.append_pair(key, value);
                }
                if let Some(token) = &page_token {
                    query.append_pair("pageToken", token);
                }
            }
            let response =
                self.request_json("GET", parsed.as_str(), Some(access_token), None, false)?;
            if let Some(page_items) = response.get("items").and_then(Value::as_array) {
                items.extend(page_items.iter().filter(|item| item.is_object()).cloned());
            }
            page_token = response
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if page_token.is_none() {
                return Ok((
                    items,
                    response
                        .get("nextSyncToken")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                ));
            }
        }
    }

    fn revoke_at(&self, endpoint: &str, refresh_token: &str) -> bool {
        let form = BTreeMap::from([("token".into(), refresh_token.to_owned())]);
        self.request_json("POST", endpoint, None, Some(&form), true)
            .is_ok()
    }
}

impl GoogleApi for GoogleClient {
    fn configured(&self) -> bool {
        self.client_path.is_file()
    }

    fn authorize(&self) -> Result<AuthResult> {
        let client = self.client()?;
        let state = random_base64(32);
        let verifier = random_base64(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| {
            CalendarError::new(
                "oauth_callback_failed",
                "Could not start Google sign-in callback",
            )
        })?;
        listener.set_nonblocking(true).map_err(|_| {
            CalendarError::new(
                "oauth_callback_failed",
                "Could not configure Google sign-in callback",
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|_| {
                CalendarError::new("oauth_callback_failed", "Could not read callback port")
            })?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let mut authorization_url =
            Url::parse(&client.auth_uri).map_err(|_| self.invalid_client())?;
        {
            let mut query = authorization_url.query_pairs_mut();
            query
                .append_pair("client_id", &client.client_id)
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("response_type", "code")
                .append_pair("scope", &SCOPES.join(" "))
                .append_pair("access_type", "offline")
                .append_pair("prompt", "select_account consent")
                .append_pair("state", &state)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
        }
        if !(self.browser_open)(authorization_url.as_str()) {
            return Err(CalendarError::new(
                "browser_open_failed",
                "Could not open Google sign-in",
            ));
        }
        let values = receive_callback(&listener, Duration::from_secs(OAUTH_TIMEOUT_SECONDS))?
            .ok_or_else(|| CalendarError::new("oauth_timeout", "Google sign-in timed out"))?;
        let returned_state = values.get("state").map(String::as_str).unwrap_or("");
        if Sha256::digest(returned_state.as_bytes()) != Sha256::digest(state.as_bytes()) {
            return Err(CalendarError::new(
                "oauth_state_mismatch",
                "Google sign-in state did not match",
            ));
        }
        if let Some(error) = values.get("error") {
            return Err(CalendarError::new(
                "oauth_denied",
                safe_message(error, "Google sign-in denied"),
            ));
        }
        let code = values
            .get("code")
            .filter(|code| !code.is_empty())
            .ok_or_else(|| {
                CalendarError::new(
                    "oauth_code_missing",
                    "Google sign-in returned no authorization code",
                )
            })?;
        let mut form = BTreeMap::from([
            ("client_id".into(), client.client_id.clone()),
            ("code".into(), code.clone()),
            ("code_verifier".into(), verifier),
            ("grant_type".into(), "authorization_code".into()),
            ("redirect_uri".into(), redirect_uri),
        ]);
        if !client.client_secret.is_empty() {
            form.insert("client_secret".into(), client.client_secret);
        }
        let token_response =
            self.request_json("POST", &client.token_uri, None, Some(&form), false)?;
        let refresh_token = token_response
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let access_token = token_response
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let (Some(refresh_token), Some(access_token)) = (refresh_token, access_token) else {
            return Err(CalendarError::new(
                "oauth_credentials_missing",
                "Google returned no offline credentials; reconnect account",
            ));
        };
        let profile =
            self.request_json("GET", USERINFO_ENDPOINT, Some(access_token), None, false)?;
        let subject = profile
            .get("sub")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let email = profile
            .get("email")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let (Some(subject), Some(email)) = (subject, email) else {
            return Err(CalendarError::new(
                "oauth_profile_invalid",
                "Google account profile is incomplete",
            ));
        };
        Ok(AuthResult {
            refresh_token: refresh_token.to_owned(),
            access_token: access_token.to_owned(),
            subject: subject.to_owned(),
            email: email.to_owned(),
        })
    }

    fn refresh_access_token(&self, refresh_token: &str) -> Result<String> {
        let client = self.client()?;
        let mut form = BTreeMap::from([
            ("client_id".into(), client.client_id),
            ("refresh_token".into(), refresh_token.to_owned()),
            ("grant_type".into(), "refresh_token".into()),
        ]);
        if !client.client_secret.is_empty() {
            form.insert("client_secret".into(), client.client_secret);
        }
        let response = self.request_json("POST", &client.token_uri, None, Some(&form), false)?;
        response
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                CalendarError::new("token_refresh_failed", "Google account needs reconnection")
            })
    }

    fn revoke(&self, refresh_token: &str) -> bool {
        self.revoke_at(REVOCATION_ENDPOINT, refresh_token)
    }

    fn list_calendars(&self, access_token: &str) -> Result<Vec<CalendarEntry>> {
        let parameters = BTreeMap::from([
            ("maxResults".into(), "250".into()),
            ("showDeleted".into(), "false".into()),
            ("showHidden".into(), "false".into()),
        ]);
        let (items, _) = self.paged_items(
            &format!("{CALENDAR_API}/users/me/calendarList"),
            access_token,
            &parameters,
        )?;
        let mut calendars = Vec::new();
        for item in items {
            let Some(id) = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            if item
                .get("deleted")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let reminders = item
                .get("defaultReminders")
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
                .collect();
            calendars.push(CalendarEntry {
                id: id.to_owned(),
                label: item
                    .get("summaryOverride")
                    .or_else(|| item.get("summary"))
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned(),
                color: item
                    .get("backgroundColor")
                    .and_then(Value::as_str)
                    .unwrap_or("#8ab4f8")
                    .to_owned(),
                selected: !item.get("hidden").and_then(Value::as_bool).unwrap_or(false),
                primary: item
                    .get("primary")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                default_reminders: Value::Array(reminders),
            });
        }
        Ok(calendars)
    }

    fn list_events(
        &self,
        access_token: &str,
        calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<(Vec<Value>, String)> {
        let parameters = event_parameters((self.now)(), sync_token);
        let calendar_id = encode_path_segment(calendar_id);
        let (items, next_sync_token) = self.paged_items(
            &format!("{CALENDAR_API}/calendars/{calendar_id}/events"),
            access_token,
            &parameters,
        )?;
        let token = next_sync_token.ok_or_else(|| {
            CalendarError::new(
                "sync_token_missing",
                "Google returned no Calendar sync token",
            )
        })?;
        Ok((items, token))
    }

    fn list_task_lists(&self, access_token: &str) -> Result<Vec<TaskListEntry>> {
        let parameters = BTreeMap::from([("maxResults".into(), "100".into())]);
        let (items, _) = self.paged_items(
            &format!("{TASKS_API}/users/@me/lists"),
            access_token,
            &parameters,
        )?;
        Ok(items
            .into_iter()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?.to_owned();
                Some(TaskListEntry {
                    id,
                    label: item
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Tasks")
                        .to_owned(),
                    updated: item
                        .get("updated")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                })
            })
            .collect())
    }

    fn list_tasks(&self, access_token: &str, task_list_id: &str) -> Result<Vec<Value>> {
        let parameters = BTreeMap::from([
            ("maxResults".into(), "100".into()),
            ("showCompleted".into(), "false".into()),
            ("showDeleted".into(), "false".into()),
            ("showHidden".into(), "false".into()),
        ]);
        let task_list_id = encode_path_segment(task_list_id);
        let (items, _) = self.paged_items(
            &format!("{TASKS_API}/lists/{task_list_id}/tasks"),
            access_token,
            &parameters,
        )?;
        Ok(items)
    }
}

fn receive_callback(
    listener: &TcpListener,
    timeout: Duration,
) -> Result<Option<BTreeMap<String, String>>> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                return callback_request(&mut stream).map(Some);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                return Err(CalendarError::new(
                    "oauth_callback_failed",
                    "Google sign-in callback failed",
                ));
            }
        }
    }
}

fn callback_request(stream: &mut TcpStream) -> Result<BTreeMap<String, String>> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 1024];
    while raw.len() < MAX_CALLBACK_REQUEST {
        let count = stream.read(&mut chunk).map_err(|_| {
            CalendarError::new(
                "oauth_callback_failed",
                "Could not read Google sign-in callback",
            )
        })?;
        if count == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..count]);
        if raw.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let request = std::str::from_utf8(&raw).map_err(|_| {
        CalendarError::new("oauth_callback_failed", "Invalid Google sign-in callback")
    })?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| {
            CalendarError::new("oauth_callback_failed", "Invalid Google sign-in callback")
        })?;
    let parsed = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| {
        CalendarError::new("oauth_callback_failed", "Invalid Google sign-in callback")
    })?;
    if parsed.path() != "/callback" {
        let body = b"Not found";
        let response = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
        return Err(CalendarError::new(
            "oauth_callback_failed",
            "Invalid Google sign-in callback path",
        ));
    }
    let values = parsed
        .query_pairs()
        .fold(BTreeMap::new(), |mut output, (key, value)| {
            output
                .entry(key.into_owned())
                .or_insert_with(|| value.into_owned());
            output
        });
    let body = b"<!doctype html><meta charset=utf-8><title>Omarchy Calendar</title><p>Google account connected. You may close this tab.</p>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    Ok(values)
}

fn random_base64(bytes: usize) -> String {
    let mut raw = vec![0_u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

fn open_browser(url: &str) -> bool {
    let child = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

fn google_error_message(payload: &Value) -> Option<String> {
    let error = payload.get("error")?;
    match error {
        Value::String(message) => Some(message.clone()),
        Value::Object(object) => object
            .get("message")
            .or_else(|| object.get("status"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn event_parameters(now: DateTime<Utc>, sync_token: Option<&str>) -> BTreeMap<String, String> {
    let mut parameters = BTreeMap::from([
        ("maxResults".into(), "2500".into()),
        ("showDeleted".into(), "true".into()),
        ("singleEvents".into(), "true".into()),
    ]);
    if let Some(token) = sync_token {
        parameters.insert("syncToken".into(), token.to_owned());
    } else {
        parameters.insert(
            "timeMin".into(),
            (now - ChronoDuration::days(90)).to_rfc3339_opts(SecondsFormat::Micros, true),
        );
        parameters.insert(
            "timeMax".into(),
            (now + ChronoDuration::days(548)).to_rfc3339_opts(SecondsFormat::Micros, true),
        );
    }
    parameters
}

fn encode_path_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn path_encoding_matches_google_contract() {
        assert_eq!(
            encode_path_segment("primary@example.com"),
            "primary%40example.com"
        );
        assert_eq!(encode_path_segment("a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn configured_only_checks_for_file() {
        let client = GoogleClient::new(std::path::Path::new(
            "/definitely/missing/google-client.json",
        ));
        assert!(!client.configured());
    }

    #[test]
    fn initial_and_incremental_event_parameters_match_google_contract() {
        let now = DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let initial = event_parameters(now, None);
        assert!(initial.contains_key("timeMin"));
        assert!(initial.contains_key("timeMax"));
        assert!(!initial.contains_key("syncToken"));
        let incremental = event_parameters(now, Some("sync-1"));
        assert_eq!(
            incremental.get("syncToken").map(String::as_str),
            Some("sync-1")
        );
        assert!(!incremental.contains_key("timeMin"));
        assert!(!incremental.contains_key("timeMax"));
    }

    #[test]
    fn revoke_accepts_empty_success_response() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.starts_with("POST /revoke HTTP/1.1"));
            assert!(!request.contains("refresh-token\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        let client = GoogleClient::new("/unused/google-client.json");
        assert!(client.revoke_at(&format!("http://{address}/revoke"), "refresh-token"));
        server.join().unwrap();
    }
}
