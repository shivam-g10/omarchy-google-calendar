use crate::{
    CalendarError, CalendarService, Database, GoogleClient, MAX_MESSAGE_BYTES, Result, SecretStore,
};
use rand::Rng;
use serde_json::{Value, json};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

pub fn default_paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CalendarError::new("home_unavailable", "HOME is not set"))?;
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let state_home = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"));
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| CalendarError::new("runtime_unavailable", "XDG_RUNTIME_DIR is not set"))?;
    Ok((
        config_home.join("omarchy/calendar/google-client.json"),
        state_home.join("omarchy/calendar/agenda.db"),
        runtime.join("omarchy-calendar.sock"),
    ))
}

struct ClientWriter {
    stream: Mutex<UnixStream>,
}

impl ClientWriter {
    fn send(&self, payload: &Value) -> io::Result<()> {
        let mut stream = self.stream.lock().expect("client writer poisoned");
        serde_json::to_writer(&mut *stream, payload)?;
        stream.write_all(b"\n")?;
        stream.flush()
    }
}

struct ClientHub {
    next_id: AtomicU64,
    clients: Mutex<HashMap<u64, Arc<ClientWriter>>>,
}

impl ClientHub {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn register(&self, stream: UnixStream) -> (u64, Arc<ClientWriter>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let writer = Arc::new(ClientWriter {
            stream: Mutex::new(stream),
        });
        self.clients
            .lock()
            .expect("client hub poisoned")
            .insert(id, Arc::clone(&writer));
        (id, writer)
    }

    fn unregister(&self, id: u64) {
        self.clients
            .lock()
            .expect("client hub poisoned")
            .remove(&id);
    }

    fn broadcast(&self, event: &str, data: Value) {
        let payload = json!({"event": event, "data": data});
        let clients = self
            .clients
            .lock()
            .expect("client hub poisoned")
            .iter()
            .map(|(id, client)| (*id, Arc::clone(client)))
            .collect::<Vec<_>>();
        let mut failed = Vec::new();
        for (id, client) in clients {
            if client.send(&payload).is_err() {
                failed.push(id);
            }
        }
        if !failed.is_empty() {
            let mut clients = self.clients.lock().expect("client hub poisoned");
            for id in failed {
                clients.remove(&id);
            }
        }
    }
}

pub struct CalendarServer {
    socket_path: PathBuf,
    listener: UnixListener,
    service: Arc<CalendarService>,
    hub: Arc<ClientHub>,
    stop: Arc<AtomicBool>,
    socket_identity: (u64, u64),
}

impl CalendarServer {
    pub fn bind(socket_path: impl Into<PathBuf>, service: Arc<CalendarService>) -> Result<Self> {
        let socket_path = socket_path.into();
        prepare_socket_path(&socket_path)?;
        let listener = UnixListener::bind(&socket_path).map_err(|error| {
            CalendarError::new(
                "socket_error",
                format!("Could not bind calendar socket: {error}"),
            )
        })?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            CalendarError::new(
                "socket_error",
                format!("Could not protect calendar socket: {error}"),
            )
        })?;
        listener.set_nonblocking(true).map_err(|error| {
            CalendarError::new(
                "socket_error",
                format!("Could not configure calendar socket: {error}"),
            )
        })?;
        let metadata = fs::symlink_metadata(&socket_path).map_err(|error| {
            CalendarError::new(
                "socket_error",
                format!("Could not inspect calendar socket: {error}"),
            )
        })?;
        let hub = Arc::new(ClientHub::new());
        let notifier_hub = Arc::clone(&hub);
        service.set_notifier(Arc::new(move |event, data| {
            notifier_hub.broadcast(event, data);
        }));
        Ok(Self {
            socket_path,
            listener,
            service,
            hub,
            stop: Arc::new(AtomicBool::new(false)),
            socket_identity: (metadata.dev(), metadata.ino()),
        })
    }

    pub fn serve(&self) -> Result<()> {
        while !self.stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let service = Arc::clone(&self.service);
                    let hub = Arc::clone(&self.hub);
                    thread::Builder::new()
                        .name("calendar-client".into())
                        .spawn(move || handle_client(stream, service, hub))
                        .map_err(|error| {
                            CalendarError::new(
                                "socket_error",
                                format!("Could not start calendar client thread: {error}"),
                            )
                        })?;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_error) if self.stop.load(Ordering::Relaxed) => break,
                Err(error) => {
                    return Err(CalendarError::new(
                        "socket_error",
                        format!("Calendar socket failed: {error}"),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    pub fn cleanup(&self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path)
            && (metadata.dev(), metadata.ino()) == self.socket_identity
        {
            let _ = fs::remove_file(&self.socket_path);
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for CalendarServer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn handle_client(stream: UnixStream, service: Arc<CalendarService>, hub: Arc<ClientHub>) {
    let writer_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let (client_id, writer) = hub.register(writer_stream);
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_line_limited(&mut reader) {
            Ok(LineRead::Eof) => break,
            Ok(LineRead::TooLarge) => {
                let error = CalendarError::new("message_too_large", "Request exceeds size limit");
                if writer.send(&error.response(Value::Null)).is_err() {
                    break;
                }
                continue;
            }
            Ok(LineRead::Line(line)) => line,
            Err(_) => break,
        };
        let request_id = request_id_from_line(&line);
        let request_service = Arc::clone(&service);
        let request_writer = Arc::clone(&writer);
        if thread::Builder::new()
            .name("calendar-request".into())
            .spawn(move || {
                let response = dispatch_line(&line, &request_service);
                let _ = request_writer.send(&response);
            })
            .is_err()
        {
            let error = CalendarError::new("internal_error", "Could not start calendar request");
            if writer.send(&error.response(request_id)).is_err() {
                break;
            }
        }
    }
    hub.unregister(client_id);
}

fn request_id_from_line(line: &[u8]) -> Value {
    serde_json::from_slice::<Value>(line)
        .ok()
        .and_then(|request| request.get("id").cloned())
        .unwrap_or(Value::Null)
}

fn dispatch_line(line: &[u8], service: &CalendarService) -> Value {
    let request: Value = match serde_json::from_slice(line) {
        Ok(value) => value,
        Err(_) => {
            return CalendarError::new("invalid_json", "Request is not valid JSON")
                .response(Value::Null);
        }
    };
    let Some(object) = request.as_object() else {
        return CalendarError::new("invalid_request", "Request must be an object")
            .response(Value::Null);
    };
    let request_id = object.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = object
        .get("method")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return CalendarError::new("invalid_request", "method is required").response(request_id);
    };
    let parameters = object.get("params").cloned().unwrap_or_else(|| json!({}));
    if !parameters.is_object() {
        return CalendarError::new("invalid_params", "params must be an object")
            .response(request_id);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        service.dispatch(method, &parameters)
    }));
    match result {
        Ok(Ok(result)) => json!({"id": request_id, "ok": true, "result": result}),
        Ok(Err(error)) => error.response(request_id),
        Err(_) => {
            CalendarError::new("internal_error", "Internal calendar error").response(request_id)
        }
    }
}

enum LineRead {
    Eof,
    Line(Vec<u8>),
    TooLarge,
}

fn read_line_limited(reader: &mut impl BufRead) -> io::Result<LineRead> {
    let mut line = Vec::new();
    let mut too_large = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if line.is_empty() && !too_large {
                Ok(LineRead::Eof)
            } else if too_large || line.len() > MAX_MESSAGE_BYTES {
                Ok(LineRead::TooLarge)
            } else {
                Ok(LineRead::Line(line))
            };
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map(|index| index + 1).unwrap_or(buffer.len());
        if !too_large {
            let allowed = MAX_MESSAGE_BYTES + 1;
            if line.len() + consumed > allowed {
                too_large = true;
                line.clear();
            } else {
                line.extend_from_slice(&buffer[..consumed]);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            if too_large || line.len() > MAX_MESSAGE_BYTES {
                return Ok(LineRead::TooLarge);
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(LineRead::Line(line));
        }
    }
}

fn prepare_socket_path(socket_path: &Path) -> Result<()> {
    let parent = socket_path
        .parent()
        .ok_or_else(|| CalendarError::new("socket_error", "Calendar socket path has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| {
        CalendarError::new(
            "socket_error",
            format!("Could not create calendar socket directory: {error}"),
        )
    })?;
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CalendarError::new(
                "socket_error",
                format!("Could not inspect calendar socket: {error}"),
            ));
        }
    };
    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid || !metadata.file_type().is_socket() {
        return Err(CalendarError::new(
            "unsafe_socket_path",
            "Calendar socket path is not a user-owned socket",
        ));
    }
    if UnixStream::connect(socket_path).is_ok() {
        return Err(CalendarError::new(
            "already_running",
            "Calendar daemon is already running",
        ));
    }
    fs::remove_file(socket_path).map_err(|error| {
        CalendarError::new(
            "socket_error",
            format!("Could not remove stale calendar socket: {error}"),
        )
    })
}

pub fn build_service(
    client_path: Option<PathBuf>,
    database_path: Option<PathBuf>,
) -> Result<(Arc<CalendarService>, PathBuf)> {
    let (default_client, default_database, socket_path) = default_paths()?;
    let google = Arc::new(GoogleClient::new(client_path.unwrap_or(default_client)));
    let database = Arc::new(Database::new(database_path.unwrap_or(default_database))?);
    let service = Arc::new(CalendarService::new(
        database,
        google,
        Arc::new(SecretStore::new()),
    ));
    Ok((service, socket_path))
}

pub fn run_daemon() -> Result<i32> {
    unsafe {
        libc::umask(0o077);
    }
    let (service, socket_path) = build_service(None, None)?;
    let server = Arc::new(CalendarServer::bind(socket_path, Arc::clone(&service))?);
    signal_hook::flag::register(SIGTERM, server.stop_handle()).map_err(|error| {
        CalendarError::new(
            "signal_error",
            format!("Could not register SIGTERM handler: {error}"),
        )
    })?;
    signal_hook::flag::register(SIGINT, server.stop_handle()).map_err(|error| {
        CalendarError::new(
            "signal_error",
            format!("Could not register SIGINT handler: {error}"),
        )
    })?;
    let poll_control = Arc::new((Mutex::new(false), Condvar::new()));
    let (poll_done_tx, poll_done_rx) = mpsc::channel();
    let poll_thread = {
        let service = Arc::clone(&service);
        let control = Arc::clone(&poll_control);
        thread::Builder::new()
            .name("calendar-poll".into())
            .spawn(move || {
                poll(service, control);
                let _ = poll_done_tx.send(());
            })
            .map_err(|error| {
                CalendarError::new(
                    "internal_error",
                    format!("Could not start calendar poll thread: {error}"),
                )
            })?
    };
    let result = server.serve();
    {
        let (lock, condition) = &*poll_control;
        *lock.lock().expect("poll control poisoned") = true;
        condition.notify_all();
    }
    if poll_done_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
        let _ = poll_thread.join();
    }
    server.cleanup();
    result?;
    Ok(0)
}

fn poll(service: Arc<CalendarService>, control: Arc<(Mutex<bool>, Condvar)>) {
    if wait_or_stop(&control, Duration::from_secs(2)) {
        return;
    }
    loop {
        if service
            .database()
            .configured_accounts()
            .map(|accounts| !accounts.is_empty())
            .unwrap_or(false)
        {
            let _ = service.refresh(&json!({}));
        }
        let interval = rand::thread_rng().gen_range(270..=330);
        if wait_or_stop(&control, Duration::from_secs(interval)) {
            return;
        }
    }
}

fn wait_or_stop(control: &Arc<(Mutex<bool>, Condvar)>, timeout: Duration) -> bool {
    let (lock, condition) = &**control;
    let stopped = lock.lock().expect("poll control poisoned");
    if *stopped {
        return true;
    }
    let (stopped, _) = condition
        .wait_timeout(stopped, timeout)
        .expect("poll condition poisoned");
    *stopped
}

pub fn send_request(socket_path: &Path, method: &str, parameters: Value) -> Result<Value> {
    let request_id = Uuid::new_v4().to_string();
    let request = json!({"id": request_id, "method": method, "params": parameters});
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CalendarError::new("daemon_unavailable", "Calendar daemon is not running")
        } else {
            CalendarError::new("daemon_unavailable", "Could not connect to calendar daemon")
        }
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(310)))
        .map_err(|_| {
            CalendarError::new("daemon_unavailable", "Could not configure daemon client")
        })?;
    stream
        .set_write_timeout(Some(Duration::from_secs(310)))
        .map_err(|_| {
            CalendarError::new("daemon_unavailable", "Could not configure daemon client")
        })?;
    serde_json::to_writer(&mut stream, &request)
        .map_err(|_| CalendarError::new("daemon_unavailable", "Could not send calendar request"))?;
    stream
        .write_all(b"\n")
        .map_err(|_| CalendarError::new("daemon_unavailable", "Could not send calendar request"))?;
    stream
        .flush()
        .map_err(|_| CalendarError::new("daemon_unavailable", "Could not send calendar request"))?;
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_line_limited(&mut reader).map_err(|_| {
            CalendarError::new("daemon_disconnected", "Calendar daemon disconnected")
        })? {
            LineRead::Eof => {
                return Err(CalendarError::new(
                    "daemon_disconnected",
                    "Calendar daemon disconnected",
                ));
            }
            LineRead::TooLarge => {
                return Err(CalendarError::new(
                    "message_too_large",
                    "Calendar daemon response exceeds size limit",
                ));
            }
            LineRead::Line(line) => line,
        };
        let response: Value = serde_json::from_slice(&line).map_err(|_| {
            CalendarError::new("invalid_json", "Calendar daemon returned invalid JSON")
        })?;
        if response.get("id") != Some(&Value::String(request_id.clone())) {
            continue;
        }
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            let error = response.get("error").and_then(Value::as_object);
            return Err(CalendarError::new(
                error
                    .and_then(|value| value.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("daemon_error"),
                error
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Calendar daemon failed"),
            ));
        }
        return Ok(response.get("result").cloned().unwrap_or(Value::Null));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_reader_rejects_oversized_messages() {
        let bytes = vec![b'a'; MAX_MESSAGE_BYTES + 2];
        let mut reader = BufReader::new(bytes.as_slice());
        assert!(matches!(
            read_line_limited(&mut reader).unwrap(),
            LineRead::TooLarge
        ));
    }
}
