use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const BRAVE_EXECUTABLE: &str = "/opt/brave-bin/brave";
const CHROMIUM_SINGLETON_LIMIT: usize = 32 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_BROWSER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserOpenResult {
    Opened,
    BraveHandoffFailed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DefaultBrowser {
    Brave,
    Other,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BraveTarget {
    pid: libc::pid_t,
    uid: libc::uid_t,
    executable: PathBuf,
    socket_inode: u64,
    socket_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BraveDiscovery {
    None,
    Target(BraveTarget),
    Unsafe,
}

#[derive(Clone, Debug)]
struct ProcLayout {
    root: PathBuf,
    net_unix: PathBuf,
}

impl ProcLayout {
    fn system() -> Self {
        Self {
            root: PathBuf::from("/proc"),
            net_unix: PathBuf::from("/proc/net/unix"),
        }
    }
}

trait BrowserBackend {
    fn default_browser(&self) -> DefaultBrowser;
    fn discover_brave(&self) -> BraveDiscovery;
    fn handoff(&self, target: &BraveTarget, url: &str) -> bool;
    fn launch_default(&self, url: &str) -> bool;
}

struct SystemBrowserBackend {
    layout: ProcLayout,
    uid: libc::uid_t,
}

impl SystemBrowserBackend {
    fn new() -> Self {
        Self {
            layout: ProcLayout::system(),
            // SAFETY: geteuid has no preconditions and does not dereference memory.
            uid: unsafe { libc::geteuid() },
        }
    }
}

impl BrowserBackend for SystemBrowserBackend {
    fn default_browser(&self) -> DefaultBrowser {
        query_default_browser()
    }

    fn discover_brave(&self) -> BraveDiscovery {
        discover_brave(&self.layout, self.uid, Path::new(BRAVE_EXECUTABLE))
    }

    fn handoff(&self, target: &BraveTarget, url: &str) -> bool {
        notify_existing_brave(&self.layout, target, url)
    }

    fn launch_default(&self, url: &str) -> bool {
        launch_default_browser(url)
    }
}

pub(crate) fn open_browser(url: &str) -> BrowserOpenResult {
    open_with_backend(url, &SystemBrowserBackend::new())
}

fn open_with_backend(url: &str, backend: &impl BrowserBackend) -> BrowserOpenResult {
    if !is_safe_browser_url(url) {
        return BrowserOpenResult::Failed;
    }
    match backend.default_browser() {
        DefaultBrowser::Other | DefaultBrowser::Unknown => opened(backend.launch_default(url)),
        DefaultBrowser::Brave => match backend.discover_brave() {
            BraveDiscovery::None => opened(backend.launch_default(url)),
            BraveDiscovery::Target(target) => {
                if backend.handoff(&target, url) {
                    BrowserOpenResult::Opened
                } else {
                    BrowserOpenResult::BraveHandoffFailed
                }
            }
            BraveDiscovery::Unsafe => BrowserOpenResult::BraveHandoffFailed,
        },
    }
}

fn opened(success: bool) -> BrowserOpenResult {
    if success {
        BrowserOpenResult::Opened
    } else {
        BrowserOpenResult::Failed
    }
}

fn is_safe_browser_url(url: &str) -> bool {
    if url.as_bytes().contains(&0) {
        return false;
    }
    Url::parse(url)
        .map(|parsed| parsed.scheme() == "https" && parsed.host_str().is_some())
        .unwrap_or(false)
}

fn query_default_browser() -> DefaultBrowser {
    let mut child = match Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return DefaultBrowser::Unknown,
    };
    let deadline = Instant::now() + DEFAULT_BROWSER_TIMEOUT;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return DefaultBrowser::Unknown;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return DefaultBrowser::Unknown;
            }
        }
    };
    if !success {
        return DefaultBrowser::Unknown;
    }
    let mut value = String::new();
    if child
        .stdout
        .take()
        .is_none_or(|mut stdout| stdout.read_to_string(&mut value).is_err())
    {
        return DefaultBrowser::Unknown;
    }
    classify_default_browser(&value)
}

fn classify_default_browser(value: &str) -> DefaultBrowser {
    match value.trim() {
        "brave-browser.desktop" => DefaultBrowser::Brave,
        "" => DefaultBrowser::Unknown,
        _ => DefaultBrowser::Other,
    }
}

fn discover_brave(layout: &ProcLayout, uid: libc::uid_t, executable: &Path) -> BraveDiscovery {
    let entries = match fs::read_dir(&layout.root) {
        Ok(entries) => entries,
        Err(_) => return discovery_failed("read_proc"),
    };
    let mut brave_processes = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return discovery_failed("read_proc_entry"),
        };
        let Some(pid) = parse_pid(&entry.file_name()) else {
            continue;
        };
        match inspect_brave_executable(layout, pid, uid, executable) {
            ProcessInspection::Brave => brave_processes.push(pid),
            ProcessInspection::NotBrave => {}
            ProcessInspection::Unsafe => return discovery_failed("inspect_process"),
        }
    }
    if brave_processes.is_empty() {
        return BraveDiscovery::None;
    }
    let socket_entries = match read_listening_singleton_sockets(&layout.net_unix) {
        Ok(entries) => entries,
        Err(_) => return discovery_failed("read_unix_sockets"),
    };
    let mut targets = Vec::new();
    for pid in brave_processes {
        let ownership = derive_socket_target(layout, pid, uid, executable, &socket_entries);
        let role = match inspect_process_role(layout, pid, executable) {
            ProcessRoleInspection::Role(role) => role,
            ProcessRoleInspection::Gone => continue,
            ProcessRoleInspection::Unsafe => return discovery_failed("inspect_process_role"),
        };
        match (role, ownership) {
            (ProcessRole::DefaultMain, SocketOwnership::Target(target)) => targets.push(target),
            (ProcessRole::DefaultMain, _) => return discovery_failed("main_socket_ownership"),
            (ProcessRole::Child, SocketOwnership::Target(_)) => {
                return discovery_failed("child_owns_singleton");
            }
            (ProcessRole::Child, SocketOwnership::None)
            | (ProcessRole::CustomProfile, SocketOwnership::None)
            | (ProcessRole::CustomProfile, SocketOwnership::Target(_)) => {}
            (_, SocketOwnership::Unsafe) => return discovery_failed("socket_ownership"),
        }
    }
    match targets.as_slice() {
        [] => BraveDiscovery::None,
        [target] => BraveDiscovery::Target(target.clone()),
        _ => discovery_failed("multiple_main_targets"),
    }
}

fn discovery_failed(stage: &str) -> BraveDiscovery {
    eprintln!("omarchy-calendar: Brave discovery failed at {stage}");
    BraveDiscovery::Unsafe
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessInspection {
    Brave,
    NotBrave,
    Unsafe,
}

fn inspect_brave_executable(
    layout: &ProcLayout,
    pid: libc::pid_t,
    uid: libc::uid_t,
    executable: &Path,
) -> ProcessInspection {
    let process_dir = layout.root.join(pid.to_string());
    let metadata = match fs::metadata(&process_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProcessInspection::NotBrave;
        }
        Err(_) => return process_inspection_failed(pid, "metadata"),
    };
    if metadata.uid() != uid {
        return ProcessInspection::NotBrave;
    }
    let status = match fs::read_to_string(process_dir.join("status")) {
        Ok(status) => status,
        Err(_) => return ProcessInspection::NotBrave,
    };
    if process_name(&status) != Some("brave") {
        return ProcessInspection::NotBrave;
    }
    match process_state(&status) {
        Some('Z') => return ProcessInspection::NotBrave,
        Some(_) => {}
        None => return process_inspection_failed(pid, "state"),
    }
    if process_effective_uid(&status) != Some(uid) {
        return process_inspection_failed(pid, "effective_uid");
    }
    let actual_executable = match fs::read_link(process_dir.join("exe")) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProcessInspection::NotBrave;
        }
        Err(_) => return process_inspection_failed(pid, "executable_unreadable"),
    };
    if actual_executable != executable {
        return process_inspection_failed(pid, "executable_mismatch");
    }
    ProcessInspection::Brave
}

fn process_inspection_failed(pid: libc::pid_t, reason: &str) -> ProcessInspection {
    eprintln!("omarchy-calendar: rejected process {pid} during Brave discovery: {reason}");
    ProcessInspection::Unsafe
}

fn parse_pid(value: &OsStr) -> Option<libc::pid_t> {
    let text = value.to_str()?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn process_effective_uid(status: &str) -> Option<libc::uid_t> {
    let line = status.lines().find(|line| line.starts_with("Uid:"))?;
    line.split_whitespace().nth(2)?.parse().ok()
}

fn process_name(status: &str) -> Option<&str> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Name:"))
        .map(str::trim)
}

fn process_state(status: &str) -> Option<char> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .and_then(|value| value.trim_start().chars().next())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessRole {
    DefaultMain,
    Child,
    CustomProfile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessRoleInspection {
    Role(ProcessRole),
    Gone,
    Unsafe,
}

fn inspect_process_role(
    layout: &ProcLayout,
    pid: libc::pid_t,
    executable: &Path,
) -> ProcessRoleInspection {
    let process_dir = layout.root.join(pid.to_string());
    let command_line = match fs::read(process_dir.join("cmdline")) {
        Ok(command_line) => command_line,
        Err(_) if !process_dir.exists() => return ProcessRoleInspection::Gone,
        Err(_) => return ProcessRoleInspection::Unsafe,
    };
    let custom = command_line_contains_switch(&command_line, executable, b"--user-data-dir");
    let child = command_line_contains_switch(&command_line, executable, b"--type");
    match (custom, child) {
        (Some(true), _) => ProcessRoleInspection::Role(ProcessRole::CustomProfile),
        (Some(false), Some(true)) => ProcessRoleInspection::Role(ProcessRole::Child),
        (Some(false), Some(false)) => ProcessRoleInspection::Role(ProcessRole::DefaultMain),
        _ => ProcessRoleInspection::Unsafe,
    }
}

fn command_line_contains_switch(
    command_line: &[u8],
    executable: &Path,
    switch: &[u8],
) -> Option<bool> {
    let mut components = command_line
        .split(|byte| *byte == 0)
        .filter(|component| !component.is_empty());
    let first = components.next()?;
    let executable = executable.as_os_str().as_bytes();
    if first == executable {
        // Normal /proc cmdline: each remaining component is one argv value.
    } else {
        let tail = first.strip_prefix(executable)?;
        if tail.first().is_none_or(|byte| !byte.is_ascii_whitespace()) {
            return None;
        }
        if rewritten_title_contains_switch(tail, switch) {
            return Some(true);
        }
    }
    Some(components.any(|component| argument_is_switch(component, switch)))
}

fn argument_is_switch(argument: &[u8], switch: &[u8]) -> bool {
    argument
        .strip_prefix(switch)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(b"="))
}

fn rewritten_title_contains_switch(title: &[u8], switch: &[u8]) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut token_start = true;
    let mut index = 0;
    while index < title.len() {
        let byte = title[index];
        if escaped {
            escaped = false;
            token_start = false;
            index += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            token_start = false;
            index += 1;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            token_start = false;
            index += 1;
            continue;
        }
        if byte.is_ascii_whitespace() {
            token_start = true;
            index += 1;
            continue;
        }
        if token_start && title[index..].starts_with(switch) {
            let suffix = &title[index + switch.len()..];
            if suffix
                .first()
                .is_none_or(|next| *next == b'=' || next.is_ascii_whitespace())
            {
                return true;
            }
        }
        token_start = false;
        index += 1;
    }
    false
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SocketOwnership {
    None,
    Target(BraveTarget),
    Unsafe,
}

fn derive_socket_target(
    layout: &ProcLayout,
    pid: libc::pid_t,
    uid: libc::uid_t,
    executable: &Path,
    entries: &[UnixSocketEntry],
) -> SocketOwnership {
    let Some(inodes) = process_socket_inodes(layout, pid) else {
        return if layout.root.join(pid.to_string()).exists() {
            SocketOwnership::Unsafe
        } else {
            SocketOwnership::None
        };
    };
    let mut matches = entries
        .iter()
        .filter(|entry| inodes.contains(&entry.inode))
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.path.cmp(&right.path));
    matches.dedup();
    let entry = match matches.as_slice() {
        [] => return SocketOwnership::None,
        [entry] => entry,
        _ => return SocketOwnership::Unsafe,
    };
    let target = BraveTarget {
        pid,
        uid,
        executable: executable.to_owned(),
        socket_inode: entry.inode,
        socket_path: entry.path.clone(),
    };
    if validate_socket_binding(layout, &target) {
        SocketOwnership::Target(target)
    } else {
        SocketOwnership::Unsafe
    }
}

fn process_socket_inodes(layout: &ProcLayout, pid: libc::pid_t) -> Option<HashSet<u64>> {
    let mut inodes = HashSet::new();
    for entry in fs::read_dir(layout.root.join(pid.to_string()).join("fd"))
        .ok()?
        .flatten()
    {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if let Some(inode) = parse_socket_inode(&target) {
            inodes.insert(inode);
        }
    }
    Some(inodes)
}

fn parse_socket_inode(target: &Path) -> Option<u64> {
    let value = target.as_os_str().as_bytes();
    let number = value.strip_prefix(b"socket:[")?.strip_suffix(b"]")?;
    std::str::from_utf8(number).ok()?.parse().ok()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct UnixSocketEntry {
    inode: u64,
    path: PathBuf,
}

fn read_listening_singleton_sockets(path: &Path) -> std::io::Result<Vec<UnixSocketEntry>> {
    let content = fs::read(path)?;
    Ok(parse_listening_singleton_sockets(&content))
}

fn parse_listening_singleton_sockets(content: &[u8]) -> Vec<UnixSocketEntry> {
    content
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let (fields, path) = split_unix_socket_line(line)?;
            let flags = parse_ascii_radix(fields[3], 16)? as u32;
            let socket_type = parse_ascii_radix(fields[4], 16)? as u32;
            if flags & 0x0001_0000 == 0
                || socket_type != libc::SOCK_STREAM as u32
                || fields[5] != b"01"
            {
                return None;
            }
            let inode = parse_ascii_radix(fields[6], 10)?;
            let path = PathBuf::from(OsStr::from_bytes(path));
            if !path.is_absolute() || path.file_name()? != "SingletonSocket" {
                return None;
            }
            Some(UnixSocketEntry { inode, path })
        })
        .collect()
}

fn split_unix_socket_line(line: &[u8]) -> Option<([&[u8]; 7], &[u8])> {
    let mut offset = 0;
    let mut fields = [b"".as_slice(); 7];
    for field in &mut fields {
        while line.get(offset).is_some_and(u8::is_ascii_whitespace) {
            offset += 1;
        }
        let start = offset;
        while line
            .get(offset)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            offset += 1;
        }
        if start == offset {
            return None;
        }
        *field = &line[start..offset];
    }
    while line.get(offset).is_some_and(u8::is_ascii_whitespace) {
        offset += 1;
    }
    (offset < line.len()).then_some((fields, &line[offset..]))
}

fn parse_ascii_radix(value: &[u8], radix: u32) -> Option<u64> {
    std::str::from_utf8(value).ok().and_then(|value| {
        if radix == 10 {
            value.parse().ok()
        } else {
            u64::from_str_radix(value, radix).ok()
        }
    })
}

fn validate_socket_target(layout: &ProcLayout, target: &BraveTarget) -> bool {
    if inspect_brave_executable(layout, target.pid, target.uid, &target.executable)
        != ProcessInspection::Brave
        || inspect_process_role(layout, target.pid, &target.executable)
            != ProcessRoleInspection::Role(ProcessRole::DefaultMain)
    {
        return false;
    }
    validate_socket_binding(layout, target)
}

fn validate_socket_binding(layout: &ProcLayout, target: &BraveTarget) -> bool {
    let Some(inodes) = process_socket_inodes(layout, target.pid) else {
        return false;
    };
    if !inodes.contains(&target.socket_inode) {
        return false;
    }
    let Ok(entries) = read_listening_singleton_sockets(&layout.net_unix) else {
        return false;
    };
    if entries
        .iter()
        .filter(|entry| entry.inode == target.socket_inode && entry.path == target.socket_path)
        .count()
        != 1
    {
        return false;
    }
    validate_socket_path(&target.socket_path, target.uid)
}

fn validate_socket_path(path: &Path, uid: libc::uid_t) -> bool {
    if !path.is_absolute() || path.file_name() != Some(OsStr::new("SingletonSocket")) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(parent_metadata) = fs::symlink_metadata(parent) else {
        return false;
    };
    if !parent_metadata.is_dir()
        || parent_metadata.uid() != uid
        || parent_metadata.mode() & 0o7777 != 0o700
    {
        return false;
    }
    let Ok(socket_metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    socket_metadata.file_type().is_socket() && socket_metadata.uid() == uid
}

fn notify_existing_brave(layout: &ProcLayout, target: &BraveTarget, url: &str) -> bool {
    let Some(message) = build_singleton_message(Path::new("/"), &target.executable, url) else {
        return handoff_failed("build_message");
    };
    if !validate_socket_target(layout, target) {
        return handoff_failed("validate_before_connect");
    }
    let mut stream = match connect_unix_with_timeout(&target.socket_path, CONNECT_TIMEOUT) {
        Ok(stream) => stream,
        Err(_) => return handoff_failed("connect"),
    };
    if !peer_matches(&stream, target.pid, target.uid) {
        return handoff_failed("peer_credentials");
    }
    if !validate_socket_target(layout, target) {
        return handoff_failed("validate_after_connect");
    }
    if stream.set_read_timeout(Some(ACK_TIMEOUT)).is_err() {
        return handoff_failed("set_read_timeout");
    }
    if stream.set_write_timeout(Some(CONNECT_TIMEOUT)).is_err() {
        return handoff_failed("set_write_timeout");
    }
    if stream.write_all(&message).is_err() {
        return handoff_failed("write_message");
    }
    if stream.shutdown(std::net::Shutdown::Write).is_err() {
        return handoff_failed("shutdown_write");
    }
    let mut response = Vec::new();
    if stream.take(9).read_to_end(&mut response).is_err() {
        return handoff_failed("read_response");
    }
    if response != b"ACK" {
        return handoff_failed("unexpected_response");
    }
    true
}

fn handoff_failed(stage: &str) -> bool {
    eprintln!("omarchy-calendar: Brave handoff failed at {stage}");
    false
}

fn connect_unix_with_timeout(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let path = path.as_os_str().as_bytes();
    if path.is_empty() || path.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid Unix socket path",
        ));
    }
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix socket path is too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(path) {
        *destination = *source as libc::c_char;
    }
    // SAFETY: socket receives valid constants and returns a new owned descriptor.
    let raw_fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: raw_fd was returned by socket above and ownership transfers here once.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let address_length =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + path.len() + 1) as libc::socklen_t;
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: address is initialized for AF_UNIX, address_length covers the path,
        // and descriptor remains owned for the duration of the call.
        let connected = unsafe {
            libc::connect(
                descriptor.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                address_length,
            )
        };
        if connected == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(code) if code == libc::EINPROGRESS || code == libc::EALREADY => {
                wait_for_socket_connect(descriptor.as_raw_fd(), deadline)?;
                break;
            }
            Some(libc::EISCONN) => break,
            Some(libc::EINTR) if Instant::now() < deadline => continue,
            // Linux uses EAGAIN when a nonblocking Unix listener's backlog is full.
            // No connection was queued, so retry instead of treating POLLOUT as success.
            Some(libc::EAGAIN) if Instant::now() < deadline => {
                thread::sleep(
                    Duration::from_millis(10)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Some(code) if code == libc::EINTR || code == libc::EAGAIN => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Unix socket connect timed out",
                ));
            }
            _ => return Err(error),
        }
    }
    let stream = UnixStream::from(descriptor);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn wait_for_socket_connect(fd: std::os::fd::RawFd, deadline: Instant) -> std::io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Unix socket connect timed out",
            ));
        }
        let timeout_ms = remaining.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: descriptor points to one initialized pollfd for the duration of poll.
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Unix socket connect timed out",
            ));
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if descriptor.revents & (libc::POLLOUT | libc::POLLERR | libc::POLLHUP) == 0 {
            return Err(std::io::Error::other("Unix socket connect failed"));
        }
        let mut socket_error = 0;
        let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: socket_error and length are valid output storage for SO_ERROR.
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut socket_error as *mut libc::c_int).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if socket_error != 0 {
            return Err(std::io::Error::from_raw_os_error(socket_error));
        }
        return Ok(());
    }
}

fn build_singleton_message(cwd: &Path, executable: &Path, url: &str) -> Option<Vec<u8>> {
    if !is_safe_browser_url(url) {
        return None;
    }
    let cwd = cwd.as_os_str().as_bytes();
    let executable = executable.as_os_str().as_bytes();
    if cwd.contains(&0) || executable.contains(&0) {
        return None;
    }
    let mut message =
        Vec::with_capacity(START_PREFIX.len() + cwd.len() + executable.len() + url.len());
    message.extend_from_slice(START_PREFIX);
    message.extend_from_slice(cwd);
    message.push(0);
    message.extend_from_slice(executable);
    message.push(0);
    message.extend_from_slice(url.as_bytes());
    (message.len() < CHROMIUM_SINGLETON_LIMIT).then_some(message)
}

const START_PREFIX: &[u8] = b"START\0";

fn peer_matches(stream: &UnixStream, pid: libc::pid_t, uid: libc::uid_t) -> bool {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the socket descriptor is valid for this call, and the output buffer
    // and length point to initialized, correctly sized storage for libc::ucred.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    result == 0
        && length as usize == std::mem::size_of::<libc::ucred>()
        && credentials.pid == pid
        && credentials.uid == uid
}

fn launch_default_browser(url: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::env;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;
    use std::process::Stdio;
    use std::sync::mpsc;

    struct FakeBackend {
        default: DefaultBrowser,
        discovery: BraveDiscovery,
        handoff_success: bool,
        launch_success: bool,
        discoveries: Cell<usize>,
        handoffs: Cell<usize>,
        launches: Cell<usize>,
    }

    impl BrowserBackend for FakeBackend {
        fn default_browser(&self) -> DefaultBrowser {
            self.default
        }

        fn discover_brave(&self) -> BraveDiscovery {
            self.discoveries.set(self.discoveries.get() + 1);
            self.discovery.clone()
        }

        fn handoff(&self, _target: &BraveTarget, _url: &str) -> bool {
            self.handoffs.set(self.handoffs.get() + 1);
            self.handoff_success
        }

        fn launch_default(&self, _url: &str) -> bool {
            self.launches.set(self.launches.get() + 1);
            self.launch_success
        }
    }

    fn fake_target() -> BraveTarget {
        BraveTarget {
            pid: 42,
            uid: 1000,
            executable: PathBuf::from(BRAVE_EXECUTABLE),
            socket_inode: 99,
            socket_path: PathBuf::from("/tmp/fake/SingletonSocket"),
        }
    }

    fn fake_backend(discovery: BraveDiscovery) -> FakeBackend {
        FakeBackend {
            default: DefaultBrowser::Brave,
            discovery,
            handoff_success: false,
            launch_success: true,
            discoveries: Cell::new(0),
            handoffs: Cell::new(0),
            launches: Cell::new(0),
        }
    }

    #[test]
    fn only_native_brave_desktop_id_uses_direct_handoff() {
        assert_eq!(
            classify_default_browser("brave-browser.desktop\n"),
            DefaultBrowser::Brave
        );
        assert_eq!(
            classify_default_browser("com.brave.Browser.desktop"),
            DefaultBrowser::Other
        );
        assert_eq!(
            classify_default_browser("brave-browser-beta.desktop"),
            DefaultBrowser::Other
        );
        assert_eq!(
            classify_default_browser("Brave-Browser.desktop"),
            DefaultBrowser::Other
        );
        assert_eq!(classify_default_browser(""), DefaultBrowser::Unknown);
    }

    #[test]
    fn non_native_browser_ids_use_default_launcher() {
        for desktop_id in [
            "firefox.desktop",
            "google-chrome.desktop",
            "chromium.desktop",
            "com.brave.Browser.desktop",
        ] {
            let backend = FakeBackend {
                default: classify_default_browser(desktop_id),
                ..fake_backend(BraveDiscovery::Unsafe)
            };
            assert_eq!(
                open_with_backend("https://accounts.google.com/authorize", &backend),
                BrowserOpenResult::Opened,
                "{desktop_id}"
            );
            assert_eq!(backend.discoveries.get(), 0, "{desktop_id}");
            assert_eq!(backend.handoffs.get(), 0, "{desktop_id}");
            assert_eq!(backend.launches.get(), 1, "{desktop_id}");
        }
    }

    #[test]
    fn brave_handoff_failure_never_launches_fallback() {
        let backend = fake_backend(BraveDiscovery::Target(fake_target()));
        assert_eq!(
            open_with_backend("https://accounts.google.com/authorize", &backend),
            BrowserOpenResult::BraveHandoffFailed
        );
        assert_eq!(backend.handoffs.get(), 1);
        assert_eq!(backend.launches.get(), 0);

        let unsafe_backend = fake_backend(BraveDiscovery::Unsafe);
        assert_eq!(
            open_with_backend("https://accounts.google.com/authorize", &unsafe_backend),
            BrowserOpenResult::BraveHandoffFailed
        );
        assert_eq!(unsafe_backend.handoffs.get(), 0);
        assert_eq!(unsafe_backend.launches.get(), 0);
    }

    #[test]
    fn missing_brave_or_other_default_uses_fallback() {
        let no_brave = fake_backend(BraveDiscovery::None);
        assert_eq!(
            open_with_backend("https://example.com", &no_brave),
            BrowserOpenResult::Opened
        );
        assert_eq!(no_brave.launches.get(), 1);

        let other = FakeBackend {
            default: DefaultBrowser::Other,
            ..fake_backend(BraveDiscovery::Unsafe)
        };
        assert_eq!(
            open_with_backend("https://example.com", &other),
            BrowserOpenResult::Opened
        );
        assert_eq!(other.launches.get(), 1);
        assert_eq!(other.handoffs.get(), 0);
    }

    #[test]
    fn unknown_default_uses_fallback() {
        let backend = FakeBackend {
            default: DefaultBrowser::Unknown,
            ..fake_backend(BraveDiscovery::Unsafe)
        };
        assert_eq!(
            open_with_backend("https://accounts.google.com/authorize", &backend),
            BrowserOpenResult::Opened
        );
        assert_eq!(backend.discoveries.get(), 0);
        assert_eq!(backend.launches.get(), 1);
        assert_eq!(backend.handoffs.get(), 0);
    }

    #[test]
    fn unsafe_urls_are_never_opened() {
        let backend = fake_backend(BraveDiscovery::None);
        assert_eq!(
            open_with_backend("javascript:alert(1)", &backend),
            BrowserOpenResult::Failed
        );
        assert_eq!(backend.launches.get(), 0);
        assert_eq!(backend.handoffs.get(), 0);
    }

    #[test]
    fn chromium_framing_is_exact_and_ack_is_required() {
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = temporary.path().join("SingletonSocket");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let inode = socket_inode_for_fd(listener.as_raw_fd());
        let pid = std::process::id() as libc::pid_t;
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        let layout = create_fake_layout(temporary.path(), pid, uid, inode, &socket_path);
        let target = BraveTarget {
            pid,
            uid,
            executable: PathBuf::from(BRAVE_EXECUTABLE),
            socket_inode: inode,
            socket_path: socket_path.clone(),
        };
        let (message_sender, message_receiver) = mpsc::sync_channel(1);
        let server = thread::spawn(move || {
            let Some(mut stream) = accept_with_timeout(&listener, Duration::from_secs(2)) else {
                return;
            };
            let mut message = Vec::new();
            stream.read_to_end(&mut message).unwrap();
            message_sender.send(message).unwrap();
            stream.write_all(b"ACK").unwrap();
        });

        let url = "https://accounts.google.com/o/oauth2/v2/auth?state=opaque";
        let opened = notify_existing_brave(&layout, &target, url);
        server.join().unwrap();
        assert!(opened);
        assert_eq!(
            message_receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            [
                b"START\0/\0".as_slice(),
                BRAVE_EXECUTABLE.as_bytes(),
                b"\0",
                url.as_bytes(),
            ]
            .concat()
        );
    }

    #[test]
    fn shutdown_and_bad_ack_are_rejected() {
        for reply in [b"SHUTDOWN".as_slice(), b"NOPE".as_slice()] {
            let temporary = tempfile::tempdir().unwrap();
            fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let socket_path = temporary.path().join("SingletonSocket");
            let listener = UnixListener::bind(&socket_path).unwrap();
            let inode = socket_inode_for_fd(listener.as_raw_fd());
            let pid = std::process::id() as libc::pid_t;
            // SAFETY: geteuid has no preconditions and does not dereference memory.
            let uid = unsafe { libc::geteuid() };
            let layout = create_fake_layout(temporary.path(), pid, uid, inode, &socket_path);
            let target = BraveTarget {
                pid,
                uid,
                executable: PathBuf::from(BRAVE_EXECUTABLE),
                socket_inode: inode,
                socket_path: socket_path.clone(),
            };
            let reply = reply.to_vec();
            let server = thread::spawn(move || {
                let Some(mut stream) = accept_with_timeout(&listener, Duration::from_secs(2))
                else {
                    return;
                };
                let mut message = Vec::new();
                stream.read_to_end(&mut message).unwrap();
                stream.write_all(&reply).unwrap();
            });
            let opened =
                notify_existing_brave(&layout, &target, "https://accounts.google.com/authorize");
            server.join().unwrap();
            assert!(!opened);
        }
    }

    #[test]
    fn discovery_requires_one_default_profile_main_process() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        let socket_directory = temporary.path().join("chromium");
        fs::create_dir(&socket_directory).unwrap();
        fs::set_permissions(&socket_directory, fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = socket_directory.join("SingletonSocket");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let inode = socket_inode_for_fd(listener.as_raw_fd());
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        create_fake_brave_process(&proc_root, 4242, uid, inode, false);
        let net_unix = temporary.path().join("unix");
        fs::write(
            &net_unix,
            format!(
                "Num RefCount Protocol Flags Type St Inode Path\n0000: 2 0 00010000 0001 01 {inode} {}\n",
                socket_path.display()
            ),
        )
        .unwrap();
        let layout = ProcLayout {
            root: proc_root.clone(),
            net_unix,
        };

        let BraveDiscovery::Target(target) =
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE))
        else {
            panic!("expected one safe Brave target");
        };
        assert_eq!(target.pid, 4242);
        assert_eq!(target.socket_inode, inode);
        assert_eq!(target.socket_path, socket_path);

        create_fake_brave_process(&proc_root, 4243, uid, inode, false);
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::Unsafe
        );
    }

    #[test]
    fn discovery_ignores_children_and_custom_user_data_dirs() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        let socket_directory = temporary.path().join("chromium");
        fs::create_dir(&socket_directory).unwrap();
        fs::set_permissions(&socket_directory, fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = socket_directory.join("SingletonSocket");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let inode = socket_inode_for_fd(listener.as_raw_fd());
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        create_fake_brave_process(&proc_root, 11, uid, inode + 1, true);
        let custom = proc_root.join("12");
        create_fake_brave_process(&proc_root, 12, uid, inode, false);
        fs::write(
            custom.join("cmdline"),
            b"/opt/brave-bin/brave --user-data-dir=/tmp/isolated --ozone-platform=wayland\0",
        )
        .unwrap();
        let net_unix = temporary.path().join("unix");
        fs::write(
            &net_unix,
            format!(
                "Num RefCount Protocol Flags Type St Inode Path\n0000: 2 0 00010000 0001 01 {inode} {}\n",
                socket_path.display()
            ),
        )
        .unwrap();
        let layout = ProcLayout {
            root: proc_root,
            net_unix,
        };
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::None
        );
    }

    #[test]
    fn rewritten_chromium_titles_detect_boundary_delimited_switches() {
        let executable = Path::new(BRAVE_EXECUTABLE);
        assert_eq!(
            command_line_contains_switch(
                b"/opt/brave-bin/brave --type=zygote --no-zygote-sandbox\0",
                executable,
                b"--type"
            ),
            Some(true)
        );
        assert_eq!(
            command_line_contains_switch(
                b"/opt/brave-bin/brave --user-data-dir=/tmp/custom --type=renderer\0",
                executable,
                b"--user-data-dir"
            ),
            Some(true)
        );
        assert_eq!(
            command_line_contains_switch(
                b"/opt/brave-bin/brave --label=\"safe --type=word\" --typewriter\0",
                executable,
                b"--type"
            ),
            Some(false)
        );
        assert_eq!(
            command_line_contains_switch(
                b"/opt/brave-bin/brave\0--type=renderer\0",
                executable,
                b"--type"
            ),
            Some(true)
        );
    }

    #[test]
    fn default_main_without_singleton_listener_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        create_fake_brave_process(&proc_root, 20, uid, 999, false);
        let net_unix = temporary.path().join("unix");
        fs::write(
            &net_unix,
            "Num RefCount Protocol Flags Type St Inode Path\n",
        )
        .unwrap();
        let layout = ProcLayout {
            root: proc_root,
            net_unix,
        };
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::Unsafe
        );
    }

    #[test]
    fn deleted_or_mismatched_brave_executable_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        create_fake_brave_process(&proc_root, 30, uid, 1, false);
        fs::remove_file(proc_root.join("30/exe")).unwrap();
        symlink(
            format!("{BRAVE_EXECUTABLE} (deleted)"),
            proc_root.join("30/exe"),
        )
        .unwrap();
        let layout = ProcLayout {
            root: proc_root.clone(),
            net_unix: temporary.path().join("unix"),
        };
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::Unsafe
        );

        fs::remove_dir_all(proc_root.join("30")).unwrap();
        create_fake_brave_process(&proc_root, 31, uid, 1, false);
        fs::remove_file(proc_root.join("31/exe")).unwrap();
        symlink("/tmp/not-the-brave-executable", proc_root.join("31/exe")).unwrap();
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::Unsafe
        );
    }

    #[test]
    fn non_zombie_brave_whose_executable_vanished_is_ignored() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        create_fake_brave_process(&proc_root, 35, uid, 1, false);
        fs::remove_file(proc_root.join("35/exe")).unwrap();
        let layout = ProcLayout {
            root: proc_root,
            net_unix: temporary.path().join("unix"),
        };
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::None
        );
    }

    #[test]
    fn zombie_brave_without_executable_is_ignored() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        create_fake_brave_process(&proc_root, 32, uid, 1, false);
        fs::remove_file(proc_root.join("32/exe")).unwrap();
        fs::write(
            proc_root.join("32/status"),
            format!("Name:\tbrave\nState:\tZ (zombie)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        let layout = ProcLayout {
            root: proc_root,
            net_unix: temporary.path().join("unix"),
        };
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::None
        );
    }

    #[test]
    fn unrelated_same_uid_process_races_are_ignored() {
        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        fs::create_dir(&proc_root).unwrap();
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };

        let unrelated = proc_root.join("33");
        fs::create_dir(&unrelated).unwrap();
        fs::write(
            unrelated.join("status"),
            format!("Name:\tshort-lived\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        fs::create_dir(proc_root.join("34")).unwrap();

        let layout = ProcLayout {
            root: proc_root,
            net_unix: temporary.path().join("unix"),
        };
        assert_eq!(
            discover_brave(&layout, uid, Path::new(BRAVE_EXECUTABLE)),
            BraveDiscovery::None
        );
    }

    #[test]
    fn insecure_singleton_parent_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let socket_path = temporary.path().join("SingletonSocket");
        let _listener = UnixListener::bind(&socket_path).unwrap();
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        assert!(!validate_socket_path(&socket_path, uid));
    }

    #[test]
    fn wrong_peer_pid_is_rejected_before_payload_is_sent() {
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = temporary.path().join("SingletonSocket");
        let ready_path = temporary.path().join("ready");
        let payload_path = temporary.path().join("payload");
        let mut child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "browser::tests::peer_server_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("OMARCHY_CALENDAR_TEST_PEER_SOCKET", &socket_path)
            .env("OMARCHY_CALENDAR_TEST_PEER_READY", &ready_path)
            .env("OMARCHY_CALENDAR_TEST_PEER_PAYLOAD", &payload_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready_path.is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if !ready_path.is_file() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("peer helper did not become ready");
        }
        let ready = fs::read_to_string(&ready_path).unwrap();
        let mut values = ready.split_whitespace();
        let peer_pid: libc::pid_t = values.next().unwrap().parse().unwrap();
        let inode: u64 = values.next().unwrap().parse().unwrap();
        let expected_pid = std::process::id() as libc::pid_t;
        assert_ne!(peer_pid, expected_pid);
        // SAFETY: geteuid has no preconditions and does not dereference memory.
        let uid = unsafe { libc::geteuid() };
        let layout = create_fake_layout(temporary.path(), expected_pid, uid, inode, &socket_path);
        let target = BraveTarget {
            pid: expected_pid,
            uid,
            executable: PathBuf::from(BRAVE_EXECUTABLE),
            socket_inode: inode,
            socket_path,
        };
        assert!(!notify_existing_brave(
            &layout,
            &target,
            "https://accounts.google.com/authorize"
        ));
        assert!(wait_for_test_child(&mut child, Duration::from_secs(3)));
        assert_eq!(fs::read_to_string(payload_path).unwrap(), "0");
    }

    #[test]
    #[ignore]
    fn peer_server_helper() {
        let Some(socket_path) = env::var_os("OMARCHY_CALENDAR_TEST_PEER_SOCKET") else {
            return;
        };
        let ready_path = env::var_os("OMARCHY_CALENDAR_TEST_PEER_READY").unwrap();
        let payload_path = env::var_os("OMARCHY_CALENDAR_TEST_PEER_PAYLOAD").unwrap();
        let listener = UnixListener::bind(PathBuf::from(socket_path)).unwrap();
        let inode = socket_inode_for_fd(listener.as_raw_fd());
        fs::write(ready_path, format!("{} {inode}", std::process::id())).unwrap();
        let Some(mut stream) = accept_with_timeout(&listener, Duration::from_secs(2)) else {
            fs::write(payload_path, "no-connection").unwrap();
            return;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut payload = Vec::new();
        stream.read_to_end(&mut payload).unwrap();
        fs::write(payload_path, payload.len().to_string()).unwrap();
    }

    #[test]
    fn parser_accepts_only_listening_singleton_stream_sockets() {
        let entries = parse_listening_singleton_sockets(
            b"Num RefCount Protocol Flags Type St Inode Path\n\
             0: 2 0 00010000 0001 01 41 /tmp/a/SingletonSocket\n\
             1: 2 0 00000000 0001 01 42 /tmp/b/SingletonSocket\n\
             2: 2 0 00010000 0001 03 43 /tmp/c/SingletonSocket\n\
             3: 2 0 00010000 0002 01 44 /tmp/d/SingletonSocket\n\
             4: 2 0 00010000 0001 01 45 /tmp/e/not-the-browser\n",
        );
        assert_eq!(
            entries,
            vec![UnixSocketEntry {
                inode: 41,
                path: PathBuf::from("/tmp/a/SingletonSocket"),
            }]
        );
    }

    #[test]
    fn parser_does_not_require_unrelated_socket_paths_to_be_utf8() {
        let entries = parse_listening_singleton_sockets(
            b"Num RefCount Protocol Flags Type St Inode Path\n\
             0: 2 0 00010000 0001 01 40 /tmp/\xff/not-brave\n\
             1: 2 0 00010000 0001 01 41 /tmp/a/SingletonSocket\n",
        );
        assert_eq!(
            entries,
            vec![UnixSocketEntry {
                inode: 41,
                path: PathBuf::from("/tmp/a/SingletonSocket"),
            }]
        );
    }

    fn socket_inode_for_fd(fd: std::os::fd::RawFd) -> u64 {
        parse_socket_inode(&fs::read_link(format!("/proc/self/fd/{fd}")).unwrap()).unwrap()
    }

    fn accept_with_timeout(listener: &UnixListener, timeout: Duration) -> Option<UnixStream> {
        listener.set_nonblocking(true).ok()?;
        let deadline = Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).ok()?;
                    return Some(stream);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return None,
            }
        }
    }

    fn wait_for_test_child(child: &mut std::process::Child, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    }

    fn create_fake_layout(
        root: &Path,
        pid: libc::pid_t,
        uid: libc::uid_t,
        inode: u64,
        socket_path: &Path,
    ) -> ProcLayout {
        let proc_root = root.join("proc");
        fs::create_dir(&proc_root).unwrap();
        create_fake_brave_process(&proc_root, pid, uid, inode, false);
        let net_unix = root.join("unix");
        fs::write(
            &net_unix,
            format!(
                "Num RefCount Protocol Flags Type St Inode Path\n0000: 2 0 00010000 0001 01 {inode} {}\n",
                socket_path.display()
            ),
        )
        .unwrap();
        ProcLayout {
            root: proc_root,
            net_unix,
        }
    }

    fn create_fake_brave_process(
        proc_root: &Path,
        pid: libc::pid_t,
        uid: libc::uid_t,
        inode: u64,
        child: bool,
    ) {
        let process = proc_root.join(pid.to_string());
        fs::create_dir(&process).unwrap();
        fs::create_dir(process.join("fd")).unwrap();
        symlink(BRAVE_EXECUTABLE, process.join("exe")).unwrap();
        symlink(format!("socket:[{inode}]"), process.join("fd/15")).unwrap();
        fs::write(
            process.join("status"),
            format!("Name:\tbrave\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        let command_line = if child {
            b"/opt/brave-bin/brave --type=renderer --renderer-client-id=7\0".as_slice()
        } else {
            b"/opt/brave-bin/brave\0".as_slice()
        };
        fs::write(process.join("cmdline"), command_line).unwrap();
    }
}
