//! The protocol engine: one JSON Lines request in, one reply out, over any transport and credential store.
//!
//! Framing and error conventions follow `pime-core` (an optional integer `id` is echoed; failures are
//! `{"ok":false,"error":"TOKEN"[,"field":"name"]}`; lines are bounded). Differences that matter and are deliberate:
//! - `http.send` takes seconds, so it runs on its own thread and its reply is matched to the request by `id`, not by
//!   position; `id` is therefore mandatory for `http.send` and must be unique among the requests in flight;
//! - every other operation answers immediately on the reading thread, before the next line is read, so an
//!   `http.cancel` always finds the `http.send` that precedes it on the pipe;
//! - unknown request fields are refused (a mistyped `credentialRef` must not silently send a request without a key).
//!
//! The engine never logs. It writes only reply lines, and the only free text in a reply is the provider's own
//! response body (with the key scrubbed out of it).
#![forbid(unsafe_code)]

use crate::config::Config;
use crate::error::{Code, Failure};
use crate::framing::{LineRead, read_bounded_line};
use crate::policy::{
    AuthTemplate, UrlPolicy, parse_origin, parse_url, response_header_allowed, valid_provider_ref,
    validate_request_headers,
};
use crate::redact::scrub_exact;
use crate::secret::{Secret, wipe_vec};
use crate::transport::{AbortReason, Flight, HttpResponse, Method, PreparedRequest, Transport};
use crate::vault::{Vault, VaultError};
use pime_core::json::{self, Json, JsonErrorKind, Limits};
use pime_core::{MAX_BULK_LINE_BYTES, MAX_LINE_BYTES};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const NAME: &str = "pime-ai-worker";
pub const VERSION: &str = "0.2.0";
/// The framing and conventions are those of pime-core's protocol 0.3.
pub const PROTOCOL: &str = "0.3";
pub const OPS: [&str; 6] = [
    "health",
    "http.send",
    "http.cancel",
    "cred.set",
    "cred.exists",
    "cred.delete",
];

pub const MAX_IN_FLIGHT: usize = 8;
/// The same cap the JavaScript client applied to a response body.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_TIMEOUT_MS: i64 = 30_000;
pub const MAX_TIMEOUT_MS: i64 = 300_000;

const SMALL_LIMITS: Limits = Limits {
    max_bytes: MAX_LINE_BYTES,
    max_depth: 16,
};
const BULK_LIMITS: Limits = Limits {
    max_bytes: MAX_BULK_LINE_BYTES,
    max_depth: 16,
};

struct Inner {
    config: Config,
    transport: Arc<dyn Transport>,
    vault: Arc<dyn Vault>,
    out: Mutex<Box<dyn Write + Send>>,
    /// Requests whose `http.send` has not produced a reply yet; removed just before the reply is written, so a client
    /// may reuse an id as soon as it has read the reply.
    flights: Mutex<HashMap<i64, Arc<Flight>>>,
    /// Sender threads that have not finished (reply written). Used by `wait_idle` and `shutdown`.
    active: AtomicUsize,
    idle: (Mutex<()>, Condvar),
}

/// One worker: configuration, transport, credential store and the output stream.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

fn reply_ok(id: Option<i64>, rest: Vec<(&str, Json)>) -> Json {
    let mut pairs = vec![("ok", Json::Bool(true))];
    if let Some(i) = id {
        pairs.push(("id", Json::int(i)));
    }
    pairs.extend(rest);
    Json::obj(pairs)
}

fn reply_err(id: Option<i64>, failure: &Failure) -> Json {
    let mut pairs = vec![("ok", Json::Bool(false))];
    if let Some(i) = id {
        pairs.push(("id", Json::int(i)));
    }
    pairs.push(("error", Json::str(failure.code.as_str())));
    if let Some(field) = &failure.field {
        pairs.push(("field", Json::str(field)));
    }
    if let Some(code) = failure.os_code {
        pairs.push(("winError", Json::int(i64::from(code))));
    }
    Json::obj(pairs)
}

fn vault_failure(e: VaultError) -> Failure {
    let mut f = Failure::new(Code::VaultError);
    if e.0 != 0 {
        f.os_code = Some(e.0);
    }
    f
}

/// Takes the string out of an object field (so the caller owns the only copy). Absent or null -> `None`.
fn take_string(req: &mut Json, key: &str) -> Result<Option<String>, ()> {
    let Json::Obj(pairs) = req else {
        return Ok(None);
    };
    match pairs.iter_mut().find(|(k, _)| k == key) {
        None | Some((_, Json::Null)) => Ok(None),
        Some((_, Json::Str(s))) => Ok(Some(std::mem::take(s))),
        Some(_) => Err(()),
    }
}

fn opt_str<'a>(req: &'a Json, key: &str) -> Result<Option<&'a str>, Failure> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s)),
        Some(_) => Err(Failure::invalid(key)),
    }
}

fn req_str<'a>(req: &'a Json, key: &str) -> Result<&'a str, Failure> {
    opt_str(req, key)?.ok_or_else(|| Failure::invalid(key))
}

fn check_keys(req: &Json, allowed: &[&str]) -> Result<(), Failure> {
    for (k, _) in req.as_object().map(Vec::as_slice).unwrap_or(&[]) {
        if !allowed.contains(&k.as_str()) {
            return Err(Failure::invalid(k));
        }
    }
    Ok(())
}

fn parse_headers(req: &Json) -> Result<Vec<(String, String)>, Failure> {
    let pairs = match req.get("headers") {
        None | Some(Json::Null) => return Ok(Vec::new()),
        Some(Json::Obj(o)) => o,
        Some(_) => return Err(Failure::invalid("headers")),
    };
    let mut out = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        let Json::Str(v) = v else {
            return Err(Failure::invalid("headers"));
        };
        out.push((k.clone(), v.clone()));
    }
    validate_request_headers(&out)
}

impl Inner {
    fn write(&self, reply: &Json) {
        let mut line = reply.to_json_string();
        line.push('\n');
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        // a closed output only means the other side is gone; the read loop ends on its own
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    fn target_name(&self, provider: &str) -> String {
        format!("{}/{}", self.config.namespace, provider)
    }

    fn begin_active(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
    }

    fn end_active(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        let _guard = self.idle.0.lock().unwrap_or_else(|e| e.into_inner());
        self.idle.1.notify_all();
    }

    fn unregister(&self, id: i64) {
        self.flights
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    fn register(&self, id: i64) -> Result<Arc<Flight>, Failure> {
        let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
        if flights.contains_key(&id) {
            return Err(Failure::invalid("id"));
        }
        if flights.len() >= MAX_IN_FLIGHT {
            return Err(Failure::new(Code::Busy));
        }
        let flight = Arc::new(Flight::new());
        flights.insert(id, flight.clone());
        Ok(flight)
    }

    // ------------------------------------------------------------------------------------------------- operations

    fn health(&self, id: Option<i64>) -> Json {
        reply_ok(
            id,
            vec![
                ("name", Json::str(NAME)),
                ("version", Json::str(VERSION)),
                ("protocol", Json::str(PROTOCOL)),
                ("transport", Json::str(self.transport.name())),
                ("ops", Json::strs(&OPS)),
                ("namespace", Json::str(&self.config.namespace)),
                ("testMode", Json::Bool(self.config.test_mode)),
                ("insecureTestCa", Json::Bool(self.config.insecure_test_ca)),
                (
                    "allowLoopbackHttp",
                    Json::Bool(self.config.allow_loopback_http),
                ),
                (
                    "limits",
                    Json::obj(vec![
                        ("maxInFlight", Json::int(MAX_IN_FLIGHT as i64)),
                        ("maxResponseBytes", Json::int(MAX_RESPONSE_BYTES as i64)),
                        (
                            "maxRequestBodyBytes",
                            Json::int(MAX_REQUEST_BODY_BYTES as i64),
                        ),
                    ]),
                ),
            ],
        )
    }

    fn prepare_send(&self, req: &mut Json) -> Result<(PreparedRequest, Option<Secret>), Failure> {
        check_keys(
            req,
            &[
                "op",
                "id",
                "method",
                "url",
                "headers",
                "body",
                "timeoutMs",
                "credentialRef",
                "authHeader",
            ],
        )?;
        let method = match req_str(req, "method")? {
            "GET" => Method::Get,
            "POST" => Method::Post,
            _ => return Err(Failure::invalid("method")),
        };
        let target = parse_url(
            req_str(req, "url")?,
            UrlPolicy {
                allow_loopback_http: self.config.allow_loopback_http,
            },
        )?;
        let headers = parse_headers(req)?;
        let timeout_ms = match req.get("timeoutMs") {
            None | Some(Json::Null) => DEFAULT_TIMEOUT_MS,
            Some(v) => match v.as_i64() {
                Some(ms) if (1..=MAX_TIMEOUT_MS).contains(&ms) => ms,
                _ => return Err(Failure::invalid("timeoutMs")),
            },
        };
        let body = take_string(req, "body")
            .map_err(|()| Failure::invalid("body"))?
            .unwrap_or_default();
        if body.len() > MAX_REQUEST_BODY_BYTES || (method == Method::Get && !body.is_empty()) {
            return Err(Failure::invalid("body"));
        }
        let (auth_line, key) = match (opt_str(req, "credentialRef")?, opt_str(req, "authHeader")?) {
            (None, None) => (None, None),
            (Some(_), None) => return Err(Failure::invalid("authHeader")),
            (None, Some(_)) => return Err(Failure::invalid("credentialRef")),
            (Some(provider), Some(template)) => {
                if !valid_provider_ref(provider) {
                    return Err(Failure::invalid("credentialRef"));
                }
                let template = AuthTemplate::parse(template)?;
                let stored = self
                    .vault
                    .read(&self.target_name(provider))
                    .map_err(vault_failure)?
                    .ok_or_else(|| Failure::with_field(Code::NoCredential, "credentialRef"))?;
                // the key may only go where it was stored for
                if stored.origin != target.origin() {
                    return Err(Failure::with_field(Code::CredentialOriginMismatch, "url"));
                }
                (Some(template.render(&stored.secret)), Some(stored.secret))
            }
        };
        let accept_untrusted_ca = self.config.insecure_test_ca && target.is_loopback();
        Ok((
            PreparedRequest {
                method,
                target,
                headers,
                auth_line,
                body: body.into_bytes(),
                timeout: Duration::from_millis(timeout_ms as u64),
                accept_untrusted_ca,
                max_response_bytes: MAX_RESPONSE_BYTES,
            },
            key,
        ))
    }

    fn http_send(self: &Arc<Self>, id: Option<i64>, req: &mut Json) {
        let Some(id) = id else {
            self.write(&reply_err(None, &Failure::invalid("id")));
            return;
        };
        let (prepared, key) = match self.prepare_send(req) {
            Ok(v) => v,
            Err(f) => {
                self.write(&reply_err(Some(id), &f));
                return;
            }
        };
        let flight = match self.register(id) {
            Ok(f) => f,
            Err(f) => {
                self.write(&reply_err(Some(id), &f));
                return;
            }
        };
        self.begin_active();
        let inner = self.clone();
        let spawned = std::thread::Builder::new()
            .name("pime-http".to_string())
            .spawn(move || Inner::run_send(inner, id, prepared, key, flight));
        if spawned.is_err() {
            self.unregister(id);
            self.end_active();
            self.write(&reply_err(Some(id), &Failure::new(Code::Internal)));
        }
    }

    fn run_send(
        inner: Arc<Inner>,
        id: i64,
        prepared: PreparedRequest,
        key: Option<Secret>,
        flight: Arc<Flight>,
    ) {
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            inner.exchange(&prepared, key.as_ref(), &flight, started)
        }));
        let reply = match outcome {
            Ok(Ok(fields)) => reply_ok(Some(id), fields),
            Ok(Err(failure)) => reply_err(Some(id), &failure),
            Err(_) => reply_err(Some(id), &Failure::new(Code::Internal)),
        };
        inner.unregister(id);
        inner.write(&reply);
        // the key and the header line are wiped here, after the reply is out
        drop(key);
        drop(prepared);
        inner.end_active();
    }

    /// Sends the request under a deadline timer and turns the result into reply fields.
    fn exchange(
        &self,
        prepared: &PreparedRequest,
        key: Option<&Secret>,
        flight: &Flight,
        started: Instant,
    ) -> Result<Vec<(&'static str, Json)>, Failure> {
        let timeout = prepared.timeout;
        let result = std::thread::scope(|scope| {
            let (done, wait) = mpsc::channel::<()>();
            let timer = std::thread::Builder::new()
                .name("pime-deadline".to_string())
                .spawn_scoped(scope, move || {
                    if let Err(RecvTimeoutError::Timeout) = wait.recv_timeout(timeout) {
                        flight.abort(AbortReason::Timeout);
                    }
                });
            if timer.is_err() {
                return Err(Failure::new(Code::Internal));
            }
            let r = self.transport.send(prepared, flight);
            drop(done); // wakes the timer thread, which then ends
            r
        });
        match result {
            Ok(response) => response_fields(response, key, started),
            // the engine, not the transport, knows whether the abort was a cancel or the deadline
            Err(failure) => Err(match flight.reason() {
                Some(reason) => Failure::new(reason.code()),
                None => failure,
            }),
        }
    }

    fn http_cancel(&self, id: Option<i64>, req: &Json) -> Json {
        if let Err(f) = check_keys(req, &["op", "id", "request"]) {
            return reply_err(id, &f);
        }
        let Some(target) = req.get("request").and_then(Json::as_i64) else {
            return reply_err(id, &Failure::invalid("request"));
        };
        let flight = self
            .flights
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&target)
            .cloned();
        let cancelled = flight.is_some_and(|f| f.abort(AbortReason::Cancelled));
        reply_ok(id, vec![("cancelled", Json::Bool(cancelled))])
    }

    fn cred_set(&self, id: Option<i64>, req: &mut Json) -> Json {
        // the key leaves the parsed request first, so that every early return below drops (and wipes) it
        let secret = match take_string(req, "secret") {
            Ok(Some(text)) => Secret::parse(text).ok(),
            _ => None,
        };
        let result = (|| {
            check_keys(req, &["op", "id", "credentialRef", "secret", "host"])?;
            let provider = req_str(req, "credentialRef")?;
            if !valid_provider_ref(provider) {
                return Err(Failure::invalid("credentialRef"));
            }
            let origin =
                parse_origin(req_str(req, "host")?).ok_or_else(|| Failure::invalid("host"))?;
            let secret = secret.as_ref().ok_or_else(|| Failure::invalid("secret"))?;
            self.vault
                .set(&self.target_name(provider), &origin, secret)
                .map_err(vault_failure)?;
            Ok(origin)
        })();
        match result {
            Ok(origin) => reply_ok(
                id,
                vec![("stored", Json::Bool(true)), ("host", Json::str(&origin))],
            ),
            Err(f) => reply_err(id, &f),
        }
    }

    fn cred_exists(&self, id: Option<i64>, req: &Json) -> Json {
        let result = (|| {
            check_keys(req, &["op", "id", "credentialRef"])?;
            let provider = req_str(req, "credentialRef")?;
            if !valid_provider_ref(provider) {
                return Err(Failure::invalid("credentialRef"));
            }
            self.vault
                .exists(&self.target_name(provider))
                .map_err(vault_failure)
        })();
        match result {
            Ok(Some(origin)) => reply_ok(
                id,
                vec![("exists", Json::Bool(true)), ("host", Json::str(&origin))],
            ),
            Ok(None) => reply_ok(id, vec![("exists", Json::Bool(false))]),
            Err(f) => reply_err(id, &f),
        }
    }

    fn cred_delete(&self, id: Option<i64>, req: &Json) -> Json {
        let result = (|| {
            check_keys(req, &["op", "id", "credentialRef"])?;
            let provider = req_str(req, "credentialRef")?;
            if !valid_provider_ref(provider) {
                return Err(Failure::invalid("credentialRef"));
            }
            self.vault
                .delete(&self.target_name(provider))
                .map_err(vault_failure)
        })();
        match result {
            Ok(deleted) => reply_ok(id, vec![("deleted", Json::Bool(deleted))]),
            Err(f) => reply_err(id, &f),
        }
    }
}

/// The reply fields of a finished exchange. 3xx is refused (redirects are never followed: the credential header must not
/// travel to another host); the response size is checked again here as a second line behind the transport's own cap;
/// the key is scrubbed from the body and from every header that is passed on.
fn response_fields(
    response: HttpResponse,
    key: Option<&Secret>,
    started: Instant,
) -> Result<Vec<(&'static str, Json)>, Failure> {
    if !(100..=599).contains(&response.status) {
        return Err(Failure::new(Code::BadResponse));
    }
    if (300..=399).contains(&response.status) {
        return Err(Failure::new(Code::RedirectBlocked));
    }
    if response.body.len() > MAX_RESPONSE_BYTES {
        return Err(Failure::new(Code::ResponseTooLarge));
    }
    let key_bytes = key.map(Secret::expose).unwrap_or(&[]);
    let mut redactions = 0;
    let mut headers: Vec<(String, String)> = Vec::new();
    for (name, value) in &response.headers {
        if !response_header_allowed(name) {
            continue;
        }
        let (clean, n) = scrub_exact(value.as_bytes(), key_bytes);
        redactions += n;
        let clean = String::from_utf8_lossy(&clean).into_owned();
        match headers.iter_mut().find(|(k, _)| k == name) {
            Some((_, existing)) => {
                existing.push_str(", ");
                existing.push_str(&clean);
            }
            None => headers.push((name.clone(), clean)),
        }
    }
    let (body, n) = scrub_exact(&response.body, key_bytes);
    redactions += n;
    Ok(vec![
        ("status", Json::int(i64::from(response.status))),
        (
            "headers",
            Json::Obj(
                headers
                    .into_iter()
                    .map(|(k, v)| (k, Json::Str(v)))
                    .collect(),
            ),
        ),
        (
            "body",
            Json::Str(String::from_utf8_lossy(&body).into_owned()),
        ),
        ("bodyBytes", Json::int(response.body.len() as i64)),
        ("redactions", Json::int(redactions as i64)),
        ("elapsedMs", Json::int(started.elapsed().as_millis() as i64)),
    ])
}

impl Engine {
    pub fn new(
        config: Config,
        transport: Arc<dyn Transport>,
        vault: Arc<dyn Vault>,
        out: Box<dyn Write + Send>,
    ) -> Engine {
        Engine {
            inner: Arc::new(Inner {
                config,
                transport,
                vault,
                out: Mutex::new(out),
                flights: Mutex::new(HashMap::new()),
                active: AtomicUsize::new(0),
                idle: (Mutex::new(()), Condvar::new()),
            }),
        }
    }

    /// Handles one request line. The buffer is wiped before returning when the line is small enough to be a credential
    /// operation (a `cred.set` line contains a key; bulk lines are request bodies and are left alone).
    pub fn handle_line(&self, line: &mut Vec<u8>) {
        let small = line.len() <= MAX_LINE_BYTES;
        self.handle_line_inner(line);
        if small {
            wipe_vec(line);
        }
    }

    fn handle_line_inner(&self, line: &[u8]) {
        let inner = &self.inner;
        let Ok(text) = std::str::from_utf8(line) else {
            inner.write(&reply_err(None, &Failure::new(Code::InvalidJson)));
            return;
        };
        let big = line.len() > MAX_LINE_BYTES;
        let parsed = json::parse(text, if big { BULK_LIMITS } else { SMALL_LIMITS });
        let mut req = match parsed {
            Ok(r) => r,
            Err(e) if e.kind == JsonErrorKind::TooLarge => {
                inner.write(&reply_err(None, &Failure::new(Code::PayloadTooLarge)));
                return;
            }
            Err(_) if big => {
                inner.write(&reply_err(None, &Failure::new(Code::PayloadTooLarge)));
                return;
            }
            Err(_) => {
                inner.write(&reply_err(None, &Failure::new(Code::InvalidJson)));
                return;
            }
        };
        if req.as_object().is_none() {
            inner.write(&reply_err(None, &Failure::new(Code::InvalidRequest)));
            return;
        }
        let id = match req.get("id") {
            None | Some(Json::Null) => None,
            Some(v) => match v.as_i64() {
                Some(i) => Some(i),
                None => {
                    inner.write(&reply_err(None, &Failure::invalid("id")));
                    return;
                }
            },
        };
        let op = req
            .get("op")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        if big && op != "http.send" {
            inner.write(&reply_err(None, &Failure::new(Code::PayloadTooLarge)));
            return;
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| match op.as_str() {
            "health" => Some(inner.health(id)),
            "http.send" => {
                inner.http_send(id, &mut req);
                None
            }
            "http.cancel" => Some(inner.http_cancel(id, &req)),
            "cred.set" => Some(inner.cred_set(id, &mut req)),
            "cred.exists" => Some(inner.cred_exists(id, &req)),
            "cred.delete" => Some(inner.cred_delete(id, &req)),
            _ => Some(reply_err(id, &Failure::new(Code::UnknownOperation))),
        }));
        match outcome {
            Ok(Some(reply)) => inner.write(&reply),
            Ok(None) => {}
            Err(_) => inner.write(&reply_err(id, &Failure::new(Code::Internal))),
        }
    }

    /// The reply to a line that exceeded even the bulk limit.
    pub fn reject_too_long(&self) {
        self.inner
            .write(&reply_err(None, &Failure::new(Code::PayloadTooLarge)));
    }

    /// Waits until every sender thread has written its reply. `false` on timeout.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.idle.0.lock().unwrap_or_else(|e| e.into_inner());
        while self.inner.active.load(Ordering::SeqCst) > 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (g, _) = self
                .inner
                .idle
                .1
                .wait_timeout(guard, (deadline - now).min(Duration::from_millis(50)))
                .unwrap_or_else(|e| e.into_inner());
            guard = g;
        }
        true
    }

    /// Aborts every request in flight (their replies are `CANCELLED`) and waits for the replies to be written.
    pub fn shutdown(&self, grace: Duration) -> bool {
        let flights: Vec<Arc<Flight>> = self
            .inner
            .flights
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for f in flights {
            f.abort(AbortReason::Cancelled);
        }
        self.wait_idle(grace)
    }
}

/// Serves requests until the input ends, then aborts what is still in flight: a closed input means nobody reads the
/// replies any more (the parent exited or closed the pipe).
pub fn serve<R: BufRead>(engine: &Engine, mut input: R) -> io::Result<()> {
    let mut buf = Vec::new();
    let result = loop {
        match read_bounded_line(&mut input, &mut buf, MAX_BULK_LINE_BYTES) {
            Ok(LineRead::Eof) => break Ok(()),
            Ok(LineRead::TooLong) => engine.reject_too_long(),
            Ok(LineRead::Line) => engine.handle_line(&mut buf),
            Err(e) => break Err(e),
        }
    };
    engine.shutdown(Duration::from_secs(5));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::MemVault;
    use std::io::Cursor;
    use std::thread::sleep;

    const KEY: &str = "mock-key-test-123";

    struct Seen {
        method: Method,
        host: String,
        port: u16,
        path: String,
        headers: Vec<(String, String)>,
        auth_line: Option<String>,
        body: Vec<u8>,
        accept_untrusted_ca: bool,
        timeout: Duration,
    }

    type Handler = dyn Fn(&PreparedRequest, &Flight) -> Result<HttpResponse, Failure> + Send + Sync;

    struct Fake {
        handler: Box<Handler>,
        seen: Mutex<Vec<Seen>>,
    }

    impl Transport for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn send(&self, r: &PreparedRequest, flight: &Flight) -> Result<HttpResponse, Failure> {
            self.seen.lock().unwrap().push(Seen {
                method: r.method,
                host: r.target.host.clone(),
                port: r.target.port,
                path: r.target.path_and_query.clone(),
                headers: r.headers.clone(),
                auth_line: r
                    .auth_line
                    .as_ref()
                    .map(|s| String::from_utf8(s.expose().to_vec()).unwrap()),
                body: r.body.clone(),
                accept_untrusted_ca: r.accept_untrusted_ca,
                timeout: r.timeout,
            });
            (self.handler)(r, flight)
        }
    }

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Harness {
        engine: Engine,
        fake: Arc<Fake>,
        vault: Arc<MemVault>,
        out: Capture,
    }

    fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// Emulates a server that never answers: returns when the flight is aborted (like a closed WinHTTP handle).
    fn hang(_: &PreparedRequest, flight: &Flight) -> Result<HttpResponse, Failure> {
        let t0 = Instant::now();
        while !flight.is_aborted() {
            if t0.elapsed() > Duration::from_secs(20) {
                return Err(Failure::new(Code::TransportError));
            }
            sleep(Duration::from_millis(2));
        }
        Err(Failure::new(Code::Cancelled))
    }

    impl Harness {
        fn new<F>(handler: F) -> Harness
        where
            F: Fn(&PreparedRequest, &Flight) -> Result<HttpResponse, Failure>
                + Send
                + Sync
                + 'static,
        {
            Harness::with_config(Config::default(), handler)
        }

        fn with_config<F>(config: Config, handler: F) -> Harness
        where
            F: Fn(&PreparedRequest, &Flight) -> Result<HttpResponse, Failure>
                + Send
                + Sync
                + 'static,
        {
            let fake = Arc::new(Fake {
                handler: Box::new(handler),
                seen: Mutex::new(Vec::new()),
            });
            let vault = Arc::new(MemVault::new());
            let out = Capture::default();
            let engine = Engine::new(config, fake.clone(), vault.clone(), Box::new(out.clone()));
            Harness {
                engine,
                fake,
                vault,
                out,
            }
        }

        fn line(&self, text: &str) {
            self.engine.handle_line(&mut text.as_bytes().to_vec());
        }

        fn wait(&self) {
            assert!(
                self.engine.wait_idle(Duration::from_secs(10)),
                "senders did not finish"
            );
        }

        fn raw(&self) -> String {
            String::from_utf8(self.out.0.lock().unwrap().clone()).unwrap()
        }

        fn replies(&self) -> Vec<Json> {
            self.raw()
                .lines()
                .map(|l| json::parse(l, BULK_LIMITS).expect("every output line is JSON"))
                .collect()
        }

        fn try_reply(&self, id: i64) -> Option<Json> {
            self.replies()
                .into_iter()
                .find(|r| r.get("id").and_then(Json::as_i64) == Some(id))
        }

        fn reply(&self, id: i64) -> Json {
            let t0 = Instant::now();
            loop {
                if let Some(r) = self.try_reply(id) {
                    return r;
                }
                assert!(
                    t0.elapsed() < Duration::from_secs(10),
                    "no reply for id {id}: {}",
                    self.raw()
                );
                sleep(Duration::from_millis(2));
            }
        }

        fn seen_count(&self) -> usize {
            self.fake.seen.lock().unwrap().len()
        }

        fn wait_for_requests(&self, n: usize) {
            let t0 = Instant::now();
            while self.seen_count() < n {
                assert!(
                    t0.elapsed() < Duration::from_secs(10),
                    "the transport did not start"
                );
                sleep(Duration::from_millis(2));
            }
        }

        fn store_key(&self, provider: &str, host: &str) {
            let line = Json::obj(vec![
                ("op", Json::str("cred.set")),
                ("id", Json::int(9000)),
                ("credentialRef", Json::str(provider)),
                ("host", Json::str(host)),
                ("secret", Json::str(KEY)),
            ])
            .to_json_string();
            self.line(&line);
            assert_eq!(
                self.reply(9000).get("ok"),
                Some(&Json::Bool(true)),
                "{}",
                self.raw()
            );
        }
    }

    fn send_line(id: i64, method: &str, url: &str, extra: Vec<(&str, Json)>) -> String {
        let mut pairs = vec![
            ("op", Json::str("http.send")),
            ("id", Json::int(id)),
            ("method", Json::str(method)),
            ("url", Json::str(url)),
        ];
        pairs.extend(extra);
        Json::obj(pairs).to_json_string()
    }

    fn bearer(provider: &str) -> Vec<(&'static str, Json)> {
        vec![
            ("credentialRef", Json::str(provider)),
            ("authHeader", Json::str("Authorization: Bearer {key}")),
        ]
    }

    fn code(reply: &Json) -> &str {
        reply.get("error").and_then(Json::as_str).unwrap_or("")
    }

    fn is_ok(reply: &Json) -> bool {
        reply.get("ok") == Some(&Json::Bool(true))
    }

    #[test]
    fn ut_031_http_send_injects_the_key_and_no_output_line_contains_it() {
        let h = Harness::new(|_, _| {
            Ok(response(
                200,
                &[("content-type", "application/json")],
                r#"{"output_text":"hi"}"#,
            ))
        });
        h.store_key("openai", "api.openai.com");
        let mut extra = bearer("openai");
        extra.push((
            "headers",
            Json::obj(vec![("Content-Type", Json::str("application/json"))]),
        ));
        extra.push(("body", Json::str(r#"{"model":"m"}"#)));
        h.line(&send_line(
            1,
            "POST",
            "https://api.openai.com/v1/responses",
            extra,
        ));
        h.wait();
        let r = h.reply(1);
        assert!(is_ok(&r), "{}", h.raw());
        assert_eq!(r.get("status").and_then(Json::as_i64), Some(200));
        assert_eq!(
            r.get("body").and_then(Json::as_str),
            Some(r#"{"output_text":"hi"}"#)
        );
        assert_eq!(
            r.get("headers")
                .and_then(|h| h.get("content-type"))
                .and_then(Json::as_str),
            Some("application/json")
        );
        assert_eq!(h.seen_count(), 1);
        let seen = h.fake.seen.lock().unwrap();
        assert_eq!(
            seen[0].auth_line.as_deref(),
            Some("Authorization: Bearer mock-key-test-123")
        );
        assert_eq!(seen[0].method, Method::Post);
        assert_eq!(
            (seen[0].host.as_str(), seen[0].port),
            ("api.openai.com", 443)
        );
        assert_eq!(seen[0].path, "/v1/responses");
        assert_eq!(seen[0].body, br#"{"model":"m"}"#);
        assert_eq!(
            seen[0].headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );
        // the positive control above proves the key reached the transport; now it must be absent from everything that left
        assert!(
            !h.raw().contains(KEY),
            "the key appeared in a reply: {}",
            h.raw()
        );
        assert_eq!(h.vault.targets(), vec!["ProgrammingIME/openai".to_string()]);
    }

    #[test]
    fn the_x_api_key_form_is_injected_too() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.store_key("anthropic", "api.anthropic.com");
        let extra = vec![
            ("credentialRef", Json::str("anthropic")),
            ("authHeader", Json::str("x-api-key: {key}")),
            (
                "headers",
                Json::obj(vec![("anthropic-version", Json::str("2026-10-21"))]),
            ),
        ];
        h.line(&send_line(
            2,
            "POST",
            "https://api.anthropic.com/v1/messages",
            extra,
        ));
        h.wait();
        assert!(is_ok(&h.reply(2)));
        assert_eq!(
            h.fake.seen.lock().unwrap()[0].auth_line.as_deref(),
            Some("x-api-key: mock-key-test-123")
        );
        assert!(!h.raw().contains(KEY));
    }

    #[test]
    fn a_request_without_a_credential_reference_carries_no_credential() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line(&send_line(
            3,
            "GET",
            "https://models.example/v1/models",
            vec![],
        ));
        h.wait();
        assert!(is_ok(&h.reply(3)));
        assert_eq!(h.fake.seen.lock().unwrap()[0].auth_line, None);
        assert_eq!(h.fake.seen.lock().unwrap()[0].method, Method::Get);
    }

    #[test]
    fn ut_036_rate_limit_and_server_errors_pass_through_with_the_headers_the_product_reads() {
        let h = Harness::new(|r, _| {
            let status = if r.target.path_and_query.ends_with("429") {
                429
            } else {
                500
            };
            Ok(response(
                status,
                &[
                    ("retry-after", "7"),
                    ("set-cookie", "session=abc; HttpOnly"),
                    ("www-authenticate", "Bearer realm=x"),
                    ("x-ratelimit-remaining-requests", "0"),
                    ("server", "mock"),
                ],
                r#"{"error":"mock-error"}"#,
            ))
        });
        h.line(&send_line(
            4,
            "POST",
            "https://api.example.com/limit/429",
            vec![],
        ));
        h.line(&send_line(
            5,
            "POST",
            "https://api.example.com/fail/500",
            vec![],
        ));
        h.wait();
        let r = h.reply(4);
        assert!(
            is_ok(&r),
            "a 429 is a normal reply: the caller maps the status"
        );
        assert_eq!(r.get("status").and_then(Json::as_i64), Some(429));
        let headers = r.get("headers").unwrap();
        assert_eq!(headers.get("retry-after").and_then(Json::as_str), Some("7"));
        assert_eq!(
            headers
                .get("x-ratelimit-remaining-requests")
                .and_then(Json::as_str),
            Some("0")
        );
        assert!(headers.get("set-cookie").is_none() && headers.get("www-authenticate").is_none());
        assert!(headers.get("server").is_none());
        assert_eq!(h.reply(5).get("status").and_then(Json::as_i64), Some(500));
        assert!(!h.raw().contains("session=abc"));
    }

    #[test]
    fn a_provider_that_echoes_the_key_has_it_scrubbed_from_body_and_headers() {
        let h = Harness::new(|_, _| {
            Ok(response(
                401,
                &[("x-request-id", "req-mock-key-test-123-end")],
                r#"{"error":"Incorrect API key provided: mock-key-test-123. See mock-key-test-123"}"#,
            ))
        });
        h.store_key("openai", "api.openai.com");
        h.line(&send_line(
            6,
            "POST",
            "https://api.openai.com/v1/responses",
            bearer("openai"),
        ));
        h.wait();
        let r = h.reply(6);
        assert!(is_ok(&r));
        assert_eq!(r.get("redactions").and_then(Json::as_i64), Some(3));
        assert_eq!(
            r.get("body").and_then(Json::as_str),
            Some(r#"{"error":"Incorrect API key provided: [redacted]. See [redacted]"}"#)
        );
        assert_eq!(
            r.get("headers")
                .and_then(|h| h.get("x-request-id"))
                .and_then(Json::as_str),
            Some("req-[redacted]-end")
        );
        assert!(!h.raw().contains(KEY));
    }

    #[test]
    fn redirects_are_blocked_and_their_target_is_not_reported() {
        let h = Harness::new(|_, _| {
            Ok(response(
                302,
                &[("location", "https://evil.example/steal")],
                "",
            ))
        });
        h.store_key("openai", "api.openai.com");
        h.line(&send_line(
            7,
            "POST",
            "https://api.openai.com/v1/responses",
            bearer("openai"),
        ));
        h.wait();
        let r = h.reply(7);
        assert_eq!((code(&r), is_ok(&r)), ("REDIRECT_BLOCKED", false));
        assert!(!h.raw().contains("evil.example"));
    }

    #[test]
    fn the_deadline_aborts_a_hanging_exchange_with_timeout() {
        let h = Harness::new(hang);
        h.line(&send_line(
            8,
            "POST",
            "https://api.example.com/v1",
            vec![("timeoutMs", Json::int(60))],
        ));
        let t0 = Instant::now();
        let r = h.reply(8);
        assert_eq!(code(&r), "TIMEOUT");
        assert!(t0.elapsed() < Duration::from_secs(5));
        h.wait();
    }

    #[test]
    fn http_cancel_aborts_the_request_in_flight_and_both_replies_are_correct() {
        let h = Harness::new(hang);
        h.line(&send_line(
            10,
            "POST",
            "https://api.example.com/v1",
            vec![("timeoutMs", Json::int(30_000))],
        ));
        h.wait_for_requests(1);
        h.line(r#"{"op":"http.cancel","id":11,"request":10}"#);
        let cancel = h.reply(11);
        assert!(is_ok(&cancel));
        assert_eq!(cancel.get("cancelled"), Some(&Json::Bool(true)));
        let r = h.reply(10);
        assert_eq!(code(&r), "CANCELLED");
        h.wait();
        // the request is gone: cancelling it again, or something that never existed, reports false
        h.line(r#"{"op":"http.cancel","id":12,"request":10}"#);
        h.line(r#"{"op":"http.cancel","id":13,"request":999}"#);
        assert_eq!(h.reply(12).get("cancelled"), Some(&Json::Bool(false)));
        assert_eq!(h.reply(13).get("cancelled"), Some(&Json::Bool(false)));
        // and the id can be used again
        h.line(&send_line(
            10,
            "POST",
            "https://api.example.com/v1",
            vec![("timeoutMs", Json::int(40))],
        ));
        h.wait();
    }

    #[test]
    fn cancel_requests_are_validated() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line(r#"{"op":"http.cancel","id":1}"#);
        h.line(r#"{"op":"http.cancel","id":2,"request":"x"}"#);
        h.line(r#"{"op":"http.cancel","id":3,"request":1,"extra":1}"#);
        for (id, field) in [(1, "request"), (2, "request"), (3, "extra")] {
            let r = h.reply(id);
            assert_eq!(
                (code(&r), r.get("field").and_then(Json::as_str)),
                ("INVALID_REQUEST", Some(field))
            );
        }
    }

    #[test]
    fn replies_are_matched_by_id_so_a_fast_request_overtakes_a_slow_one() {
        let h = Harness::new(|r, flight| {
            if r.body == b"slow" {
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_millis(300) && !flight.is_aborted() {
                    sleep(Duration::from_millis(2));
                }
            }
            Ok(response(200, &[], "{}"))
        });
        h.line(&send_line(
            20,
            "POST",
            "https://api.example.com/a",
            vec![("body", Json::str("slow"))],
        ));
        h.line(&send_line(
            21,
            "POST",
            "https://api.example.com/b",
            vec![("body", Json::str("fast"))],
        ));
        h.wait();
        let ids: Vec<i64> = h
            .replies()
            .iter()
            .filter_map(|r| r.get("id").and_then(Json::as_i64))
            .collect();
        assert_eq!(ids, vec![21, 20]);
    }

    #[test]
    fn an_id_that_is_still_in_flight_cannot_be_reused() {
        let h = Harness::new(hang);
        h.line(&send_line(30, "POST", "https://api.example.com/a", vec![]));
        h.wait_for_requests(1);
        h.line(&send_line(30, "POST", "https://api.example.com/b", vec![]));
        let dup = h.reply(30);
        assert_eq!(
            (code(&dup), dup.get("field").and_then(Json::as_str)),
            ("INVALID_REQUEST", Some("id"))
        );
        assert!(h.engine.shutdown(Duration::from_secs(5)));
    }

    #[test]
    fn too_many_requests_in_flight_are_refused_with_busy() {
        let h = Harness::new(hang);
        for id in 0..MAX_IN_FLIGHT as i64 {
            h.line(&send_line(
                100 + id,
                "POST",
                "https://api.example.com/a",
                vec![],
            ));
        }
        h.wait_for_requests(MAX_IN_FLIGHT);
        h.line(&send_line(200, "POST", "https://api.example.com/a", vec![]));
        assert_eq!(code(&h.reply(200)), "BUSY");
        // shutdown cancels them all and their replies are written before it returns
        assert!(h.engine.shutdown(Duration::from_secs(5)));
        for id in 0..MAX_IN_FLIGHT as i64 {
            assert_eq!(code(&h.reply(100 + id)), "CANCELLED");
        }
    }

    #[test]
    fn a_key_is_never_sent_to_an_origin_it_was_not_stored_for() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.store_key("openai", "api.openai.com");
        for (id, url) in [
            (40, "https://evil.example/v1/responses"),
            (41, "https://api.openai.com:8443/v1/responses"),
            (42, "https://api.openai.com.evil.example/v1"),
            (43, "https://API.OPENAI.COM@evil.example/"),
        ] {
            h.line(&send_line(id, "POST", url, bearer("openai")));
            let r = h.reply(id);
            assert_ne!(code(&r), "", "{url}");
            assert!(!is_ok(&r), "{url}");
        }
        assert_eq!(code(&h.reply(40)), "CREDENTIAL_ORIGIN_MISMATCH");
        assert_eq!(code(&h.reply(41)), "CREDENTIAL_ORIGIN_MISMATCH");
        assert_eq!(code(&h.reply(42)), "CREDENTIAL_ORIGIN_MISMATCH");
        assert_eq!(code(&h.reply(43)), "URL_NOT_ALLOWED");
        assert_eq!(h.seen_count(), 0, "nothing was sent");
        // the right origin works, also spelled in upper case
        h.line(&send_line(
            44,
            "POST",
            "https://API.OpenAI.com/v1/responses",
            bearer("openai"),
        ));
        h.wait();
        assert!(is_ok(&h.reply(44)));
    }

    #[test]
    fn a_missing_credential_or_a_failing_store_sends_nothing() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line(&send_line(
            50,
            "POST",
            "https://api.openai.com/v1",
            bearer("openai"),
        ));
        let r = h.reply(50);
        assert_eq!(
            (code(&r), r.get("field").and_then(Json::as_str)),
            ("NO_CREDENTIAL", Some("credentialRef"))
        );
        h.vault.set_failing(true);
        h.line(&send_line(
            51,
            "POST",
            "https://api.openai.com/v1",
            bearer("openai"),
        ));
        assert_eq!(code(&h.reply(51)), "VAULT_ERROR");
        assert_eq!(h.seen_count(), 0);
    }

    #[test]
    fn invalid_http_send_requests_are_refused_before_anything_is_sent() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        let base = |id: i64, extra: Vec<(&str, Json)>| {
            send_line(id, "POST", "https://api.example.com/v1", extra)
        };
        let cases: Vec<(i64, String, &str, &str)> = vec![
            (
                60,
                send_line(60, "PUT", "https://api.example.com/v1", vec![]),
                "INVALID_REQUEST",
                "method",
            ),
            (
                61,
                send_line(61, "POST", "http://api.example.com/v1", vec![]),
                "URL_NOT_ALLOWED",
                "url",
            ),
            (
                62,
                send_line(62, "POST", "https://u:p@api.example.com/v1", vec![]),
                "URL_NOT_ALLOWED",
                "url",
            ),
            (
                63,
                send_line(
                    63,
                    "GET",
                    "https://api.example.com/v1",
                    vec![("body", Json::str("x"))],
                ),
                "INVALID_REQUEST",
                "body",
            ),
            (
                64,
                base(
                    64,
                    vec![(
                        "headers",
                        Json::obj(vec![("Authorization", Json::str("Bearer stolen"))]),
                    )],
                ),
                "HEADER_NOT_ALLOWED",
                "headers",
            ),
            (
                65,
                base(
                    65,
                    vec![("headers", Json::obj(vec![("Content-Type", Json::int(1))]))],
                ),
                "INVALID_REQUEST",
                "headers",
            ),
            (
                66,
                base(66, vec![("credentialRef", Json::str("openai"))]),
                "INVALID_REQUEST",
                "authHeader",
            ),
            (
                67,
                base(
                    67,
                    vec![("authHeader", Json::str("Authorization: Bearer {key}"))],
                ),
                "INVALID_REQUEST",
                "credentialRef",
            ),
            (
                68,
                base(68, vec![("timeoutMs", Json::int(0))]),
                "INVALID_REQUEST",
                "timeoutMs",
            ),
            (
                69,
                base(69, vec![("timeoutMs", Json::int(MAX_TIMEOUT_MS + 1))]),
                "INVALID_REQUEST",
                "timeoutMs",
            ),
            (
                70,
                base(70, vec![("timeoutMs", Json::str("5"))]),
                "INVALID_REQUEST",
                "timeoutMs",
            ),
            (
                71,
                base(71, vec![("credentialREF", Json::str("openai"))]),
                "INVALID_REQUEST",
                "credentialREF",
            ),
            (
                75,
                base(75, vec![("sk-proj-AbCdEf0123456789", Json::int(1))]),
                "INVALID_REQUEST",
                "?",
            ),
            (
                72,
                base(
                    72,
                    vec![
                        ("credentialRef", Json::str("a/b")),
                        ("authHeader", Json::str("Authorization: Bearer {key}")),
                    ],
                ),
                "INVALID_REQUEST",
                "credentialRef",
            ),
            (
                73,
                base(
                    73,
                    vec![
                        ("credentialRef", Json::str("openai")),
                        ("authHeader", Json::str("Host: {key}")),
                    ],
                ),
                "HEADER_NOT_ALLOWED",
                "authHeader",
            ),
            (
                74,
                base(74, vec![("body", Json::int(5))]),
                "INVALID_REQUEST",
                "body",
            ),
        ];
        for (id, line, want, field) in cases {
            h.line(&line);
            let r = h.reply(id);
            assert_eq!(code(&r), want, "id {id}: {}", h.raw());
            assert_eq!(
                r.get("field").and_then(Json::as_str),
                Some(field),
                "id {id}"
            );
        }
        // a request without an id cannot be matched to its reply
        h.line(r#"{"op":"http.send","method":"GET","url":"https://api.example.com/"}"#);
        let last = h.replies().pop().unwrap();
        assert_eq!(
            (code(&last), last.get("field").and_then(Json::as_str)),
            ("INVALID_REQUEST", Some("id"))
        );
        assert_eq!(h.seen_count(), 0);
    }

    #[test]
    fn transport_failures_become_tokens_and_a_numeric_code_only() {
        let h = Harness::new(|r, _| {
            if r.target.path_and_query.ends_with("tls") {
                Err(Failure::new(Code::DepthZeroSelfSignedCert))
            } else {
                let mut f = Failure::new(Code::TransportError);
                f.os_code = Some(12004);
                Err(f)
            }
        });
        h.line(&send_line(
            80,
            "POST",
            "https://api.example.com/tls",
            vec![],
        ));
        h.line(&send_line(
            81,
            "POST",
            "https://api.example.com/other",
            vec![],
        ));
        h.wait();
        assert_eq!(code(&h.reply(80)), "DEPTH_ZERO_SELF_SIGNED_CERT");
        let r = h.reply(81);
        assert_eq!(code(&r), "TRANSPORT_ERROR");
        assert_eq!(r.get("winError").and_then(Json::as_i64), Some(12004));
    }

    #[test]
    fn an_oversized_response_is_refused_even_if_the_transport_returned_it() {
        let h = Harness::new(|_, _| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: vec![b'x'; MAX_RESPONSE_BYTES + 1],
            })
        });
        h.line(&send_line(90, "POST", "https://api.example.com/v1", vec![]));
        h.wait();
        assert_eq!(code(&h.reply(90)), "RESPONSE_TOO_LARGE");
        let ok = Harness::new(|_, _| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: vec![b'x'; MAX_RESPONSE_BYTES],
            })
        });
        ok.line(&send_line(91, "POST", "https://api.example.com/v1", vec![]));
        ok.wait();
        assert_eq!(
            ok.reply(91).get("bodyBytes").and_then(Json::as_i64),
            Some(MAX_RESPONSE_BYTES as i64)
        );
    }

    #[test]
    fn a_bogus_status_line_is_a_bad_response() {
        let h = Harness::new(|_, _| Ok(response(0, &[], "")));
        h.line(&send_line(92, "POST", "https://api.example.com/v1", vec![]));
        h.wait();
        assert_eq!(code(&h.reply(92)), "BAD_RESPONSE");
    }

    #[test]
    fn non_utf8_response_bodies_are_decoded_lossily_not_dropped() {
        let h = Harness::new(|_, _| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: vec![b'o', b'k', 0xff, b'!'],
            })
        });
        h.line(&send_line(93, "POST", "https://api.example.com/v1", vec![]));
        h.wait();
        let r = h.reply(93);
        assert_eq!(r.get("body").and_then(Json::as_str), Some("ok\u{fffd}!"));
        assert_eq!(r.get("bodyBytes").and_then(Json::as_i64), Some(4));
    }

    #[test]
    fn the_untrusted_ca_relaxation_applies_only_to_loopback_targets_and_only_when_enabled() {
        let relaxed = Config {
            insecure_test_ca: true,
            test_mode: true,
            ..Config::default()
        };
        let h = Harness::with_config(relaxed, |_, _| Ok(response(200, &[], "{}")));
        h.line(&send_line(110, "POST", "https://127.0.0.1:9/v1", vec![]));
        h.line(&send_line(
            111,
            "POST",
            "https://api.example.com/v1",
            vec![],
        ));
        h.wait();
        let seen = h.fake.seen.lock().unwrap();
        let flag_for = |host: &str| {
            seen.iter()
                .find(|s| s.host == host)
                .unwrap()
                .accept_untrusted_ca
        };
        assert!(flag_for("127.0.0.1"));
        assert!(
            !flag_for("api.example.com"),
            "a real host is always verified against the system store"
        );
        drop(seen);
        let strict = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        strict.line(&send_line(112, "POST", "https://127.0.0.1:9/v1", vec![]));
        strict.wait();
        assert!(
            !strict.fake.seen.lock().unwrap()[0].accept_untrusted_ca,
            "the default never relaxes"
        );
    }

    #[test]
    fn plain_http_is_refused_unless_the_test_mode_allows_loopback() {
        let strict = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        strict.line(&send_line(120, "POST", "http://127.0.0.1:9000/v1", vec![]));
        assert_eq!(code(&strict.reply(120)), "URL_NOT_ALLOWED");
        let cfg = Config {
            allow_loopback_http: true,
            test_mode: true,
            ..Config::default()
        };
        let h = Harness::with_config(cfg, |_, _| Ok(response(200, &[], "{}")));
        h.line(&send_line(121, "POST", "http://127.0.0.1:9000/v1", vec![]));
        h.line(&send_line(122, "POST", "http://api.example.com/v1", vec![]));
        h.wait();
        assert!(is_ok(&h.reply(121)));
        assert_eq!(code(&h.reply(122)), "URL_NOT_ALLOWED");
    }

    #[test]
    fn it_016_credential_operations_round_trip_and_no_reply_ever_carries_the_key() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line(r#"{"op":"cred.exists","id":1,"credentialRef":"openai"}"#);
        assert_eq!(h.reply(1).get("exists"), Some(&Json::Bool(false)));
        h.store_key("openai", "API.OpenAI.com");
        let stored = h.reply(9000);
        assert_eq!(stored.get("stored"), Some(&Json::Bool(true)));
        assert_eq!(
            stored.get("host").and_then(Json::as_str),
            Some("api.openai.com:443")
        );
        h.line(r#"{"op":"cred.exists","id":2,"credentialRef":"openai"}"#);
        let exists = h.reply(2);
        assert_eq!(exists.get("exists"), Some(&Json::Bool(true)));
        assert_eq!(
            exists.get("host").and_then(Json::as_str),
            Some("api.openai.com:443")
        );
        assert_eq!(h.vault.targets(), vec!["ProgrammingIME/openai".to_string()]);
        h.line(r#"{"op":"cred.delete","id":3,"credentialRef":"openai"}"#);
        assert_eq!(h.reply(3).get("deleted"), Some(&Json::Bool(true)));
        h.line(r#"{"op":"cred.delete","id":4,"credentialRef":"openai"}"#);
        assert_eq!(h.reply(4).get("deleted"), Some(&Json::Bool(false)));
        h.line(r#"{"op":"cred.exists","id":5,"credentialRef":"openai"}"#);
        assert_eq!(h.reply(5).get("exists"), Some(&Json::Bool(false)));
        // a deleted key can no longer be used
        h.line(&send_line(
            6,
            "POST",
            "https://api.openai.com/v1",
            bearer("openai"),
        ));
        assert_eq!(code(&h.reply(6)), "NO_CREDENTIAL");
        assert!(!h.raw().contains(KEY), "{}", h.raw());
    }

    #[test]
    fn a_test_namespace_changes_the_target_name_and_nothing_else() {
        let cfg = Config {
            namespace: "PIME-TEST-abc".to_string(),
            session_credentials: true,
            test_mode: true,
            ..Config::default()
        };
        let h = Harness::with_config(cfg, |_, _| Ok(response(200, &[], "{}")));
        h.store_key("openai", "api.openai.com");
        assert_eq!(h.vault.targets(), vec!["PIME-TEST-abc/openai".to_string()]);
    }

    #[test]
    fn cred_set_refuses_bad_input_without_echoing_the_key() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        let line = |id: i64, provider: &str, host: &str, secret: &str| {
            Json::obj(vec![
                ("op", Json::str("cred.set")),
                ("id", Json::int(id)),
                ("credentialRef", Json::str(provider)),
                ("host", Json::str(host)),
                ("secret", Json::str(secret)),
            ])
            .to_json_string()
        };
        h.line(&line(
            1,
            "openai",
            "api.openai.com",
            "has spaces in the pasted key",
        ));
        h.line(&line(2, "openai", "api.openai.com", "short"));
        h.line(&line(3, "a/b", "api.openai.com", KEY));
        h.line(&line(4, "openai", "https://api.openai.com", KEY));
        h.line(&line(5, "openai", "", KEY));
        h.line(r#"{"op":"cred.set","id":6,"credentialRef":"openai","host":"api.openai.com"}"#);
        h.line(r#"{"op":"cred.set","id":7,"credentialRef":"openai","host":"api.openai.com","secret":12345678}"#);
        h.line(r#"{"op":"cred.set","id":8,"credentialRef":"openai","host":"api.openai.com","secret":"mock-key-test-123","extra":1}"#);
        for (id, field) in [
            (1, "secret"),
            (2, "secret"),
            (3, "credentialRef"),
            (4, "host"),
            (5, "host"),
            (6, "secret"),
            (7, "secret"),
            (8, "extra"),
        ] {
            let r = h.reply(id);
            assert_eq!(
                (code(&r), r.get("field").and_then(Json::as_str)),
                ("INVALID_REQUEST", Some(field)),
                "id {id}"
            );
        }
        assert!(h.vault.targets().is_empty(), "nothing was stored");
        for echoed in ["spaces", "pasted", "short", KEY] {
            assert!(!h.raw().contains(echoed), "{echoed} leaked: {}", h.raw());
        }
    }

    #[test]
    fn a_failing_credential_store_is_reported_as_a_token_for_every_credential_operation() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.vault.set_failing(true);
        h.line(r#"{"op":"cred.exists","id":1,"credentialRef":"openai"}"#);
        h.line(r#"{"op":"cred.delete","id":2,"credentialRef":"openai"}"#);
        h.line(r#"{"op":"cred.set","id":3,"credentialRef":"openai","host":"api.openai.com","secret":"mock-key-test-123"}"#);
        for id in 1..=3 {
            assert_eq!(code(&h.reply(id)), "VAULT_ERROR", "id {id}");
        }
        assert!(!h.raw().contains(KEY));
    }

    #[test]
    fn framing_errors_follow_the_core_conventions() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line("not json");
        h.line("[1]");
        h.line(r#"{"op":"nope","id":3}"#);
        h.line(r#"{"op":"health","id":"x"}"#);
        h.line("{}");
        h.engine.handle_line(&mut vec![0xff, 0xfe]);
        let big = format!(
            r#"{{"op":"cred.exists","id":9,"credentialRef":"{}"}}"#,
            "a".repeat(70_000)
        );
        h.line(&big);
        h.engine.reject_too_long();
        let codes: Vec<(String, Option<i64>)> = h
            .replies()
            .iter()
            .map(|r| (code(r).to_string(), r.get("id").and_then(Json::as_i64)))
            .collect();
        assert_eq!(
            codes,
            vec![
                ("INVALID_JSON".to_string(), None),
                ("INVALID_REQUEST".to_string(), None),
                ("UNKNOWN_OPERATION".to_string(), Some(3)),
                ("INVALID_REQUEST".to_string(), None),
                ("UNKNOWN_OPERATION".to_string(), None),
                ("INVALID_JSON".to_string(), None),
                ("PAYLOAD_TOO_LARGE".to_string(), None),
                ("PAYLOAD_TOO_LARGE".to_string(), None),
            ]
        );
    }

    #[test]
    fn only_http_send_may_use_the_bulk_line_size() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        let body = "b".repeat(200_000);
        h.line(&send_line(
            1,
            "POST",
            "https://api.example.com/v1",
            vec![("body", Json::str(&body))],
        ));
        h.wait();
        assert!(is_ok(&h.reply(1)));
        assert_eq!(h.fake.seen.lock().unwrap()[0].body.len(), 200_000);
        let too_big = "b".repeat(MAX_REQUEST_BODY_BYTES + 1);
        h.line(&send_line(
            2,
            "POST",
            "https://api.example.com/v1",
            vec![("body", Json::str(&too_big))],
        ));
        let r = h.reply(2);
        assert_eq!(
            (code(&r), r.get("field").and_then(Json::as_str)),
            ("INVALID_REQUEST", Some("body"))
        );
    }

    #[test]
    fn health_reports_the_security_relevant_configuration() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line(r#"{"op":"health","id":1}"#);
        let r = h.reply(1);
        assert_eq!(r.get("name").and_then(Json::as_str), Some("pime-ai-worker"));
        assert_eq!(r.get("protocol").and_then(Json::as_str), Some("0.3"));
        assert_eq!(r.get("transport").and_then(Json::as_str), Some("fake"));
        assert_eq!(
            r.get("namespace").and_then(Json::as_str),
            Some("ProgrammingIME")
        );
        for flag in ["testMode", "insecureTestCa", "allowLoopbackHttp"] {
            assert_eq!(r.get(flag), Some(&Json::Bool(false)), "{flag}");
        }
        let ops: Vec<&str> = r
            .get("ops")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .filter_map(Json::as_str)
            .collect();
        assert_eq!(ops, OPS);
    }

    #[test]
    fn a_small_line_buffer_is_wiped_after_it_was_handled() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        let mut line = format!(
            r#"{{"op":"cred.set","id":1,"credentialRef":"openai","host":"api.openai.com","secret":"{KEY}"}}"#
        )
        .into_bytes();
        h.engine.handle_line(&mut line);
        assert!(line.is_empty());
        assert!(is_ok(&h.reply(1)));
    }

    #[test]
    fn the_timeout_budget_reaches_the_transport() {
        let h = Harness::new(|_, _| Ok(response(200, &[], "{}")));
        h.line(&send_line(1, "POST", "https://api.example.com/v1", vec![]));
        h.line(&send_line(
            2,
            "POST",
            "https://api.example.com/v1",
            vec![("timeoutMs", Json::int(1234))],
        ));
        h.wait();
        let seen = h.fake.seen.lock().unwrap();
        let mut timeouts: Vec<Duration> = seen.iter().map(|s| s.timeout).collect();
        timeouts.sort();
        assert_eq!(
            timeouts,
            vec![
                Duration::from_millis(1234),
                Duration::from_millis(DEFAULT_TIMEOUT_MS as u64)
            ]
        );
    }

    #[test]
    fn serve_answers_every_line_and_aborts_what_is_left_at_end_of_input() {
        let h = Harness::new(hang);
        let input = format!(
            "{}\n{}\nnot json\n",
            r#"{"op":"health","id":1}"#,
            send_line(
                2,
                "POST",
                "https://api.example.com/v1",
                vec![("timeoutMs", Json::int(30_000))]
            )
        );
        let t0 = Instant::now();
        serve(&h.engine, Cursor::new(input.into_bytes())).unwrap();
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "end of input must not wait for the 30 s deadline"
        );
        assert!(is_ok(&h.reply(1)));
        assert_eq!(code(&h.reply(2)), "CANCELLED");
        assert!(h.replies().iter().any(|r| code(r) == "INVALID_JSON"));
    }
}
