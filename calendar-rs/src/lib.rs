mod database;
mod google;
mod protocol;
mod service;

pub use database::Database;
pub use google::{GoogleApi, GoogleClient};
pub use protocol::{CalendarServer, default_paths, run_daemon, send_request};
pub use service::{CalendarService, SecretStore, SecretStoreApi};

use serde_json::Value;
use std::fmt;

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

pub type Result<T> = std::result::Result<T, CalendarError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthResult {
    pub refresh_token: String,
    pub access_token: String,
    pub subject: String,
    pub email: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarEntry {
    pub id: String,
    pub label: String,
    pub color: String,
    pub selected: bool,
    pub primary: bool,
    pub default_reminders: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskListEntry {
    pub id: String,
    pub label: String,
    pub updated: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarError {
    pub code: String,
    pub message: String,
    pub http_status: Option<u16>,
}

impl CalendarError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            http_status: None,
        }
    }

    pub fn google_http(status: u16, message: impl Into<String>) -> Self {
        Self {
            code: format!("google_http_{status}"),
            message: message.into(),
            http_status: Some(status),
        }
    }

    pub fn response(&self, request_id: Value) -> Value {
        serde_json::json!({
            "id": request_id,
            "ok": false,
            "error": {"code": self.code, "message": self.message}
        })
    }
}

impl fmt::Display for CalendarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CalendarError {}

pub(crate) fn safe_message(value: impl AsRef<str>, fallback: &str) -> String {
    let collapsed = value
        .as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let source = if collapsed.is_empty() {
        fallback
    } else {
        &collapsed
    };
    source.chars().take(300).collect()
}
