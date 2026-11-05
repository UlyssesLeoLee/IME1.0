//! The local server: a named pipe and an HTTP server, for the clients that speak neither MCP nor the CLI.
//!
//! * The **named pipe** (`\\.\pipe\pime-agent-v1`) speaks JSON Lines: one request object per line, one answer per
//!   line. It is restricted to the current user (the security descriptor of the pipe carries the SID of the user),
//!   and it refuses the clients of another machine.
//! * The **HTTP server** binds the loopback address only (never `0.0.0.0`) and asks for a bearer token: a request
//!   without it is refused with 401. The body is limited to one megabyte.
//!
//! Both call the same dispatch as the MCP server and the CLI, so the four front-ends always answer the same.

use crate::{dispatch_request, Backend};
use pime_core::json::{self, Json, Limits};
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// The name of the pipe the clients share.
pub const PIPE_NAME: &str = r"\\.\pipe\pime-agent-v1";

/// The largest HTTP body that is read.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The largest HTTP head that is read.
pub const MAX_HEAD_BYTES: usize = 16 * 1024;

/// How long a client of the pipe may stay silent.
pub const PIPE_IDLE: Duration = Duration::from_secs(300);

/// How long the pipe waits for a client before it makes a new instance.
pub const PIPE_ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

/// What an HTTP request asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum HttpRequest {
    /// `GET /health`: the state of the server.
    Health,
    /// `POST /`: one operation of the agent API.
    Call(Json),
}

/// Why an HTTP request is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpRejection {
    /// The bearer token is missing or wrong.
    Unauthorized,
    /// The request is not one of ours.
    BadRequest(String),
    /// The body is larger than the limit.
    TooLarge,
    /// The path is not one of ours.
    NotFound,
    /// The method does not fit the path.
    MethodNotAllowed,
}

impl HttpRejection {
    /// The status of the answer.
    pub fn status(&self) -> u16 {
        match self {
            HttpRejection::Unauthorized => 401,
            HttpRejection::BadRequest(_) => 400,
            HttpRejection::TooLarge => 413,
            HttpRejection::NotFound => 404,
            HttpRejection::MethodNotAllowed => 405,
        }
    }

    /// The body of the answer.
    pub fn to_json(&self) -> Json {
        let (code, message) = match self {
            HttpRejection::Unauthorized => (
                "UNAUTHORIZED",
                "the bearer token is missing or wrong".to_string(),
            ),
            HttpRejection::BadRequest(message) => ("BAD_REQUEST", message.clone()),
            HttpRejection::TooLarge => (
                "TOO_LARGE",
                format!("the body is larger than {MAX_BODY_BYTES} bytes"),
            ),
            HttpRejection::NotFound => ("NOT_FOUND", "there is no such path".to_string()),
            HttpRejection::MethodNotAllowed => (
                "METHOD_NOT_ALLOWED",
                "the method does not fit the path".to_string(),
            ),
        };
        Json::obj(vec![
            ("ok", Json::Bool(false)),
            ("error", Json::str(code)),
            ("message", Json::str(&message)),
        ])
    }
}

/// Reads one HTTP request and says what it asks for.
///
/// The token is checked before the body is read: a request without the right token is refused with 401, whatever it
/// carries. A body that is not JSON is refused with 400, a body that is too large with 413.
pub fn parse_http_request(raw: &[u8], token: &str) -> Result<HttpRequest, HttpRejection> {
    if raw.len() > MAX_BODY_BYTES + MAX_HEAD_BYTES {
        return Err(HttpRejection::TooLarge);
    }
    let text = std::str::from_utf8(raw)
        .map_err(|_| HttpRejection::BadRequest("the request is not UTF-8".to_string()))?;
    let Some(end) = text.find("\r\n\r\n") else {
        return Err(HttpRejection::BadRequest(
            "the request has no head".to_string(),
        ));
    };
    let head = &text[..end];
    let body = &text[end + 4..];

    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if method.is_empty() || target.is_empty() {
        return Err(HttpRejection::BadRequest(
            "the request line is not one of ours".to_string(),
        ));
    }

    let mut authorization: Option<String> = None;
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => authorization = Some(value.trim().to_string()),
            "content-length" => {
                content_length = value.trim().parse::<usize>().map_err(|_| {
                    HttpRejection::BadRequest("the content length is not a number".to_string())
                })?;
            }
            _ => {}
        }
    }

    let bearer = authorization
        .as_deref()
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let given = bearer.or_else(|| query_value(target, "token"));
    match given {
        Some(given) if !token.is_empty() && constant_time_eq(given.as_bytes(), token.as_bytes()) => {}
        _ => return Err(HttpRejection::Unauthorized),
    }

    let path = target.split('?').next().unwrap_or("");
    match (method, path) {
        ("GET", "/health") => Ok(HttpRequest::Health),
        ("POST", "/") => {
            if content_length > MAX_BODY_BYTES || body.len() > MAX_BODY_BYTES {
                return Err(HttpRejection::TooLarge);
            }
            let value = json::parse(body.trim(), Limits::DEFAULT).map_err(|error| {
                HttpRejection::BadRequest(format!("the body is not JSON: {error}"))
            })?;
            if value.as_object().is_none() {
                return Err(HttpRejection::BadRequest(
                    "the body is not a JSON object".to_string(),
                ));
            }
            Ok(HttpRequest::Call(value))
        }
        (_, "/health") | (_, "/") => Err(HttpRejection::MethodNotAllowed),
        _ => Err(HttpRejection::NotFound),
    }
}

/// The value of a parameter of the query string.
fn query_value<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    let query = target.split_once('?').map(|(_, query)| query)?;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=')
            && key == name
        {
            return Some(value);
        }
    }
    None
}
/// The comparison of two secrets, without an early exit (the token is short, but the habit is worth keeping).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The answer of `GET /health`.
pub fn health_json() -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(true)),
        ("name", Json::str(crate::mcp::SERVER_NAME)),
        ("version", Json::str(crate::VERSION)),
    ])
}

/// The HTTP server: one connection at a time, on the loopback address only.
pub fn serve_http(addr: &str, token: &str, backend: &Backend) -> Result<(), String> {
    check_loopback(addr)?;
    let listener = TcpListener::bind(addr).map_err(|error| {
        format!(
            "the HTTP server could not bind {addr}: {}",
            crate::os_detail(&error)
        )
    })?;
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!(
                    "pime-ime: the connection failed: {}",
                    crate::os_detail(&error)
                );
                continue;
            }
        };
        let _ = handle_http(&mut stream, token, backend);
    }
    Ok(())
}

/// Refuses every address that is not the loopback of this machine.
fn check_loopback(addr: &str) -> Result<(), String> {
    let host = addr
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(addr)
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    match host.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => Ok(()),
        Ok(ip) => Err(format!(
            "the HTTP server binds the loopback address only (refused {ip})"
        )),
        Err(_) => Err(format!("the address {addr} is not an IP address")),
    }
}

/// Answers one HTTP connection.
fn handle_http(stream: &mut TcpStream, token: &str, backend: &Backend) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let raw = read_request(stream)?;
    let (status, body) = match parse_http_request(&raw, token) {
        Ok(HttpRequest::Health) => (200u16, health_json().to_json_string()),
        Ok(HttpRequest::Call(value)) => (200u16, dispatch_request(&value, backend).to_json_string()),
        Err(rejection) => (rejection.status(), rejection.to_json().to_json_string()),
    };
    write_response(stream, status, &body)
}

/// Reads the head and the body of one request.
fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).map_err(|error| {
            format!(
                "the request could not be read: {}",
                crate::os_detail(&error)
            )
        })?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() > MAX_BODY_BYTES + MAX_HEAD_BYTES {
            break;
        }
        if let Some(end) = find_head_end(&raw) {
            let head = String::from_utf8_lossy(&raw[..end]).to_string();
            let length = content_length_of(&head);
            if raw.len() >= end + 4 + length {
                break;
            }
        }
    }
    Ok(raw)
}
/// The end of the head of a request (the two line breaks that end it).
fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

/// The length of the body, as the head says it.
fn content_length_of(head: &str) -> usize {
    for line in head.split("\r\n").skip(1) {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            return value.trim().parse::<usize>().unwrap_or(0);
        }
    }
    0
}

/// Writes one HTTP answer.
fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).map_err(|error| {
        format!(
            "the answer could not be written: {}",
            crate::os_detail(&error)
        )
    })?;
    stream.write_all(body.as_bytes()).map_err(|error| {
        format!(
            "the answer could not be written: {}",
            crate::os_detail(&error)
        )
    })?;
    let _ = stream.flush();
    Ok(())
}

/// The named pipe: JSON Lines, one request per line, one answer per line.
#[cfg(windows)]
pub fn serve_pipe(backend: Arc<Backend>) -> Result<(), String> {
    pipe_win::serve(backend)
}

/// The named pipe: JSON Lines, one request per line, one answer per line.
#[cfg(not(windows))]
pub fn serve_pipe(_backend: Arc<Backend>) -> Result<(), String> {
    Err("the named pipe needs Windows".to_string())
}

/// Runs the server: the pipe on a thread of its own, the HTTP server in the foreground.
pub fn serve(pipe: bool, http: Option<&str>, token: Option<&str>, backend: &Backend) -> i32 {
    if !pipe && http.is_none() {
        eprintln!("pime-ime: the server needs --pipe, --http <addr> or both");
        return 1;
    }
    let backend = Arc::new(backend.clone());
    if let Some(addr) = http {
        let Some(token) = token else {
            eprintln!("pime-ime: the HTTP server needs a bearer token (--token <token>)");
            return 1;
        };
        if pipe {
            let pipe_backend = Arc::clone(&backend);
            std::thread::spawn(move || {
                if let Err(error) = serve_pipe(pipe_backend) {
                    eprintln!("pime-ime: the pipe stopped: {error}");
                }
            });
        }
        if let Err(error) = serve_http(addr, token, &backend) {
            eprintln!("pime-ime: {error}");
            return 1;
        }
        return 0;
    }
    if let Err(error) = serve_pipe(backend) {
        eprintln!("pime-ime: {error}");
        return 1;
    }
    0
}

/// The Win32 side of the named pipe.
#[cfg(windows)]
#[allow(unsafe_code)]
mod pipe_win {
    use super::{PIPE_ACCEPT_TIMEOUT, PIPE_IDLE, PIPE_NAME};
    use crate::{dispatch_request, AgentError, Backend};
    use pime_core::agent_proto::Error;
    use pime_core::json::{self, Limits};
    use std::ffi::c_void;
    use std::sync::Arc;
    use std::time::Instant;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
        ERROR_PIPE_NOT_CONNECTED, HANDLE, HLOCAL, HWND, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile, WriteFile};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, OpenProcessToken, ResetEvent, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR};

    /// The access mask the user has on the pipe: read, write and the right to make the next instance.
    const PIPE_USER_ACCESS_MASK: u32 = 0x0012_019F;

    /// The Win32 error number inside a `windows::core::Error` (the `HRESULT_FROM_WIN32` values carry it in the low
    /// word of the HRESULT).
    fn win32_code(error: &windows::core::Error) -> u32 {
        let hr = error.code().0 as u32;
        if hr >> 16 == 0x8007 { hr & 0xFFFF } else { hr }
    }

    /// A handle that is closed exactly once.
    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: the handle was valid when it was wrapped and this is the only owner.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    // SAFETY: a kernel handle is a process-wide identifier; moving it to another thread is what the API is for.
    unsafe impl Send for OwnedHandle {}

    /// Memory the system allocated (`LocalAlloc`), freed on drop.
    struct LocalMem(*mut c_void);

    impl Drop for LocalMem {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from a `LocalAlloc` of the system and is freed once.
                unsafe {
                    LocalFree(HLOCAL(self.0));
                }
            }
        }
    }

    /// The security descriptor of the pipe: only the user of this session may use it.
    struct SecurityDescriptor {
        pointer: PSECURITY_DESCRIPTOR,
    }

    impl SecurityDescriptor {
        /// The descriptor of the current user (`D:P(A;;mask;;;<SID>)`).
        fn for_current_user() -> Result<SecurityDescriptor, String> {
            let sid = current_user_sid()?;
            let sddl = format!("D:P(A;;{PIPE_USER_ACCESS_MASK:#x};;;{sid})");
            let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut pointer = PSECURITY_DESCRIPTOR::default();
            // SAFETY: the string is NUL-terminated and outlives the call; the out pointer is valid.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(wide.as_ptr()),
                    SDDL_REVISION_1,
                    &mut pointer,
                    None,
                )
            }
            .map_err(|error| {
                format!(
                    "the security descriptor of the pipe is refused: os error {}",
                    win32_code(&error)
                )
            })?;
            Ok(SecurityDescriptor { pointer })
        }

        /// The attributes that the creation of the pipe carries.
        fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.pointer.0,
                bInheritHandle: windows::Win32::Foundation::BOOL(0),
            }
        }
    }

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            drop(LocalMem(self.pointer.0));
        }
    }

    /// The SID of the user of this process, as a string.
    fn current_user_sid() -> Result<String, String> {
        let mut token = HANDLE::default();
        // SAFETY: the out pointer is valid and the pseudo handle of the process needs no cleanup.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|error| {
                format!(
                    "the token of the process could not be opened: os error {}",
                    win32_code(&error)
                )
            })?;
        let token = OwnedHandle(token);

        let mut needed = 0u32;
        // SAFETY: a null buffer of length 0 is the documented size query; it fails on purpose.
        unsafe {
            let _ = GetTokenInformation(token.raw(), TokenUser, None, 0, &mut needed);
        }
        if needed == 0 || needed > 4096 {
            return Err("the SID of the user could not be read".to_string());
        }
        // u64 elements: the buffer is 8-aligned, as TOKEN_USER (it holds pointers) requires
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer holds at least `needed` bytes and is suitably aligned.
        unsafe {
            GetTokenInformation(
                token.raw(),
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .map_err(|error| {
            format!(
                "the SID of the user could not be read: os error {}",
                win32_code(&error)
            )
        })?;
        // SAFETY: the call above filled the buffer with a TOKEN_USER whose SID points into it.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };

        let mut wide = PWSTR::null();
        // SAFETY: the SID is valid (see above) and the out pointer receives LocalAlloc memory.
        unsafe { ConvertSidToStringSidW(sid, &mut wide) }
            .map_err(|error| {
                format!(
                    "the SID of the user could not be written: os error {}",
                    win32_code(&error)
                )
            })?;
        let _free = LocalMem(wide.0.cast());
        // SAFETY: the call succeeded, so the string is NUL-terminated and `_free` keeps it alive.
        unsafe { wide.to_string() }.map_err(|_| "the SID of the user is not UTF-16".to_string())
    }

    /// Creates one instance of the pipe, in overlapped mode.
    fn create_instance(descriptor: &SecurityDescriptor) -> Result<OwnedHandle, String> {
        let name: Vec<u16> = PIPE_NAME.encode_utf16().chain(Some(0)).collect();
        let attributes = descriptor.attributes();
        // SAFETY: the name is NUL-terminated and the descriptor outlives the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                Some(&attributes),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = windows::core::Error::from_win32();
            return Err(format!(
                "the pipe {PIPE_NAME} could not be created: os error {}",
                win32_code(&error)
            ));
        }
        Ok(OwnedHandle(handle))
    }

    /// Creates the event that waits for the overlapped operations.
    fn create_event() -> Result<OwnedHandle, String> {
        // SAFETY: no security attributes and no name: a manual-reset event of this process.
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(OwnedHandle)
            .map_err(|error| {
                format!(
                    "the event of the pipe could not be created: os error {}",
                    win32_code(&error)
                )
            })
    }

    /// Waits for a client to connect.
    fn connect(handle: HANDLE, event: HANDLE) -> Result<(), String> {
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        // SAFETY: the structure and the handle are valid until the operation is finished.
        match unsafe { ConnectNamedPipe(handle, Some(&mut overlapped)) } {
            Ok(()) => Ok(()),
            Err(error) if win32_code(&error) == ERROR_PIPE_CONNECTED.0 => Ok(()),
            Err(error) if win32_code(&error) == ERROR_IO_PENDING.0 => {
                let waited = unsafe {
                    WaitForSingleObject(event, PIPE_ACCEPT_TIMEOUT.as_millis() as u32)
                };
                if waited != windows::Win32::Foundation::WAIT_OBJECT_0 {
                    // SAFETY: the pending operation belongs to this handle; the cancel ends it.
                    unsafe {
                        let _ = CancelIoEx(handle, Some(&overlapped));
                    }
                    // SAFETY: waiting here keeps the structure alive until the kernel is done with it.
                    let mut transferred = 0u32;
                    unsafe {
                        let _ = GetOverlappedResult(handle, &overlapped, &mut transferred, true);
                    }
                    return Err("no client connected".to_string());
                }
                let mut transferred = 0u32;
                // SAFETY: the operation is finished (the event was signalled).
                unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, true) }
                    .map_err(|error| {
                        format!(
                            "the pipe could not be opened: os error {}",
                            win32_code(&error)
                        )
                    })?;
                Ok(())
            }
            Err(error) => Err(format!(
                "the pipe could not be opened: os error {}",
                win32_code(&error)
            )),
        }
    }

    /// Reads one line of the pipe (`None` when the client closed it).
    fn read_line(handle: HANDLE, event: HANDLE, deadline: Instant) -> Result<Option<String>, String> {
        let mut line: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("the client of the pipe is silent".to_string());
            }
            // SAFETY: the event is ours and is reset before every operation.
            unsafe {
                let _ = ResetEvent(event);
            }
            let mut overlapped = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            // SAFETY: the buffer and the structure live until the operation is finished.
            match unsafe { ReadFile(handle, Some(&mut chunk), None, Some(&mut overlapped)) } {
                Ok(()) => {}
                Err(error) if win32_code(&error) == ERROR_IO_PENDING.0 => {
                    let milliseconds = remaining.as_millis().min(u32::MAX as u128) as u32;
                    // SAFETY: the event is ours and the wait is bounded.
                    let waited = unsafe { WaitForSingleObject(event, milliseconds) };
                    if waited != windows::Win32::Foundation::WAIT_OBJECT_0 {
                        // SAFETY: the pending operation belongs to this handle; the cancel ends it.
                        unsafe {
                            let _ = CancelIoEx(handle, Some(&overlapped));
                        }
                        let mut transferred = 0u32;
                        // SAFETY: waiting here keeps the structure alive until the kernel is done with it.
                        unsafe {
                            let _ = GetOverlappedResult(handle, &overlapped, &mut transferred, true);
                        }
                        return Err("the client of the pipe is silent".to_string());
                    }
                }
                Err(error) => {
                    // the client closed the pipe: that is the end of the connection, not a failure
                    let code = win32_code(&error);
                    if code == ERROR_BROKEN_PIPE.0
                        || code == ERROR_PIPE_NOT_CONNECTED.0
                        || code == ERROR_NO_DATA.0
                    {
                        return Ok(None);
                    }
                    return Err(format!("the pipe could not be read: os error {code}"));
                }
            }
            let mut transferred = 0u32;
            // SAFETY: the operation is finished.
            unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, true) }
                .map_err(|error| {
                    format!(
                        "the pipe could not be read: os error {}",
                        win32_code(&error)
                    )
                })?;
            if transferred == 0 {
                if line.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(String::from_utf8_lossy(&line).to_string()));
            }
            line.extend_from_slice(&chunk[..transferred as usize]);
            if let Some(position) = line.iter().position(|byte| *byte == b'\n') {
                let text = String::from_utf8_lossy(&line[..position])
                    .trim_end_matches('\r')
                    .to_string();
                return Ok(Some(text));
            }
            if line.len() > super::MAX_BODY_BYTES {
                return Err("the request of the pipe is too long".to_string());
            }
        }
    }

    /// Writes the answer of one request.
    fn write_all(handle: HANDLE, event: HANDLE, data: &[u8]) -> Result<(), String> {
        let mut sent = 0usize;
        while sent < data.len() {
            let chunk = &data[sent..];
            // SAFETY: the event is ours and is reset before every operation.
            unsafe {
                let _ = ResetEvent(event);
            }
            let mut overlapped = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            // SAFETY: the buffer and the structure live until the operation is finished.
            match unsafe { WriteFile(handle, Some(chunk), None, Some(&mut overlapped)) } {
                Ok(()) => {}
                Err(error) if win32_code(&error) == ERROR_IO_PENDING.0 => {
                    // SAFETY: the event is ours and the wait is bounded.
                    let waited = unsafe { WaitForSingleObject(event, 30_000) };
                    if waited != windows::Win32::Foundation::WAIT_OBJECT_0 {
                        // SAFETY: the pending operation belongs to this handle; the cancel ends it.
                        unsafe {
                            let _ = CancelIoEx(handle, Some(&overlapped));
                        }
                        let mut transferred = 0u32;
                        // SAFETY: waiting here keeps the structure alive until the kernel is done with it.
                        unsafe {
                            let _ = GetOverlappedResult(handle, &overlapped, &mut transferred, true);
                        }
                        return Err("the client of the pipe does not read".to_string());
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "the answer could not be written: os error {}",
                        win32_code(&error)
                    ));
                }
            }
            let mut transferred = 0u32;
            // SAFETY: the operation is finished.
            unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, true) }
                .map_err(|error| {
                    format!(
                        "the answer could not be written: os error {}",
                        win32_code(&error)
                    )
                })?;
            sent += transferred as usize;
        }
        Ok(())
    }

    /// Serves one client of the pipe.
    fn serve_client(instance: &OwnedHandle, event: &OwnedHandle, backend: &Backend) -> Result<(), String> {
        let deadline = Instant::now() + PIPE_IDLE;
        loop {
            let Some(line) = read_line(instance.raw(), event.raw(), deadline)? else {
                return Ok(());
            };
            if line.trim().is_empty() {
                continue;
            }
            let answer = match json::parse(line.trim(), Limits::DEFAULT) {
                Ok(value) => dispatch_request(&value, backend),
                Err(error) => AgentError::protocol(
                    Error::Malformed,
                    format!("the request is not JSON: {error}"),
                )
                .to_answer(),
            };
            let mut out = answer.to_json_string();
            out.push('\n');
            write_all(instance.raw(), event.raw(), out.as_bytes())?;
        }
    }

    /// The loop of the pipe: one instance at a time, one thread per client.
    pub fn serve(backend: Arc<Backend>) -> Result<(), String> {
        let descriptor = SecurityDescriptor::for_current_user()?;
        loop {
            let instance = create_instance(&descriptor)?;
            let event = create_event()?;
            if let Err(error) = connect(instance.raw(), event.raw()) {
                // no client in time: the instance is dropped and a new one is made
                eprintln!("pime-ime: the pipe waits for a client: {error}");
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            let backend = Arc::clone(&backend);
            std::thread::spawn(move || {
                if let Err(error) = serve_client(&instance, &event, &backend) {
                    eprintln!("pime-ime: the client of the pipe stopped: {error}");
                }
            });
        }
    }

    /// Keeps the linker honest about the unused import of the handle type.
    #[allow(dead_code)]
    fn unused(_: HWND) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::FakeBackend;

    const TOKEN: &str = "s3cret";

    fn request(head: &str, body: &str) -> Vec<u8> {
        let mut raw = head.as_bytes().to_vec();
        raw.extend_from_slice(body.as_bytes());
        raw
    }

    #[test]
    fn a_request_without_the_token_is_refused() {
        let raw = request(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n",
            "{}",
        );
        let error = parse_http_request(&raw, TOKEN).expect_err("the token is missing");
        assert_eq!(error, HttpRejection::Unauthorized);
        assert_eq!(error.status(), 401);
    }

    #[test]
    fn a_request_with_a_wrong_token_is_refused() {
        let raw = request(
            "POST / HTTP/1.1\r\nAuthorization: Bearer wrong\r\nContent-Length: 2\r\n\r\n",
            "{}",
        );
        assert_eq!(
            parse_http_request(&raw, TOKEN).expect_err("the token is wrong"),
            HttpRejection::Unauthorized
        );
    }

    #[test]
    fn the_token_may_come_from_the_query_string() {
        let raw = request(
            "POST /?token=s3cret HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n",
            "{}",
        );
        let parsed = parse_http_request(&raw, TOKEN).expect("the token is right");
        assert!(matches!(parsed, HttpRequest::Call(_)));
    }

    #[test]
    fn a_body_that_is_not_json_is_refused() {
        let raw = request(
            "POST / HTTP/1.1\r\nAuthorization: Bearer s3cret\r\nContent-Length: 5\r\n\r\n",
            "not j",
        );
        let error = parse_http_request(&raw, TOKEN).expect_err("the body is not JSON");
        assert!(matches!(error, HttpRejection::BadRequest(_)), "{error:?}");
        assert_eq!(error.status(), 400);
    }

    #[test]
    fn a_body_that_is_too_big_is_refused() {
        let head = format!(
            "POST / HTTP/1.1\r\nAuthorization: Bearer s3cret\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        let raw = request(&head, "");
        let error = parse_http_request(&raw, TOKEN).expect_err("the body is too big");
        assert_eq!(error, HttpRejection::TooLarge);
        assert_eq!(error.status(), 413);
    }

    #[test]
    fn a_request_without_a_head_is_refused() {
        let error = parse_http_request(b"GET / HTTP/1.1", TOKEN).expect_err("there is no head");
        assert!(matches!(error, HttpRejection::BadRequest(_)), "{error:?}");
    }

    #[test]
    fn an_unknown_path_is_refused() {
        let raw = request(
            "GET /nope HTTP/1.1\r\nAuthorization: Bearer s3cret\r\n\r\n",
            "",
        );
        assert_eq!(
            parse_http_request(&raw, TOKEN).expect_err("there is no such path"),
            HttpRejection::NotFound
        );
    }

    #[test]
    fn the_health_is_a_small_json() {
        let raw = request(
            "GET /health HTTP/1.1\r\nAuthorization: Bearer s3cret\r\n\r\n",
            "",
        );
        let parsed = parse_http_request(&raw, TOKEN).expect("the health");
        assert_eq!(parsed, HttpRequest::Health);
        let health = health_json();
        assert_eq!(health.get("ok"), Some(&Json::Bool(true)));
        assert!(health.get("name").is_some());
    }

    #[test]
    fn the_http_dispatch_calls_the_same_dispatch_as_the_other_front_ends() {
        let backend = FakeBackend::new();
        let body = r#"{"op":"insert","text":"hello"}"#;
        let head = format!(
            "POST / HTTP/1.1\r\nAuthorization: Bearer s3cret\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let raw = request(&head, body);
        let parsed = parse_http_request(&raw, TOKEN).expect("the request");
        let HttpRequest::Call(value) = parsed else {
            panic!("the request is a call");
        };
        let answer = dispatch_request(&value, backend.backend());
        assert_eq!(answer.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(answer.get("inserted").and_then(Json::as_i64), Some(3));
    }

    #[test]
    fn the_server_refuses_an_address_that_is_not_the_loopback() {
        assert!(check_loopback("127.0.0.1:8731").is_ok());
        assert!(check_loopback("localhost:8731").is_ok());
        assert!(check_loopback("0.0.0.0:8731").is_err());
        assert!(check_loopback("192.168.0.10:8731").is_err());
    }

    #[test]
    fn the_pipe_has_the_name_of_the_contract() {
        assert_eq!(PIPE_NAME, r"\\.\pipe\pime-agent-v1");
    }
}
