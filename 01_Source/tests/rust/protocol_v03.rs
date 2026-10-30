//! Protocol 0.3 session and settings tests over the real JSON Lines dispatch (`Core::handle_line`, `serve_with`
//! and the real `pime-core` binary), with an injected clock. The edit path, grants and budget are in
//! `protocol_v03_edit.rs`.

mod common;

use common::*;
use pime_core::json::{Json, Limits, parse};
use pime_core::protocol::{Core, serve_with};
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

#[test]
fn health_reports_protocol_0_3_and_needs_no_session() {
    let mut h = Harness::without_session();
    let r = h.call(r#"{"op":"health"}"#);
    assert_eq!((s(&r, "protocol"), s(&r, "name")), ("0.3", "pime-core"));
}

#[test]
fn session_open_returns_an_id_and_the_protocol() {
    let mut h = Harness::without_session();
    let r = h.call(&format!(
        r#"{{"op":"session.open","locale":"ja-JP","entropy":"{ENTROPY}","settings":{{"budget":{{"maxRequests":3}}}}}}"#
    ));
    assert!(is_ok(&r), "{r:?}");
    assert_eq!(s(&r, "protocol"), "0.3");
    let id = s(&r, "session");
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        r.get("issues").and_then(Json::as_array).map(Vec::len),
        Some(0)
    );
    assert_eq!(h.core.open_sessions(), 1);
}

#[test]
fn session_open_validates_locale_entropy_and_settings() {
    let mut h = Harness::without_session();
    let bad = |h: &mut Harness, body: &str, field: &str| {
        let r = h.call(body);
        assert_eq!(err_code(&r), "INVALID_REQUEST", "{body}");
        assert_eq!(s(&r, "field"), field, "{body}");
    };
    bad(&mut h, r#"{"op":"session.open","entropy":"00"}"#, "locale");
    bad(
        &mut h,
        r#"{"op":"session.open","locale":"en-US"}"#,
        "entropy",
    );
    bad(
        &mut h,
        r#"{"op":"session.open","locale":"en-US","entropy":"abcd"}"#,
        "entropy",
    );
    bad(
        &mut h,
        &format!(
            r#"{{"op":"session.open","locale":"en-US","entropy":"{}"}}"#,
            "00".repeat(40)
        ),
        "entropy",
    );
    bad(
        &mut h,
        &format!(
            r#"{{"op":"session.open","locale":"en-US","entropy":"{}"}}"#,
            "zz".repeat(32)
        ),
        "entropy",
    );
    bad(
        &mut h,
        &format!(
            r#"{{"op":"session.open","locale":"en-US","entropy":"{ENTROPY}","settings":{{"providers":5}}}}"#
        ),
        "settings.providers",
    );
    assert_eq!(h.core.open_sessions(), 0);
}

#[test]
fn the_same_entropy_cannot_open_two_sessions() {
    let mut h = Harness::open();
    let r = h.call(&format!(
        r#"{{"op":"session.open","locale":"en-US","entropy":"{ENTROPY}"}}"#
    ));
    assert_eq!(
        (err_code(&r), s(&r, "field")),
        ("INVALID_REQUEST", "entropy")
    );
    let other = h.call(&format!(
        r#"{{"op":"session.open","locale":"en-US","entropy":"{}"}}"#,
        entropy(9)
    ));
    assert!(is_ok(&other));
    assert_ne!(s(&other, "session"), h.session);
    assert_eq!(h.core.open_sessions(), 2);
}

#[test]
fn every_0_3_operation_except_health_and_open_needs_a_live_session() {
    let mut h = Harness::without_session();
    for op in ["settings.apply", "settings.resolve", "session.close"] {
        let r = h.call(&format!(r#"{{"op":"{op}"}}"#));
        assert_eq!(err_code(&r), "NO_SESSION", "{op} without a session");
        let r = h.call(&format!(
            r#"{{"op":"{op}","session":"{}"}}"#,
            "ab".repeat(16)
        ));
        assert_eq!(err_code(&r), "NO_SESSION", "{op} with an unknown session");
        let r = h.call(&format!(r#"{{"op":"{op}","session":5}}"#));
        assert_eq!(err_code(&r), "NO_SESSION", "{op} with a non-string session");
    }
}

#[test]
fn an_idle_session_expires_and_use_keeps_it_alive() {
    let mut h = Harness::open();
    let day = 24 * 3600 * 1000;
    h.clock.advance(day - 1000);
    assert!(is_ok(
        &h.call(r#"{"op":"settings.resolve","scenario":"f1Full"}"#)
    ));
    h.clock.advance(day - 1000);
    assert!(
        is_ok(&h.call(r#"{"op":"settings.resolve","scenario":"f1Full"}"#)),
        "use refreshed it"
    );
    h.clock.advance(day + 5000);
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.resolve","scenario":"f1Full"}"#)),
        "NO_SESSION"
    );
}

#[test]
fn session_close_forgets_the_session() {
    let mut h = Harness::open();
    assert!(is_ok(&h.call(r#"{"op":"session.close"}"#)));
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.apply","values":{}}"#)),
        "NO_SESSION"
    );
}

#[test]
fn settings_apply_returns_effective_settings_and_issues() {
    let mut h = Harness::open();
    let r = h.call(
        r#"{"op":"settings.apply","values":{"providers":{"mockA":{"api":"openai-responses","endpoint":"https://127.0.0.1:9/v1/responses"}},
           "scenarios":{"f1Full":{"provider":"mockA","model":"m-1"},"realtime":{"timeoutMs":5}},"budget":{"maxRequests":2}}}"#,
    );
    assert!(is_ok(&r), "{r:?}");
    let settings = r.get("settings").unwrap();
    assert_eq!(
        settings
            .get("scenarios")
            .unwrap()
            .get("f1Full")
            .unwrap()
            .get("model")
            .and_then(Json::as_str),
        Some("m-1")
    );
    assert_eq!(
        settings.get("budget").unwrap().to_json_string(),
        r#"{"maxRequests":2,"maxTokens":400000,"pricePer1kTokens":0}"#
    );
    let issues = r.get("issues").and_then(Json::as_array).unwrap();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(s(&issues[0], "path"), "scenarios.realtime.timeoutMs");

    // the stored settings drive scenario resolution
    let r = h.call(r#"{"op":"settings.resolve","scenario":"f1Full"}"#);
    let sc = r.get("scenario").unwrap();
    assert_eq!(
        (s(sc, "providerId"), s(sc, "family"), s(sc, "model")),
        ("mockA", "openai-responses", "m-1")
    );
    let r = h.call(r#"{"op":"settings.resolve","scenario":"nope"}"#);
    assert_eq!(err_code(&r), "SETTINGS_INVALID");
    let r = h.call(
        r#"{"op":"settings.apply","values":{"scenarios":{"f1Insert":{"provider":"ghost"}}}}"#,
    );
    assert!(is_ok(&r));
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.resolve","scenario":"f1Insert"}"#)),
        "MODEL_UNSUPPORTED"
    );
    assert_eq!(
        s(&h.call(r#"{"op":"settings.apply","values":[]}"#), "field"),
        "values"
    );
}

#[test]
fn the_0_2_operations_work_unchanged_next_to_sessions() {
    let mut h = Harness::open();
    let r = h.call(r#"{"op":"encode","instruction":"nop"}"#);
    assert_eq!(s(&r, "hex"), "90");
    let r = h.call(r#"{"op":"predict","language":"rust","prefix":"fnm","limit":2}"#);
    assert_eq!(
        s(
            &r.get("candidates").and_then(Json::as_array).unwrap()[0],
            "id"
        ),
        "rust.main#0"
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"edit.nope"}"#)),
        "UNKNOWN_OPERATION"
    );
}

#[test]
fn only_operations_that_carry_document_text_may_use_long_lines() {
    let mut h = Harness::open();
    // a 0.2 operation over 64 KiB is refused exactly as before, a stateful non-text operation too
    let big = "a".repeat(70_000);
    let reply = h
        .core
        .handle_line(&format!(r#"{{"op":"scan","text":"{big}"}}"#));
    assert_eq!(reply, r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
    let reply = h.core.handle_line(&format!(
        r#"{{"op":"settings.apply","session":"{}","values":{{"x":"{big}"}}}}"#,
        h.session
    ));
    assert_eq!(reply, r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
    // junk over the limit stays PAYLOAD_TOO_LARGE, not INVALID_JSON
    assert_eq!(
        h.core.handle_line(&"x".repeat(70_000)),
        r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#
    );
}

#[test]
fn serve_with_keeps_session_state_across_lines_and_the_binary_does_too() {
    let input = format!(
        "{}\n{}\n",
        format_args!(r#"{{"op":"session.open","locale":"en-US","entropy":"{ENTROPY}","id":1}}"#),
        r#"{"op":"settings.resolve","scenario":"f1Full","session":"deadbeef","id":2}"#
    );
    let mut out = Vec::new();
    serve_with(
        &mut Core::new(),
        Cursor::new(input.clone().into_bytes()),
        &mut out,
    )
    .unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(r#""protocol":"0.3""#));
    assert!(lines[1].contains("NO_SESSION"));

    // the real process: open a session, then use the id it returned
    let mut child = Command::new(env!("CARGO_BIN_EXE_pime-core"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pime-core");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut reader = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    use std::io::BufRead;
    stdin
        .write_all(
            format!(r#"{{"op":"session.open","locale":"en-US","entropy":"{ENTROPY}","id":1}}"#)
                .as_bytes(),
        )
        .unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let sid = s(&parse(&line, Limits::DEFAULT).unwrap(), "session").to_string();
    line.clear();
    stdin
        .write_all(
            format!(
                r#"{{"op":"settings.resolve","scenario":"realtime","session":"{sid}","id":2}}"#
            )
            .as_bytes(),
        )
        .unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let r = parse(&line, Limits::DEFAULT).unwrap();
    assert!(is_ok(&r), "{line}");
    assert_eq!(s(r.get("scenario").unwrap(), "scenario"), "realtime");
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "the core writes nothing to stderr");
}
