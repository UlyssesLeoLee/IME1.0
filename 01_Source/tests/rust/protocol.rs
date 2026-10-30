//! Protocol 0.2 integration tests: the sample request file is answered by `protocol::handle_line` and by the real
//! `pime-core` binary over stdin/stdout, with one response line per request line and the request `id` echoed.

use pime_core::json::{Json, Limits, parse};
use pime_core::protocol::{handle_line, serve};
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

fn samples() -> Vec<String> {
    let path = format!(
        "{}/../../tests/fixtures/core-jsonl-v0.2.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .expect("sample file")
        .lines()
        .map(str::to_string)
        .collect()
}

fn j(s: &str) -> Json {
    parse(s, Limits::DEFAULT).unwrap_or_else(|e| panic!("response is not JSON ({e}): {s}"))
}

fn s<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key).and_then(Json::as_str).unwrap_or("<missing>")
}

/// Checks the sample answers (index = line number - 1).
fn check_answers(lines: &[String]) {
    assert_eq!(lines.len(), 17, "one response per request line");
    let r: Vec<Json> = lines.iter().map(|l| j(l)).collect();
    // health
    assert_eq!(
        (s(&r[0], "name"), s(&r[0], "version"), s(&r[0], "protocol")),
        ("pime-core", "0.2.0", "0.3")
    );
    assert_eq!(
        r[0].get("languages").and_then(Json::as_array).map(Vec::len),
        Some(7)
    );
    // predict
    assert_eq!(
        s(
            &r[1].get("candidates").and_then(Json::as_array).unwrap()[0],
            "id"
        ),
        "rust.main#0"
    );
    assert_eq!(
        s(
            &r[2].get("candidates").and_then(Json::as_array).unwrap()[0],
            "id"
        ),
        "py.method#0"
    );
    assert_eq!(
        s(
            &r[3].get("candidates").and_then(Json::as_array).unwrap()[0],
            "id"
        ),
        "ts.generic.tsx#0"
    );
    assert_eq!(
        s(
            &r[4].get("candidates").and_then(Json::as_array).unwrap()[0],
            "text"
        ),
        "48 89 D8"
    );
    let unsupported = &r[5].get("candidates").and_then(Json::as_array).unwrap()[0];
    assert_eq!(
        (
            s(unsupported, "validation"),
            unsupported.get("text").map(Json::is_null)
        ),
        ("context_missing", Some(true))
    );
    // resolve / validate
    assert_eq!(s(r[6].get("decision").unwrap(), "suggestion"), "c");
    assert_eq!(
        s(
            &r[7]
                .get("result")
                .unwrap()
                .get("issues")
                .and_then(Json::as_array)
                .unwrap()[0],
            "code"
        ),
        "BRACKET_UNCLOSED"
    );
    assert_eq!(
        s(
            &r[8]
                .get("result")
                .unwrap()
                .get("issues")
                .and_then(Json::as_array)
                .unwrap()[0],
            "code"
        ),
        "PS7_NULL_COALESCING"
    );
    // encode / hex
    assert_eq!(s(&r[9], "hex"), "90");
    assert_eq!(
        (r[10].get("ok"), s(&r[10], "code")),
        (Some(&Json::Bool(false)), "BYTE_ENCODING_UNRESOLVED")
    );
    assert_eq!(r[11].get("byte_count").and_then(Json::as_i64), Some(2));
    assert_eq!(s(&r[12], "code"), "INVALID_HEX");
    // scan: no values, only kind/severity/line
    assert_eq!(s(r[13].get("result").unwrap(), "highest"), "medium");
    assert!(!lines[13].contains("mock-key-test-123"));
    assert_eq!(
        (
            s(r[14].get("result").unwrap(), "highest"),
            r[14].get("result").unwrap().get("sensitiveFile")
        ),
        ("high", Some(&Json::Bool(true)))
    );
    // errors
    assert_eq!(s(&r[15], "error"), "UNKNOWN_OPERATION");
    assert_eq!(s(&r[16], "error"), "INVALID_JSON");
    // ids are echoed for lines 1..=16 and absent for the unparsable line
    for (i, resp) in r.iter().enumerate().take(16) {
        assert_eq!(
            resp.get("id").and_then(Json::as_i64),
            Some(i as i64 + 1),
            "id echo on line {}",
            i + 1
        );
    }
    assert!(r[16].get("id").is_none());
}

#[test]
fn sample_requests_via_handle_line() {
    let answers: Vec<String> = samples().iter().map(|l| handle_line(l)).collect();
    check_answers(&answers);
}

#[test]
fn sample_requests_via_serve() {
    let mut out = Vec::new();
    serve(Cursor::new(samples().join("\n").into_bytes()), &mut out).unwrap();
    let lines: Vec<String> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    check_answers(&lines);
}

#[test]
fn sample_requests_via_the_real_binary() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pime-core"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pime-core");
    let input = format!("{}\n", samples().join("\n"));
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write requests");
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "the core writes nothing to stderr");
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    check_answers(&lines);
}

#[test]
fn oversized_and_invalid_lines_do_not_stop_the_server() {
    let mut input = Vec::new();
    input.extend_from_slice(&vec![b'{'; 70_000]);
    input.push(b'\n');
    input.extend_from_slice(b"{\"op\":\"health\",\"id\":5}\n");
    let mut out = Vec::new();
    serve(Cursor::new(input), &mut out).unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines[0], r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
    assert!(lines[1].contains("\"id\":5"));
}

#[test]
fn schema_file_is_valid_json_and_describes_every_op() {
    let path = format!(
        "{}/../../schemas/core-protocol-v0.2.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(path).expect("schema file");
    let schema = parse(
        &raw,
        Limits {
            max_bytes: 1 << 20,
            max_depth: 32,
        },
    )
    .expect("schema is valid JSON");
    assert_eq!(
        schema.get("$schema").and_then(Json::as_str),
        Some("https://json-schema.org/draft/2020-12/schema")
    );
    for op in [
        "health",
        "predict",
        "resolve",
        "validate",
        "encode",
        "validate_hex",
        "scan",
    ] {
        assert!(
            raw.contains(&format!("\"const\": \"{op}\"")),
            "schema lacks request op {op}"
        );
    }
    for err in ["INVALID_JSON", "PAYLOAD_TOO_LARGE", "UNKNOWN_OPERATION"] {
        assert!(raw.contains(err), "schema lacks error {err}");
    }
}
