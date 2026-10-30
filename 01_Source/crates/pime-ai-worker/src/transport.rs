//! The seam between the protocol engine and the network.
//!
//! The engine only knows this trait, so everything above it (validation, key injection, scrubbing, cancel, timeouts,
//! the framing) is tested with a fake. The real implementation is `winhttp::WinHttpTransport`.
#![forbid(unsafe_code)]

use crate::error::{Code, Failure};
use crate::policy::Target;
use crate::secret::{Secret, WideSecret};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// A fully validated request. The key, if any, is already part of `auth_line`; the transport only writes it into the
/// header block and must wipe its own copies afterwards.
pub struct PreparedRequest {
    pub method: Method,
    pub target: Target,
    pub headers: Vec<(String, String)>,
    /// `Name: value` of the credential header (without line terminator).
    pub auth_line: Option<Secret>,
    pub body: Vec<u8>,
    pub timeout: Duration,
    /// The test-only relaxation of the certificate authority check. The engine sets it only when the worker was
    /// started with `--insecure-test-ca` in test mode AND the target is a loopback host.
    pub accept_untrusted_ca: bool,
    pub max_response_bytes: usize,
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    /// Every response header with a lower-case name; the engine filters them.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    /// Reported by `health` (`winhttp`).
    fn name(&self) -> &'static str;

    /// Sends the request and reads the whole response. A 3xx answer is returned as a normal response (redirects are
    /// never followed). The implementation must stop promptly once `flight` is aborted; the engine decides between
    /// `CANCELLED` and `TIMEOUT` from the flight, whatever error the transport reports for the abort.
    fn send(&self, request: &PreparedRequest, flight: &Flight) -> Result<HttpResponse, Failure>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    Cancelled,
    Timeout,
}

impl AbortReason {
    pub fn code(self) -> Code {
        match self {
            AbortReason::Cancelled => Code::Cancelled,
            AbortReason::Timeout => Code::Timeout,
        }
    }
}

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const TIMED_OUT: u8 = 2;

/// The abort handle of one in-flight request, shared by the sender, the deadline timer and `http.cancel`.
///
/// The first `abort` wins and records its reason; the transport arms a hook that really stops the I/O (it closes the
/// WinHTTP request handle). `arm` returns `false` when the flight was aborted before the hook could be armed, and the
/// transport must then give up without doing any I/O.
pub struct Flight {
    state: AtomicU8,
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Default for Flight {
    fn default() -> Self {
        Flight::new()
    }
}

impl Flight {
    pub fn new() -> Flight {
        Flight {
            state: AtomicU8::new(RUNNING),
            hook: Mutex::new(None),
        }
    }

    /// Returns `true` when this call is the one that aborted the flight.
    pub fn abort(&self, reason: AbortReason) -> bool {
        let new = match reason {
            AbortReason::Cancelled => CANCELLED,
            AbortReason::Timeout => TIMED_OUT,
        };
        if self
            .state
            .compare_exchange(RUNNING, new, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        // run the hook outside the lock: closing a handle may block for a moment
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(hook) = hook {
            hook();
        }
        true
    }

    pub fn reason(&self) -> Option<AbortReason> {
        match self.state.load(Ordering::SeqCst) {
            CANCELLED => Some(AbortReason::Cancelled),
            TIMED_OUT => Some(AbortReason::Timeout),
            _ => None,
        }
    }

    pub fn is_aborted(&self) -> bool {
        self.reason().is_some()
    }

    /// Installs the hook that stops the I/O. `false` = already aborted, nothing was installed.
    pub fn arm(&self, hook: Box<dyn FnOnce() + Send>) -> bool {
        let mut slot = self.hook.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_aborted() {
            return false;
        }
        *slot = Some(hook);
        // an abort between the check and the store would have missed the hook: look again
        if self.is_aborted() {
            let hook = slot.take();
            drop(slot);
            if let Some(hook) = hook {
                hook();
            }
            return false;
        }
        true
    }

    /// Drops the hook after the I/O has finished normally.
    pub fn disarm(&self) {
        self.hook.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// Builds the request headers as one UTF-16 block `Name: value\r\n...` for WinHTTP, with the credential header last.
/// The block holds the key, so it is wiped on drop. Capacity is reserved up front so no stale partial copy is left
/// behind by a reallocation.
pub fn header_block(request: &PreparedRequest) -> WideSecret {
    let mut len = 0;
    for (name, value) in &request.headers {
        len += name.len() + 2 + value.len() + 2;
    }
    if let Some(auth) = &request.auth_line {
        len += auth.len() + 2;
    }
    let mut block: Vec<u16> = Vec::with_capacity(len);
    for (name, value) in &request.headers {
        block.extend(name.bytes().map(u16::from));
        block.extend(": ".bytes().map(u16::from));
        block.extend(value.bytes().map(u16::from));
        block.extend("\r\n".bytes().map(u16::from));
    }
    if let Some(auth) = &request.auth_line {
        block.extend(auth.expose().iter().map(|&b| u16::from(b)));
        block.extend("\r\n".bytes().map(u16::from));
    }
    WideSecret(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn request(auth: Option<&str>) -> PreparedRequest {
        PreparedRequest {
            method: Method::Post,
            target: Target {
                https: true,
                host: "api.example".to_string(),
                port: 443,
                path_and_query: "/v1".to_string(),
            },
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("anthropic-version".to_string(), "2026-10-21".to_string()),
            ],
            auth_line: auth.map(|a| Secret::from_bytes(a.as_bytes().to_vec())),
            body: Vec::new(),
            timeout: Duration::from_secs(1),
            accept_untrusted_ca: false,
            max_response_bytes: 1024,
        }
    }

    #[test]
    fn the_header_block_has_the_credential_header_last_and_crlf_everywhere() {
        let block = header_block(&request(Some("x-api-key: mock-key-test-123")));
        let text = String::from_utf16(&block.0).unwrap();
        assert_eq!(
            text,
            "Content-Type: application/json\r\nanthropic-version: 2026-10-21\r\nx-api-key: mock-key-test-123\r\n"
        );
        let plain = String::from_utf16(&header_block(&request(None)).0).unwrap();
        assert!(!plain.contains("x-api-key"));
        assert_eq!(plain.matches("\r\n").count(), 2);
    }

    #[test]
    fn the_header_block_has_exactly_the_capacity_it_needs() {
        let block = header_block(&request(Some("Authorization: Bearer mock-key-test-123")));
        assert_eq!(block.0.capacity(), block.0.len());
    }

    #[test]
    fn the_first_abort_wins_and_runs_the_hook_once() {
        let flight = Flight::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        assert!(flight.arm(Box::new(move || {
            r.fetch_add(1, Ordering::SeqCst);
        })));
        assert_eq!(flight.reason(), None);
        assert!(flight.abort(AbortReason::Timeout));
        assert!(
            !flight.abort(AbortReason::Cancelled),
            "the second abort is a no-op"
        );
        assert_eq!(flight.reason(), Some(AbortReason::Timeout));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn arming_an_aborted_flight_fails_and_never_installs_the_hook() {
        let flight = Flight::new();
        assert!(flight.abort(AbortReason::Cancelled));
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        assert!(!flight.arm(Box::new(move || {
            r.fetch_add(1, Ordering::SeqCst);
        })));
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_disarmed_hook_is_not_run_by_a_later_abort() {
        let flight = Flight::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        assert!(flight.arm(Box::new(move || {
            r.fetch_add(1, Ordering::SeqCst);
        })));
        flight.disarm();
        assert!(flight.abort(AbortReason::Cancelled));
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn abort_reasons_map_to_the_public_tokens() {
        assert_eq!(AbortReason::Cancelled.code(), Code::Cancelled);
        assert_eq!(AbortReason::Timeout.code(), Code::Timeout);
    }
}
