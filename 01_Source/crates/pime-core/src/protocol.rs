//! JSON Lines protocol 0.3 (one JSON object per line).
//!
//! Protocol 0.3 is a superset of 0.2: the seven stateless 0.2 operations behave exactly as before (at most
//! 64 KiB per request line), and the stateful session operations of `ops.rs` are added. Operations that carry
//! document text (`ctx.snapshot`, `edit.propose`, `edit.prepare`, `edit.begin_apply`, `edit.finish_apply`) may use
//! lines up to [`MAX_BULK_LINE_BYTES`]; every other operation keeps the 64 KiB limit.
//!
//! Every request may carry an integer `id` that is echoed as `id` in the response. The core never executes
//! anything, never opens files or the network, and never echoes secret values (`scan` returns kind, severity
//! and line only).

use crate::clock::{Clock, SystemClock};
use crate::json::{self, Json, JsonErrorKind, Limits};
use crate::ops;
use crate::pack::LANGUAGES;
use crate::predict::{DEFAULT_LIMIT, PredictOptions, engine};
use crate::resolve::{ResolveContext, resolve_language};
use crate::secrets;
use crate::session::SessionStore;
use crate::validators::{self, Dialect};
use crate::x86;
use crate::{MAX_BULK_LINE_BYTES, MAX_LINE_BYTES, PROTOCOL, VERSION};
use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

pub const MAX_PREDICT_LIMIT: i64 = 200;
const REQUEST_LIMITS: Limits = Limits {
    max_bytes: MAX_LINE_BYTES,
    max_depth: 16,
};
const BULK_LIMITS: Limits = Limits {
    max_bytes: MAX_BULK_LINE_BYTES,
    max_depth: 16,
};

fn error_json(id: Option<i64>, error: &str, field: Option<&str>) -> Json {
    let mut pairs = vec![("ok", Json::Bool(false))];
    if let Some(i) = id {
        pairs.push(("id", Json::int(i)));
    }
    pairs.push(("error", Json::str(error)));
    if let Some(f) = field {
        pairs.push(("field", Json::str(f)));
    }
    Json::obj(pairs)
}

fn ok_json(id: Option<i64>, mut rest: Vec<(&str, Json)>) -> Json {
    let mut pairs = vec![("ok", Json::Bool(true))];
    if let Some(i) = id {
        pairs.push(("id", Json::int(i)));
    }
    pairs.append(&mut rest);
    Json::obj(pairs)
}

/// Optional string field: absent or null -> `None`; a non-string is an `INVALID_REQUEST`.
fn opt_str<'a>(req: &'a Json, key: &str) -> Result<Option<&'a str>, String> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s)),
        Some(_) => Err(key.to_string()),
    }
}

pub(crate) fn dialect_of(req: &Json) -> Result<Dialect, String> {
    let mut d = Dialect::new();
    match req.get("dialect") {
        None | Some(Json::Null) => {}
        Some(Json::Obj(o)) => {
            for (k, v) in o {
                match v {
                    Json::Str(s) => {
                        d.insert(k.clone(), s.clone());
                    }
                    Json::Null => {}
                    _ => return Err("dialect".to_string()),
                }
            }
        }
        Some(_) => return Err("dialect".to_string()),
    }
    Ok(d)
}

fn health(id: Option<i64>) -> Json {
    ok_json(
        id,
        vec![
            ("name", Json::str("pime-core")),
            ("version", Json::str(VERSION)),
            ("protocol", Json::str(PROTOCOL)),
            ("languages", Json::strs(&LANGUAGES)),
        ],
    )
}

fn predict_op(id: Option<i64>, req: &Json) -> Result<Json, String> {
    let limit = match req.get("limit") {
        None | Some(Json::Null) => DEFAULT_LIMIT,
        Some(v) => match v.as_i64() {
            Some(n) if (0..=MAX_PREDICT_LIMIT).contains(&n) => n as usize,
            _ => return Err("limit".to_string()),
        },
    };
    let mut o = PredictOptions::new(
        opt_str(req, "language")?.unwrap_or(""),
        opt_str(req, "prefix")?.unwrap_or(""),
    )
    .before(opt_str(req, "before")?.unwrap_or(""))
    .after(opt_str(req, "after")?.unwrap_or(""))
    .limit(limit);
    o.dialect = dialect_of(req)?;
    // optional ranking inputs of the bridge (additive in 0.3): pinned candidate ids and local usage counts
    match req.get("pins") {
        None | Some(Json::Null) => {}
        Some(Json::Arr(a)) => {
            for p in a {
                o.pins
                    .insert(p.as_str().ok_or_else(|| "pins".to_string())?.to_string());
            }
        }
        Some(_) => return Err("pins".to_string()),
    }
    match req.get("usage") {
        None | Some(Json::Null) => {}
        Some(Json::Obj(u)) => {
            for (k, v) in u {
                o.usage
                    .insert(k.clone(), v.as_i64().ok_or_else(|| "usage".to_string())?);
            }
        }
        Some(_) => return Err("usage".to_string()),
    }
    let list = engine().predict(&o).map_err(|_| "predict".to_string())?;
    Ok(ok_json(
        id,
        vec![(
            "candidates",
            Json::Arr(list.iter().map(|c| c.to_json()).collect()),
        )],
    ))
}

fn resolve_op(id: Option<i64>, req: &Json) -> Result<Json, String> {
    let ctx = ResolveContext {
        language_id: opt_str(req, "languageId")?,
        file_name: opt_str(req, "fileName")?,
        text: opt_str(req, "text")?,
    };
    let current = opt_str(req, "current")?.unwrap_or("unknown");
    let d = resolve_language(engine().registry(), &ctx, current, opt_str(req, "locked")?);
    let hints = Json::Obj(
        d.dialect_hints
            .iter()
            .map(|(k, v)| (k.clone(), Json::str(v)))
            .collect(),
    );
    let decision = Json::obj(vec![
        ("language", Json::str(&d.language)),
        ("reason", Json::str(d.reason)),
        ("changed", Json::Bool(d.changed)),
        ("uncertain", Json::Bool(d.uncertain)),
        (
            "suggestion",
            d.suggestion.as_deref().map_or(Json::Null, Json::str),
        ),
        ("confidence", Json::int(i64::from(d.confidence))),
        ("dialectHints", hints),
    ]);
    Ok(ok_json(id, vec![("decision", decision)]))
}

fn validate_op(id: Option<i64>, req: &Json) -> Result<Json, String> {
    let language = opt_str(req, "language")?.unwrap_or("");
    let dialect = dialect_of(req)?;
    // a missing / non-string `text` is reported like the reference engine (NOT_TEXT), not as a protocol error
    let result = match req.get("text") {
        Some(Json::Str(t)) if LANGUAGES.contains(&language) => {
            validators::validate_source(language, t, &dialect)
        }
        _ if !LANGUAGES.contains(&language) => validators::validate_source(language, "", &dialect),
        _ => validators::not_text(),
    };
    let issues = result
        .issues
        .iter()
        .map(|i| {
            Json::obj(vec![
                ("line", Json::int(i.line as i64)),
                ("code", Json::str(i.code)),
                ("message", Json::str(&i.message)),
            ])
        })
        .collect();
    let body = Json::obj(vec![
        ("ok", Json::Bool(result.ok)),
        ("level", Json::str(result.level)),
        ("issues", Json::Arr(issues)),
    ]);
    Ok(ok_json(id, vec![("result", body)]))
}

fn encode_op(id: Option<i64>, req: &Json) -> Result<Json, String> {
    Ok(
        match x86::encode(opt_str(req, "instruction")?.unwrap_or("")) {
            x86::EncodeResult::Ok { hex, .. } => ok_json(id, vec![("hex", Json::str(&hex))]),
            x86::EncodeResult::Err { code, reason } => failure_json(id, code, reason),
        },
    )
}

fn failure_json(id: Option<i64>, code: &str, reason: &str) -> Json {
    let mut pairs = vec![("ok", Json::Bool(false))];
    if let Some(i) = id {
        pairs.push(("id", Json::int(i)));
    }
    pairs.push(("code", Json::str(code)));
    pairs.push(("reason", Json::str(reason)));
    Json::obj(pairs)
}

fn validate_hex_op(id: Option<i64>, req: &Json) -> Result<Json, String> {
    Ok(
        match x86::validate_hex(opt_str(req, "hex")?.unwrap_or(""), 2048) {
            x86::HexResult::Ok { bytes, hex } => ok_json(
                id,
                vec![
                    ("hex", Json::str(&hex)),
                    ("byte_count", Json::int(bytes.len() as i64)),
                ],
            ),
            x86::HexResult::Err { code, reason } => failure_json(id, code, reason),
        },
    )
}

fn scan_op(id: Option<i64>, req: &Json) -> Result<Json, String> {
    let r = secrets::scan(
        opt_str(req, "text")?.unwrap_or(""),
        opt_str(req, "fileName")?.unwrap_or(""),
    );
    // findings never carry the matched value, only its kind, severity and line
    let findings = r
        .findings
        .iter()
        .map(|f| {
            Json::obj(vec![
                ("kind", Json::str(f.kind)),
                ("severity", Json::str(f.severity)),
                ("line", Json::int(f.line as i64)),
            ])
        })
        .collect();
    let body = Json::obj(vec![
        ("highest", Json::str(r.highest)),
        ("sensitiveFile", Json::Bool(r.sensitive_file)),
        ("blocked", Json::Bool(r.blocked)),
        ("truncated", Json::Bool(r.truncated)),
        ("findings", Json::Arr(findings)),
    ]);
    Ok(ok_json(id, vec![("result", body)]))
}

/// The state of one running core: the open sessions and the clock every time-dependent rule reads.
pub struct Core {
    sessions: SessionStore,
    clock: Box<dyn Clock>,
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Core {
    /// A core on the real (wall) clock.
    pub fn new() -> Core {
        Core::with_clock(Box::new(SystemClock))
    }

    /// A core with an injected clock (tests move time explicitly).
    pub fn with_clock(clock: Box<dyn Clock>) -> Core {
        Core {
            sessions: SessionStore::new(),
            clock,
        }
    }

    pub fn open_sessions(&self) -> usize {
        self.sessions.len()
    }

    /// Handles one parsed request object.
    pub fn dispatch(&mut self, req: &Json) -> Json {
        if req.as_object().is_none() {
            return error_json(None, "INVALID_REQUEST", None);
        }
        let id = match req.get("id") {
            None | Some(Json::Null) => None,
            Some(v) => match v.as_i64() {
                Some(i) => Some(i),
                None => return error_json(None, "INVALID_REQUEST", Some("id")),
            },
        };
        let op = req.get("op").and_then(Json::as_str).unwrap_or("");
        let outcome = match op {
            "health" => Ok(health(id)),
            "predict" => predict_op(id, req),
            "resolve" => resolve_op(id, req),
            "validate" => validate_op(id, req),
            "encode" => encode_op(id, req),
            "validate_hex" => validate_hex_op(id, req),
            "scan" => scan_op(id, req),
            _ if ops::OPS.contains(&op) => {
                return ops::handle(&mut self.sessions, self.clock.now_ms(), op, id, req);
            }
            _ => return error_json(id, "UNKNOWN_OPERATION", None),
        };
        outcome.unwrap_or_else(|field| error_json(id, "INVALID_REQUEST", Some(&field)))
    }

    /// One request line -> one response line (no trailing newline). Never panics out, never echoes input text.
    pub fn handle_line(&mut self, line: &str) -> String {
        if line.len() > MAX_BULK_LINE_BYTES {
            return error_json(None, "PAYLOAD_TOO_LARGE", None).to_json_string();
        }
        let big = line.len() > MAX_LINE_BYTES;
        let limits = if big { BULK_LIMITS } else { REQUEST_LIMITS };
        let response = match json::parse(line, limits) {
            Err(e) if e.kind == JsonErrorKind::TooLarge => {
                error_json(None, "PAYLOAD_TOO_LARGE", None)
            }
            // a line over the 64 KiB limit that is not a well-formed bulk request is "too large", as in 0.2
            Err(_) if big => error_json(None, "PAYLOAD_TOO_LARGE", None),
            Err(_) => error_json(None, "INVALID_JSON", None),
            Ok(req) => {
                let op = req.get("op").and_then(Json::as_str).unwrap_or("");
                if big && !ops::BULK_OPS.contains(&op) {
                    error_json(None, "PAYLOAD_TOO_LARGE", None)
                } else {
                    catch_unwind(AssertUnwindSafe(|| self.dispatch(&req)))
                        .unwrap_or_else(|_| error_json(None, "INTERNAL", None))
                }
            }
        };
        response.to_json_string()
    }
}

thread_local! {
    // The stateless convenience entry points below share one core per thread (state survives between calls).
    static DEFAULT_CORE: RefCell<Core> = RefCell::new(Core::new());
}

/// Handles one parsed request on this thread's default core.
pub fn dispatch(req: &Json) -> Json {
    DEFAULT_CORE.with(|c| c.borrow_mut().dispatch(req))
}

/// One request line -> one response line on this thread's default core.
pub fn handle_line(line: &str) -> String {
    DEFAULT_CORE.with(|c| c.borrow_mut().handle_line(line))
}

enum LineRead {
    Line,
    TooLong,
    Eof,
}

/// Reads one line without ever buffering more than `max + 2` bytes (an oversized line is skipped to its end).
fn read_bounded_line<R: BufRead>(r: &mut R, buf: &mut Vec<u8>, max: usize) -> io::Result<LineRead> {
    buf.clear();
    let mut too_long = false;
    let mut seen_any = false;
    loop {
        let chunk = r.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if !seen_any {
                LineRead::Eof
            } else if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
        seen_any = true;
        let (take, found) = match chunk.iter().position(|&b| b == b'\n') {
            Some(p) => (p, true),
            None => (chunk.len(), false),
        };
        if !too_long {
            if buf.len() + take > max + 1 {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(take + usize::from(found));
        if found {
            if !too_long && buf.last() == Some(&b'\r') {
                buf.pop();
            }
            if buf.len() > max {
                too_long = true;
            }
            return Ok(if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
    }
}

/// Serves requests until end of input on a fresh core: exactly one response line per request line.
pub fn serve<R: BufRead, W: Write>(input: R, output: W) -> io::Result<()> {
    serve_with(&mut Core::new(), input, output)
}

/// [`serve`] on a given core (so tests can inject the clock and inspect the state afterwards).
pub fn serve_with<R: BufRead, W: Write>(
    core: &mut Core,
    mut input: R,
    mut output: W,
) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        // a line is buffered up to the bulk limit; `handle_line` applies the per-operation limit
        let reply = match read_bounded_line(&mut input, &mut buf, MAX_BULK_LINE_BYTES)? {
            LineRead::Eof => return Ok(()),
            LineRead::TooLong => error_json(None, "PAYLOAD_TOO_LARGE", None).to_json_string(),
            LineRead::Line => match std::str::from_utf8(&buf) {
                Ok(line) => core.handle_line(line),
                Err(_) => error_json(None, "INVALID_JSON", None).to_json_string(),
            },
        };
        output.write_all(reply.as_bytes())?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> Json {
        json::parse(&handle_line(line), Limits::DEFAULT).expect("response is valid JSON")
    }

    #[test]
    fn health_reports_protocol_0_3_and_seven_languages() {
        let r = call(r#"{"op":"health","id":7}"#);
        assert_eq!(r.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(r.get("id").and_then(Json::as_i64), Some(7));
        assert_eq!(r.get("name").and_then(Json::as_str), Some("pime-core"));
        assert_eq!(r.get("version").and_then(Json::as_str), Some("0.2.0"));
        assert_eq!(r.get("protocol").and_then(Json::as_str), Some("0.3"));
        assert_eq!(
            r.get("languages").and_then(Json::as_array).map(Vec::len),
            Some(7)
        );
    }

    #[test]
    fn framing_errors() {
        assert_eq!(
            call("not json").get("error").and_then(Json::as_str),
            Some("INVALID_JSON")
        );
        assert_eq!(
            call(r#"{"op":"nope","id":3}"#)
                .get("error")
                .and_then(Json::as_str),
            Some("UNKNOWN_OPERATION")
        );
        assert_eq!(
            call(r#"{"op":"nope","id":3}"#)
                .get("id")
                .and_then(Json::as_i64),
            Some(3)
        );
        assert_eq!(
            call("{}").get("error").and_then(Json::as_str),
            Some("UNKNOWN_OPERATION")
        );
        assert_eq!(
            call("[1]").get("error").and_then(Json::as_str),
            Some("INVALID_REQUEST")
        );
        assert_eq!(
            call(r#"{"op":"health","id":"x"}"#)
                .get("error")
                .and_then(Json::as_str),
            Some("INVALID_REQUEST")
        );
        assert_eq!(
            call(&format!(
                "{{\"op\":\"scan\",\"text\":\"{}\"}}",
                "a".repeat(70_000)
            ))
            .get("error")
            .and_then(Json::as_str),
            Some("PAYLOAD_TOO_LARGE")
        );
    }

    #[test]
    fn predict_returns_the_same_candidates_as_the_engine() {
        let r = call(
            r#"{"op":"predict","id":1,"language":"rust","prefix":"fnm","before":"","after":"","dialect":{},"limit":6}"#,
        );
        let c = &r.get("candidates").and_then(Json::as_array).unwrap()[0];
        assert_eq!(c.get("id").and_then(Json::as_str), Some("rust.main#0"));
        assert_eq!(c.get("kind").and_then(Json::as_str), Some("syntax"));
        assert_eq!(c.get("validation").and_then(Json::as_str), Some("valid"));
        assert_eq!(c.get("match").and_then(Json::as_str), Some("exact"));
        assert_eq!(c.get("profile_id").and_then(Json::as_str), Some("rust"));
        assert!(
            c.get("text")
                .and_then(Json::as_str)
                .unwrap()
                .starts_with("fn main()")
        );
        assert_eq!(
            call(r#"{"op":"predict","language":"rust","prefix":"fn","limit":300}"#)
                .get("field")
                .and_then(Json::as_str),
            Some("limit")
        );
        assert_eq!(
            call(r#"{"op":"predict","language":"rust","prefix":5}"#)
                .get("field")
                .and_then(Json::as_str),
            Some("prefix")
        );
        let none = call(r#"{"op":"predict","language":"machine-code","prefix":"jmp foo"}"#);
        let m = &none.get("candidates").and_then(Json::as_array).unwrap()[0];
        assert!(m.get("text").unwrap().is_null());
    }

    #[test]
    fn predict_accepts_the_bridges_pins_and_usage_counts() {
        let ids = |r: &Json| -> Vec<String> {
            r.get("candidates")
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .map(|c| c.get("id").and_then(Json::as_str).unwrap().to_string())
                .collect()
        };
        let plain = ids(&call(
            r#"{"op":"predict","language":"rust","prefix":"fn","limit":6}"#,
        ));
        assert!(plain.len() >= 2, "{plain:?}");
        let last = plain.last().unwrap().clone();
        let pinned = ids(&call(&format!(
            r#"{{"op":"predict","language":"rust","prefix":"fn","limit":6,"pins":["{last}"]}}"#
        )));
        assert_eq!(pinned[0], last, "a pinned candidate ranks first");
        let used = ids(&call(&format!(
            r#"{{"op":"predict","language":"rust","prefix":"fn","limit":6,"usage":{{"{last}":50}}}}"#
        )));
        // usage only breaks ties between equally good candidates; the set of candidates never changes
        let (mut used_sorted, mut plain_sorted) = (used.clone(), plain.clone());
        used_sorted.sort();
        plain_sorted.sort();
        assert_eq!(used_sorted, plain_sorted);
        for bad in [
            r#""pins":"x""#,
            r#""pins":[1]"#,
            r#""usage":[]"#,
            r#""usage":{"a":"b"}"#,
        ] {
            let r = call(&format!(
                r#"{{"op":"predict","language":"rust","prefix":"fn",{bad}}}"#
            ));
            assert_eq!(
                r.get("error").and_then(Json::as_str),
                Some("INVALID_REQUEST"),
                "{bad}"
            );
        }
    }

    #[test]
    fn resolve_validate_encode_and_hex() {
        let r = call(
            r#"{"op":"resolve","languageId":"plaintext","fileName":"a.h","text":"","current":"rust","locked":null}"#,
        );
        let d = r.get("decision").unwrap();
        assert_eq!(d.get("suggestion").and_then(Json::as_str), Some("c"));
        assert_eq!(d.get("uncertain").and_then(Json::as_bool), Some(true));
        let v = call(r#"{"op":"validate","language":"rust","text":"fn main() {\n","dialect":{}}"#);
        let res = v.get("result").unwrap();
        assert_eq!(res.get("level").and_then(Json::as_str), Some("invalid"));
        assert_eq!(
            res.get("issues").and_then(Json::as_array).unwrap()[0]
                .get("code")
                .and_then(Json::as_str),
            Some("BRACKET_UNCLOSED")
        );
        assert_eq!(
            call(r#"{"op":"validate","language":"rust"}"#)
                .get("result")
                .unwrap()
                .get("issues")
                .and_then(Json::as_array)
                .unwrap()[0]
                .get("code")
                .and_then(Json::as_str),
            Some("NOT_TEXT")
        );
        assert_eq!(
            call(r#"{"op":"encode","instruction":"mov rax, rbx"}"#)
                .get("hex")
                .and_then(Json::as_str),
            Some("48 89 D8")
        );
        assert_eq!(
            call(r#"{"op":"encode","instruction":"jmp unresolved_label"}"#)
                .get("code")
                .and_then(Json::as_str),
            Some("BYTE_ENCODING_UNRESOLVED")
        );
        assert_eq!(
            call(r#"{"op":"encode","instruction":"mov rax, 1"}"#)
                .get("code")
                .and_then(Json::as_str),
            Some("UNSUPPORTED_ENCODING")
        );
        assert_eq!(
            call(r#"{"op":"validate_hex","hex":"90 C3"}"#)
                .get("byte_count")
                .and_then(Json::as_i64),
            Some(2)
        );
        assert_eq!(
            call(r#"{"op":"validate_hex","hex":"GG"}"#)
                .get("code")
                .and_then(Json::as_str),
            Some("INVALID_HEX")
        );
    }

    #[test]
    fn scan_never_echoes_secret_values() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz0123";
        let line = format!(
            r#"{{"op":"scan","id":9,"text":"key = \"{secret}\"\nAKIAABCDEFGHIJKLMNOP","fileName":""}}"#
        );
        let raw = handle_line(&line);
        assert!(
            !raw.contains(secret) && !raw.contains("AKIAABCDEFGHIJKLMNOP"),
            "response leaked a secret: {raw}"
        );
        let r = json::parse(&raw, Limits::DEFAULT).unwrap();
        let res = r.get("result").unwrap();
        assert_eq!(res.get("highest").and_then(Json::as_str), Some("high"));
        let f = res.get("findings").and_then(Json::as_array).unwrap();
        assert!(f.iter().all(|x| {
            x.as_object()
                .unwrap()
                .iter()
                .all(|(k, _)| matches!(k.as_str(), "kind" | "severity" | "line"))
        }));
    }

    #[test]
    fn serve_answers_one_line_per_request_and_bounds_line_size() {
        let mut input = Vec::new();
        input.extend_from_slice(b"{\"op\":\"health\",\"id\":1}\r\n");
        input.extend_from_slice(b"garbage\n");
        input.extend_from_slice(&vec![b'x'; MAX_LINE_BYTES + 1]);
        input.push(b'\n');
        input.extend_from_slice(b"\xff\xfe\n");
        input.extend_from_slice(b"{\"op\":\"encode\",\"instruction\":\"nop\",\"id\":2}");
        let mut out = Vec::new();
        serve(io::Cursor::new(input), &mut out).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("\"id\":1") && lines[0].contains("\"protocol\":\"0.3\""));
        assert!(lines[1].contains("INVALID_JSON"));
        assert_eq!(lines[2], r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
        assert!(lines[3].contains("INVALID_JSON"));
        assert!(
            lines[4].contains("\"hex\":\"90\"") && lines[4].contains("\"id\":2"),
            "a last line without newline is still served"
        );
    }

    #[test]
    fn a_line_of_exactly_the_limit_is_accepted() {
        let pad = "a".repeat(MAX_LINE_BYTES - r#"{"op":"health","x":""}"#.len());
        let line = format!(r#"{{"op":"health","x":"{pad}"}}"#);
        assert_eq!(line.len(), MAX_LINE_BYTES);
        let mut out = Vec::new();
        serve(io::Cursor::new(format!("{line}\n").into_bytes()), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("\"ok\":true"));
    }
}
