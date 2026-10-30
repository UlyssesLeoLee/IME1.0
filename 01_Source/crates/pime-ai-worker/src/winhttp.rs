//! HTTPS through WinHTTP (SChannel): the system trust store, the system proxy, no TLS stack of our own.
//!
//! One request = one synchronous WinHTTP exchange on the calling thread. Cancel and timeout stop it by closing the
//! request handle from another thread (`Flight::arm`), which makes the blocked call return
//! `ERROR_WINHTTP_OPERATION_CANCELLED`; the caller (the engine) turns that into `CANCELLED` or `TIMEOUT` from its own
//! record of who aborted. The residual risk of that scheme is documented in docs/AI_WORKER.md: between the abort check
//! and the next WinHTTP call the handle may already be closed, and a handle value freed by `WinHttpCloseHandle` could in
//! theory be handed out again to another request before the stale call lands.
//!
//! Defaults that matter for security: TLS 1.2 and 1.3 only, redirects are never followed (a 3xx is returned and the
//! engine refuses it), no cookies, no automatic credentials, the response is read in 16 KiB chunks and abandoned the
//! moment it exceeds the cap. The only relaxation is the test-only `accept_untrusted_ca`, which sets
//! `SECURITY_FLAG_IGNORE_UNKNOWN_CA` and nothing else (host name and validity dates are still checked).
#![allow(unsafe_code)]

use crate::error::{Code, Failure};
use crate::errmap::{SecureFailure, map_win32_error};
use crate::transport::{Flight, HttpResponse, PreparedRequest, Transport, header_block};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Networking::WinHttp::{
    SECURITY_FLAG_IGNORE_UNKNOWN_CA, WINHTTP_ACCESS_TYPE, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY,
    WINHTTP_CALLBACK_FLAG_SECURE_FAILURE, WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
    WINHTTP_DISABLE_AUTHENTICATION, WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_SECURE,
    WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_DECOMPRESSION,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_OPTION_SECURE_PROTOCOLS,
    WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_OPTION_SERVER_CERT_CONTEXT, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetStatusCallback,
    WinHttpSetTimeouts,
};
use windows::Win32::Security::Cryptography::{CERT_CONTEXT, CertFreeCertificateContext};
use windows::core::PCWSTR;

/// `WINHTTP_DECOMPRESSION_FLAG_ALL` (gzip | deflate); not exported by the pinned `windows` version.
const DECOMPRESSION_ALL: u32 = 3;
const READ_CHUNK: usize = 16 * 1024;
const AGENT: &str = "ProgrammingIME-AIWorker/0.2.0";

#[derive(Clone, Copy)]
struct Handle(*mut c_void);

// SAFETY: a WinHTTP HINTERNET is an opaque token that the API documents as usable from any thread; the only shared
// mutation (closing) is serialised by `HandleSlot`.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(0)
        .unsigned_abs()
}

fn win32_of(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr >> 16 == 0x8007 { hr & 0xffff } else { hr }
}

fn set_option(handle: Handle, option: u32, value: &[u8]) -> Result<(), u32> {
    // SAFETY: `handle` is an open WinHTTP handle and `value` is a plain buffer for the duration of the call.
    unsafe { WinHttpSetOption(Some(handle.0 as *const c_void), option, Some(value)) }
        .map_err(|e| win32_of(&e))
}

fn open_session(access: WINHTTP_ACCESS_TYPE) -> Result<Handle, u32> {
    let agent = wide(AGENT);
    // SAFETY: `agent` is NUL-terminated and outlives the call; null proxy arguments are allowed for these access types.
    let h = unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            access,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    };
    if h.is_null() {
        Err(last_error())
    } else {
        Ok(Handle(h))
    }
}

/// TLS 1.2 and 1.3 only; falls back to 1.2 on a system whose WinHTTP does not know the 1.3 flag.
fn restrict_protocols(session: Handle) {
    let both = (WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3).to_ne_bytes();
    if set_option(session, WINHTTP_OPTION_SECURE_PROTOCOLS, &both).is_err() {
        let _ = set_option(
            session,
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            &WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2.to_ne_bytes(),
        );
    }
}

/// Closes a handle exactly once, whichever thread gets there first (the sender when it is done, or an abort).
struct HandleSlot(Mutex<Option<Handle>>);

impl HandleSlot {
    fn close(&self) {
        let taken = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = taken {
            // SAFETY: `h` was taken out of the slot, so this is the only close of this handle.
            let _ = unsafe { WinHttpCloseHandle(h.0) };
        }
    }
}

/// Closes the connection handle on drop.
struct ConnectionGuard(Handle);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        // SAFETY: the connection handle is owned by this guard and closed once; the request handle derived from it was
        // already closed (the request guard is declared after this one and therefore dropped first).
        let _ = unsafe { WinHttpCloseHandle(self.0.0) };
    }
}

/// Disarms the abort hook and closes the request handle on every exit path.
struct RequestGuard<'a> {
    slot: Arc<HandleSlot>,
    flight: &'a Flight,
}

impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        self.flight.disarm();
        self.slot.close();
    }
}

/// What the TLS status callback learned about a failed handshake. Lives on the sending thread's stack; the request
/// handle that points at it is closed before the stack frame ends.
#[derive(Default)]
struct CallbackContext {
    flags: AtomicU32,
    /// 0 unknown, 1 the certificate is not self-issued, 2 it is.
    self_issued: AtomicU8,
}

impl CallbackContext {
    fn failure(&self) -> Option<SecureFailure> {
        let flags = self.flags.load(Ordering::SeqCst);
        if flags == 0 {
            return None;
        }
        Some(SecureFailure {
            flags,
            self_issued: match self.self_issued.load(Ordering::SeqCst) {
                1 => Some(false),
                2 => Some(true),
                _ => None,
            },
        })
    }
}

/// Whether the server certificate's subject and issuer are the same name (a self-signed leaf).
///
/// # Safety
/// `request` must be an open WinHTTP request handle on which a TLS failure was just reported.
unsafe fn server_cert_is_self_issued(request: *mut c_void) -> Option<bool> {
    let mut cert: *const CERT_CONTEXT = std::ptr::null();
    let mut size = std::mem::size_of::<*const CERT_CONTEXT>() as u32;
    // SAFETY: per the caller's contract; `cert` is a valid out-buffer of `size` bytes.
    let queried = unsafe {
        WinHttpQueryOption(
            request,
            WINHTTP_OPTION_SERVER_CERT_CONTEXT,
            Some(&mut cert as *mut *const CERT_CONTEXT as *mut c_void),
            &mut size,
        )
    };
    if queried.is_err() || cert.is_null() {
        return None;
    }
    // SAFETY: `WinHttpQueryOption` returned a certificate context that we own until `CertFreeCertificateContext`;
    // `pCertInfo` and the name blobs are valid for the context's lifetime.
    let result = unsafe {
        let info = (*cert).pCertInfo;
        if info.is_null() {
            None
        } else {
            let (issuer, subject) = (&(*info).Issuer, &(*info).Subject);
            if issuer.pbData.is_null() || subject.pbData.is_null() {
                None
            } else {
                Some(
                    std::slice::from_raw_parts(issuer.pbData, issuer.cbData as usize)
                        == std::slice::from_raw_parts(subject.pbData, subject.cbData as usize),
                )
            }
        }
    };
    // SAFETY: releases the reference obtained above, once.
    let _ = unsafe { CertFreeCertificateContext(Some(cert)) };
    result
}

/// Called by WinHTTP on the thread that is inside a failing call, for secure failures only.
unsafe extern "system" fn status_callback(
    request: *mut c_void,
    context: usize,
    status: u32,
    info: *mut c_void,
    info_len: u32,
) {
    if status != WINHTTP_CALLBACK_STATUS_SECURE_FAILURE
        || context == 0
        || info.is_null()
        || (info_len as usize) < std::mem::size_of::<u32>()
    {
        return;
    }
    // SAFETY: `context` is the address of the `CallbackContext` registered by `exchange`, which outlives the request
    // handle; for this status WinHTTP passes a pointer to a DWORD of `WINHTTP_CALLBACK_STATUS_FLAG_*` bits.
    let ctx = unsafe { &*(context as *const CallbackContext) };
    let flags = unsafe { *(info as *const u32) };
    ctx.flags.fetch_or(flags, Ordering::SeqCst);
    // SAFETY: `request` is the handle the failure was reported on.
    if let Some(self_issued) = unsafe { server_cert_is_self_issued(request) } {
        ctx.self_issued
            .store(if self_issued { 2 } else { 1 }, Ordering::SeqCst);
    }
}

pub struct WinHttpTransport {
    /// Resolves the proxy from the system settings (WPAD/PAC/static), for every host except loopback.
    system_proxy: Handle,
    /// Never uses a proxy: for loopback targets, which no proxy could reach anyway.
    direct: Handle,
}

impl WinHttpTransport {
    /// The error is the Win32 code of the failed `WinHttpOpen`.
    pub fn new() -> Result<WinHttpTransport, u32> {
        // AUTOMATIC_PROXY needs Windows 8.1 or later; older systems use the WinHTTP default proxy configuration
        let system_proxy = open_session(WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY)
            .or_else(|_| open_session(WINHTTP_ACCESS_TYPE_DEFAULT_PROXY))?;
        let direct = open_session(WINHTTP_ACCESS_TYPE_NO_PROXY)?;
        restrict_protocols(system_proxy);
        restrict_protocols(direct);
        Ok(WinHttpTransport {
            system_proxy,
            direct,
        })
    }

    fn exchange(
        &self,
        req: &PreparedRequest,
        flight: &Flight,
        ctx: &CallbackContext,
    ) -> Result<HttpResponse, Failure> {
        let aborted = || Failure::new(Code::Cancelled);
        let fail = |e: u32| map_win32_error(e, ctx.failure());
        if flight.is_aborted() {
            return Err(aborted());
        }
        let session = if req.target.is_loopback() {
            self.direct
        } else {
            self.system_proxy
        };
        let host = wide(&req.target.host);
        // SAFETY: `session` is open for the life of the transport; `host` is NUL-terminated and outlives the call.
        let connect = unsafe { WinHttpConnect(session.0, PCWSTR(host.as_ptr()), req.target.port, 0) };
        if connect.is_null() {
            return Err(fail(last_error()));
        }
        let _connection = ConnectionGuard(Handle(connect));
        let verb = wide(req.method.as_str());
        let path = wide(&req.target.path_and_query);
        let open_flags = if req.target.https {
            WINHTTP_FLAG_SECURE
        } else {
            WINHTTP_OPEN_REQUEST_FLAGS(0)
        };
        // SAFETY: `connect` is an open connection handle; `verb` and `path` are NUL-terminated and outlive the call; the
        // null version/referrer/accept-types arguments mean "default".
        let request = unsafe {
            WinHttpOpenRequest(
                connect,
                PCWSTR(verb.as_ptr()),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                open_flags,
            )
        };
        if request.is_null() {
            return Err(fail(last_error()));
        }
        let slot = Arc::new(HandleSlot(Mutex::new(Some(Handle(request)))));
        let _request_guard = RequestGuard {
            slot: slot.clone(),
            flight,
        };
        let hook_slot = slot.clone();
        if !flight.arm(Box::new(move || hook_slot.close())) {
            return Err(aborted());
        }
        let request = Handle(request);

        // Security-relevant options must take effect or the request is not sent at all.
        let hardening = [
            (
                WINHTTP_OPTION_REDIRECT_POLICY,
                WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes(),
            ),
            (
                WINHTTP_OPTION_DISABLE_FEATURE,
                (WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION).to_ne_bytes(),
            ),
        ];
        for (option, value) in hardening {
            set_option(request, option, &value).map_err(|e| {
                let mut f = Failure::new(Code::TransportError);
                f.os_code = Some(e);
                f
            })?;
        }
        if req.accept_untrusted_ca {
            set_option(
                request,
                WINHTTP_OPTION_SECURITY_FLAGS,
                &SECURITY_FLAG_IGNORE_UNKNOWN_CA.to_ne_bytes(),
            )
            .map_err(|e| {
                let mut f = Failure::new(Code::TransportError);
                f.os_code = Some(e);
                f
            })?;
        }
        // Best effort: gzip/deflate bodies are decoded by WinHTTP, so the response cap applies to what we read.
        let _ = set_option(request, WINHTTP_OPTION_DECOMPRESSION, &DECOMPRESSION_ALL.to_ne_bytes());
        let ms = i32::try_from(req.timeout.as_millis()).unwrap_or(i32::MAX).max(1);
        // SAFETY: `request` is an open handle. The timeouts are a backstop behind the engine's own deadline.
        let _ = unsafe { WinHttpSetTimeouts(request.0, ms, ms, ms, ms) };
        // The callback only fills `ctx`, which outlives the request handle (closed by `_request_guard` before return).
        let ctx_address = (ctx as *const CallbackContext as usize).to_ne_bytes();
        let _ = set_option(request, WINHTTP_OPTION_CONTEXT_VALUE, &ctx_address);
        // SAFETY: `request` is open and `status_callback` has the required signature; the previous callback is ignored.
        let _ = unsafe {
            WinHttpSetStatusCallback(
                request.0,
                Some(status_callback),
                WINHTTP_CALLBACK_FLAG_SECURE_FAILURE,
                0,
            )
        };

        // Send: the header block holds the key, so it is wiped as soon as WinHTTP has taken it.
        let headers = header_block(req);
        let body_ptr = (!req.body.is_empty()).then(|| req.body.as_ptr() as *const c_void);
        let body_len = u32::try_from(req.body.len()).unwrap_or(u32::MAX);
        if flight.is_aborted() {
            return Err(aborted());
        }
        // SAFETY: `request` is open; the header block and the body outlive the call (the call is synchronous) and the
        // lengths are exact. An empty header block is passed as "no headers".
        let sent = unsafe {
            WinHttpSendRequest(
                request.0,
                (!headers.0.is_empty()).then_some(headers.0.as_slice()),
                body_ptr,
                body_len,
                body_len,
                0,
            )
        };
        drop(headers);
        sent.map_err(|e| fail(win32_of(&e)))?;

        if flight.is_aborted() {
            return Err(aborted());
        }
        // SAFETY: `request` is open and the request was sent; the reserved argument must be null.
        unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
            .map_err(|e| fail(win32_of(&e)))?;
        let status = query_status(request).map_err(fail)?;
        let headers = query_headers(request).map_err(fail)?;

        let mut body: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; READ_CHUNK];
        loop {
            if flight.is_aborted() {
                return Err(aborted());
            }
            let mut read = 0u32;
            // SAFETY: `chunk` is a writable buffer of `READ_CHUNK` bytes and `read` a valid out-pointer.
            unsafe {
                WinHttpReadData(
                    request.0,
                    chunk.as_mut_ptr() as *mut c_void,
                    READ_CHUNK as u32,
                    &mut read,
                )
            }
            .map_err(|e| fail(win32_of(&e)))?;
            if read == 0 {
                break;
            }
            if body.len() + read as usize > req.max_response_bytes {
                // abandoning the exchange closes the handle, which stops the download
                return Err(Failure::new(Code::ResponseTooLarge));
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
        Ok(HttpResponse {
            status: u16::try_from(status).unwrap_or(0),
            headers,
            body,
        })
    }
}

impl Drop for WinHttpTransport {
    fn drop(&mut self) {
        for session in [self.system_proxy, self.direct] {
            // SAFETY: the sessions are owned by the transport and closed once, after every request handle is gone.
            let _ = unsafe { WinHttpCloseHandle(session.0) };
        }
    }
}

fn query_status(request: Handle) -> Result<u32, u32> {
    let mut status = 0u32;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: `request` is open with a received response; `status` is a 4-byte out-buffer as the NUMBER flag requires.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| win32_of(&e))?;
    Ok(status)
}

/// The response headers as `(lower-case name, value)`; the status line is skipped.
fn query_headers(request: Handle) -> Result<Vec<(String, String)>, u32> {
    let mut bytes = 0u32;
    // SAFETY: the first call only asks for the required size (a null buffer is the documented way to do that).
    let _ = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            PCWSTR::null(),
            None,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if bytes == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u16; bytes as usize / 2 + 1];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` is a writable buffer of `len` bytes.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            PCWSTR::null(),
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| win32_of(&e))?;
    let text = String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]);
    Ok(parse_raw_headers(&text))
}

/// `HTTP/1.1 200 OK\r\nName: value\r\n...` -> `[(name, value)]` with lower-case names; malformed lines are skipped.
pub fn parse_raw_headers(raw: &str) -> Vec<(String, String)> {
    raw.split("\r\n")
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

impl Transport for WinHttpTransport {
    fn name(&self) -> &'static str {
        "winhttp"
    }

    fn send(&self, request: &PreparedRequest, flight: &Flight) -> Result<HttpResponse, Failure> {
        let ctx = CallbackContext::default();
        self.exchange(request, flight, &ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_headers_are_split_lowercased_and_the_status_line_is_skipped() {
        let raw = "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After:  7 \r\nX-Odd: a: b\r\nbroken line\r\n: nameless\r\n\r\n";
        assert_eq!(
            parse_raw_headers(raw),
            vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("retry-after".to_string(), "7".to_string()),
                ("x-odd".to_string(), "a: b".to_string()),
            ]
        );
        assert!(parse_raw_headers("").is_empty());
        assert!(parse_raw_headers("HTTP/1.1 200 OK\r\n\r\n").is_empty());
    }
}
