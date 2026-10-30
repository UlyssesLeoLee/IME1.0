//! Protocol 0.3, step 1.3 (P1B): settings checks, migration, export/import and user rule packs (`settings.*`),
//! diagnostics (`diag.*`) and interface texts and UI models (`ui.*`) over the real JSON Lines dispatch. The
//! reference-parity of the results is in `parity_cfg.rs`; these tests cover the protocol behaviour: sessions, field
//! validation, the state the core keeps (the diagnostics ring), limits, and that nothing sensitive is echoed.

mod common;

use common::*;
use pime_core::json::{Json, parse};
use pime_core::ops_cfg::{BULK_OPS, OPS};
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

fn field<'a>(v: &'a Json, key: &str) -> &'a Json {
    v.get(key).unwrap_or_else(|| panic!("no {key} in {v:?}"))
}

#[test]
fn every_operation_needs_an_open_session() {
    let mut h = Harness::without_session();
    for op in OPS {
        let r = h.call(&format!(r#"{{"op":"{op}"}}"#));
        assert_eq!(err_code(&r), "NO_SESSION", "{op}");
        let r = h.call(&format!(
            r#"{{"op":"{op}","session":"{}"}}"#,
            "0".repeat(32)
        ));
        assert_eq!(err_code(&r), "NO_SESSION", "{op} with an unknown session");
    }
}

#[test]
fn settings_validate_checks_the_session_settings_or_the_given_ones() {
    let mut h = Harness::open();
    assert_eq!(
        field(&h.call(r#"{"op":"settings.validate"}"#), "issues"),
        &Json::Arr(vec![])
    );
    // every effort level, `max` included, is valid
    for level in [
        "auto", "none", "minimal", "low", "medium", "high", "xhigh", "max", "extended",
    ] {
        let r = h.call(&format!(
            r#"{{"op":"settings.validate","settings":{{"providers":{{"openai":{{"api":"openai-responses","endpoint":"https://api.openai.com/v1/responses"}}}},"scenarios":{{"realtime":{{"provider":"openai","effort":"{level}","maxOutputTokens":10,"timeoutMs":5000}}}}}}}}"#
        ));
        assert_eq!(field(&r, "issues"), &Json::Arr(vec![]), "{level}");
    }
    let r = h.call(
        r#"{"op":"settings.validate","settings":{"providers":{"p":{"api":"nope","endpoint":"http://x","apiVersion":"latest","maxTokensParam":"x"}},"scenarios":{"realtime":{"provider":"ghost","effort":"turbo","maxOutputTokens":0,"timeoutMs":1}}}}"#,
    );
    let paths: Vec<String> = field(&r, "issues")
        .as_array()
        .unwrap()
        .iter()
        .map(|i| s(i, "path").to_string())
        .collect();
    assert_eq!(
        paths,
        [
            "providers.p.api",
            "providers.p.maxTokensParam",
            "providers.p.apiVersion",
            "providers.p.endpoint",
            "scenarios.realtime.provider",
            "scenarios.realtime.effort",
            "scenarios.realtime.maxOutputTokens",
            "scenarios.realtime.timeoutMs"
        ]
    );
    // the session settings follow settings.apply
    h.call(r#"{"op":"settings.apply","values":{"scenarios":{"f1Full":{"effort":"max"}}}}"#);
    assert_eq!(
        field(&h.call(r#"{"op":"settings.validate"}"#), "issues"),
        &Json::Arr(vec![])
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.validate","settings":5}"#)),
        "INVALID_REQUEST"
    );
}

#[test]
fn settings_migrate_returns_the_new_structure_and_the_document_to_store() {
    let mut h = Harness::open(); // the manual clock stands at 1_000_000 ms
    let r = h.call(
        r#"{"op":"settings.migrate","doc":{"provider":"openai-compatible","model":"m1","effort":"max","effortCapability":"supported","userRules":[1],"theme":"x"}}"#,
    );
    assert!(is_ok(&r), "{r:?}");
    let result = field(&r, "result");
    assert_eq!(
        field(
            field(field(result, "scenarios"), "f1Full"),
            "effortCapability"
        )
        .to_json_string(),
        r#"{"state":"supported","supported":["low","medium","high"],"checkedAt":"1970-01-01T00:16:40.000Z"}"#,
        "the clock of the core, not of the caller"
    );
    assert_eq!(field(result, "conflicts"), &Json::Arr(vec![]));
    let document = field(&r, "document");
    assert_eq!(s(document, "theme"), "x");
    assert!(document.get("model").is_none() && document.get("provider").is_none());
    assert_eq!(field(document, "schema_version"), &Json::int(1));
    // a newer schema is refused with its code, a doc that is not an object is a request error
    let e = h.call(r#"{"op":"settings.migrate","doc":{"schema_version":9}}"#);
    assert_eq!(err_code(&e), "SETTINGS_INVALID");
    assert!(s(&e, "message").contains("newer than this build"));
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.migrate","doc":[]}"#)),
        "INVALID_REQUEST"
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.migrate"}"#)),
        "INVALID_REQUEST"
    );
}

#[test]
fn settings_export_and_import_round_trip_and_refuse_secrets() {
    let mut h = Harness::open();
    h.call(r#"{"op":"settings.apply","values":{"scenarios":{"realtime":{"model":"m","effort":"max"}},"budget":{"maxRequests":7}}}"#);
    let exported = h.call(r#"{"op":"settings.export"}"#);
    let text = s(&exported, "text").to_string();
    assert!(text.starts_with("{\n  \"schema_version\": 1,"), "{text}");
    let back = h.call(&format!(
        r#"{{"op":"settings.import","text":{}}}"#,
        q(&text)
    ));
    assert!(is_ok(&back), "{back:?}");
    let imported = field(&back, "settings");
    assert_eq!(
        s(field(field(imported, "scenarios"), "realtime"), "effort"),
        "max"
    );
    assert_eq!(
        field(field(imported, "budget"), "maxRequests"),
        &Json::int(7)
    );
    assert_eq!(field(&back, "notes"), &Json::Arr(vec![]));
    // importing applies nothing: the session settings are unchanged by it
    let again = h.call(r#"{"op":"settings.export"}"#);
    assert_eq!(s(&again, "text"), text);
    // secret-like data is never exported, and the error does not repeat it
    let leaky = h.call(r#"{"op":"settings.export","settings":{"providers":{"p":{"api":"openai-chat","endpoint":"https://p.example/v1","note":"sk-proj-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH"}}}}"#);
    assert_eq!(err_code(&leaky), "SETTINGS_INVALID");
    assert!(!leaky.to_json_string().contains("sk-proj"));
    // invalid imports name the offending paths
    let bad = h.call(r#"{"op":"settings.import","text":"{\"schema_version\":1,\"scenarios\":{\"realtime\":{\"effort\":\"turbo\"}}}"}"#);
    assert_eq!(err_code(&bad), "SETTINGS_INVALID");
    assert!(s(&bad, "message").contains("scenarios.realtime.effort"));
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.import","text":"{bad"}"#)),
        "SETTINGS_INVALID"
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"settings.import"}"#)),
        "INVALID_REQUEST"
    );
}

#[test]
fn rules_check_reports_accepted_and_rejected_packs_without_keeping_anything() {
    let mut h = Harness::open();
    let good = r#"{"schema_version":1,"profile_id":"python","version":"1.0.0","triggers":[{"id":"my.log","pattern":"logd","candidates":[{"label":"debug log","text":"logging.debug(\"${msg}\")"}]}]}"#;
    let evil = good.replace(r#""text":"#, r#""shell":"calc.exe","text":"#);
    let request = format!(
        r#"{{"op":"settings.rules_check","files":[{{"name":"good.json","text":{}}},{{"name":"evil.json","text":{}}},{{"name":"bad.json","text":"{{"}}]}}"#,
        q(good),
        q(&evil)
    );
    let r = h.call(&request);
    assert_eq!(field(&r, "accepted").to_json_string(), r#"["good.json"]"#);
    let rejected = field(&r, "rejected").as_array().unwrap();
    assert_eq!(rejected.len(), 2);
    assert_eq!(
        (s(&rejected[0], "name"), s(&rejected[0], "code")),
        ("evil.json", "RULEPACK_INVALID")
    );
    assert_eq!(s(&rejected[1], "message"), "not valid JSON");
    // a second check starts from the built-in rules again: the same pack is accepted again
    assert_eq!(
        field(&h.call(&request), "accepted").to_json_string(),
        r#"["good.json"]"#
    );
    // a pack over the 256 KiB limit is rejected, and the request may be a long line (bulk)
    let huge = format!(
        r#"{{"schema_version":1,"profile_id":"python","version":"1.0.0","pad":"{}"}}"#,
        "x".repeat(300_000)
    );
    let r = h.call(&format!(
        r#"{{"op":"settings.rules_check","files":[{{"name":"huge.json","text":{}}}]}}"#,
        q(&huge)
    ));
    assert_eq!(
        s(&field(&r, "rejected").as_array().unwrap()[0], "message"),
        "pack exceeds size limit"
    );
    for bad in [
        r#"{"op":"settings.rules_check"}"#,
        r#"{"op":"settings.rules_check","files":[{"name":"x"}]}"#,
        r#"{"op":"settings.rules_check","files":5}"#,
    ] {
        assert_eq!(err_code(&h.call(bad)), "INVALID_REQUEST", "{bad}");
    }
}

#[test]
fn only_the_documented_operations_may_use_long_lines() {
    assert_eq!(BULK_OPS, ["settings.import", "settings.rules_check"]);
    let mut h = Harness::open();
    let long = "x".repeat(70_000);
    let reply = h.core.handle_line(&format!(
        r#"{{"op":"ui.text","session":"{}","key":{}}}"#,
        h.session,
        q(&long)
    ));
    assert_eq!(reply, r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
}

#[test]
fn diagnostics_record_only_whitelisted_metadata_and_stay_per_session() {
    let mut h = Harness::open();
    let r = h.call(
        r#"{"op":"diag.record","event":{"component":"ai","error_code":"AI_TIMEOUT","elapsed_ms":12.6,"request_type":"f1Full","counters":{"requests":1},"input_text":"fn secret_code() {}","api_key":"sk-live-123","prompt":"p","document_uri":"file:///C:/secret.rs","error_detail":"C:\\Users\\me\\secret.rs"}}"#,
    );
    let event = field(&r, "event");
    assert_eq!(s(event, "component"), "ai");
    assert_eq!(field(event, "elapsed_ms"), &Json::int(13));
    assert_eq!(
        s(event, "error_detail"),
        "[filtered]",
        "a path is not a token"
    );
    assert_eq!(
        field(event, "timestamp"),
        &Json::int(1_000_000),
        "the clock of the core"
    );
    assert_eq!(s(event, "anonymized_session").len(), 12);
    let session_hash = s(event, "anonymized_session").to_string();
    // a timestamp that is not a number cannot carry content into the record
    let r = h.call(r#"{"op":"diag.record","event":{"timestamp":"/home/me/secret.rs","component":"edit","error_code":"EDIT_UNKNOWN"}}"#);
    assert_eq!(
        field(field(&r, "event"), "timestamp"),
        &Json::int(1_000_000)
    );
    h.clock.advance(5);
    h.call(r#"{"op":"diag.record","event":{"component":"edit"}}"#);
    let summary = h.call(r#"{"op":"diag.summary"}"#);
    let sm = field(&summary, "summary");
    assert_eq!(field(sm, "total"), &Json::int(3));
    assert_eq!(
        field(sm, "byCode").to_json_string(),
        r#"{"AI_TIMEOUT":1,"EDIT_UNKNOWN":1,"OK":1}"#
    );
    assert_eq!(
        field(sm, "lastByComponent").to_json_string(),
        r#"{"ai":{"error_code":"AI_TIMEOUT","elapsed_ms":13},"edit":{"error_code":"OK","elapsed_ms":null}}"#
    );
    let text = h.call(r#"{"op":"diag.text"}"#);
    let all = format!(
        "{}{}{}",
        s(&text, "text"),
        summary.to_json_string(),
        r.to_json_string()
    );
    for leak in [
        "secret",
        "sk-live",
        "file:///",
        "input_text",
        "api_key",
        "prompt",
        "Users",
    ] {
        assert!(!all.contains(leak), "the diagnostics leaked {leak}");
    }
    assert_eq!(s(&text, "text").lines().count(), 3);
    // another session has its own ring and its own anonymised id
    let other = h.open_another(9);
    let r = h.call(&format!(r#"{{"op":"diag.summary","session":"{other}"}}"#));
    assert_eq!(field(field(&r, "summary"), "total"), &Json::int(0));
    let r = h.call(&format!(
        r#"{{"op":"diag.record","session":"{other}","event":{{"component":"x"}}}}"#
    ));
    assert_ne!(s(field(&r, "event"), "anonymized_session"), session_hash);
    // the event must be an object
    for bad in [
        r#"{"op":"diag.record"}"#,
        r#"{"op":"diag.record","event":"x"}"#,
        r#"{"op":"diag.record","event":[1]}"#,
    ] {
        assert_eq!(err_code(&h.call(bad)), "INVALID_REQUEST", "{bad}");
    }
}

#[test]
fn the_diagnostics_ring_is_bounded() {
    let mut h = Harness::open();
    for i in 0..520 {
        h.call(&format!(
            r#"{{"op":"diag.record","event":{{"component":"c","error_code":"E{}"}}}}"#,
            i % 3
        ));
    }
    let r = h.call(r#"{"op":"diag.summary"}"#);
    assert_eq!(field(field(&r, "summary"), "total"), &Json::int(500));
}

#[test]
fn interface_texts_follow_the_session_locale_or_the_named_one() {
    let mut h = Harness::open(); // session locale en-US
    let r = h.call(r#"{"op":"ui.text","key":"f1.hunk","args":[2,7,1,3]}"#);
    assert_eq!(s(&r, "text"), "Block 2 (old lines 7, 1 removed, 3 added)");
    let r = h.call(r#"{"op":"ui.text","key":"f1.tab.full","locale":"zh-CN"}"#);
    assert!(s(&r, "text").contains("整文件"));
    let r = h.call(
        r#"{"op":"ui.text","key":"f1.hunk","locale":"ja-JP","args":[null,"{1}",true,[1,2]]}"#,
    );
    assert_eq!(
        s(&r, "text"),
        "ブロック （元の {1} 行目、削除 true、追加 1,2）"
    );
    assert_eq!(
        s(&h.call(r#"{"op":"ui.text","key":"no.such.key"}"#), "text"),
        "no.such.key"
    );
    assert_eq!(
        s(&h.call(r#"{"op":"ui.text","key":"constructor"}"#), "text"),
        "constructor"
    );
    // the session locale follows settings.apply
    h.call(r#"{"op":"settings.apply","values":{},"locale":"ja-JP"}"#);
    assert!(s(&h.call(r#"{"op":"ui.text","key":"status.off"}"#), "text").contains("オフ"));
    for bad in [
        r#"{"op":"ui.text"}"#,
        r#"{"op":"ui.text","key":5}"#,
        r#"{"op":"ui.text","key":"x","args":"a"}"#,
        r#"{"op":"ui.text","key":"x","locale":5}"#,
    ] {
        assert_eq!(err_code(&h.call(bad)), "INVALID_REQUEST", "{bad}");
    }
    // error sentences, with the fallback rules
    assert_eq!(
        s(
            &h.call(r#"{"op":"ui.error_text","code":"AI_DISABLED","locale":"en-US"}"#),
            "text"
        ),
        "AI is disabled."
    );
    assert_eq!(
        s(
            &h.call(r#"{"op":"ui.error_text","code":"NOPE","message":"raw","locale":"en-US"}"#),
            "text"
        ),
        "raw"
    );
    assert_eq!(
        s(
            &h.call(r#"{"op":"ui.error_text","code":"NOPE","locale":"en-US"}"#),
            "text"
        ),
        "NOPE"
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"ui.error_text"}"#)),
        "INVALID_REQUEST"
    );
    // the locale of the editor language
    for (request, want) in [
        (
            r#"{"op":"ui.resolve_locale","setting":"zh-CN","language":"ja"}"#,
            "zh-CN",
        ),
        (
            r#"{"op":"ui.resolve_locale","setting":"auto","language":"ja-JP"}"#,
            "ja-JP",
        ),
        (
            r#"{"op":"ui.resolve_locale","setting":"auto","language":"zh-cn"}"#,
            "zh-CN",
        ),
        (
            r#"{"op":"ui.resolve_locale","setting":"auto","language":"fr"}"#,
            "en-US",
        ),
        (r#"{"op":"ui.resolve_locale","setting":"auto"}"#, "en-US"),
        (r#"{"op":"ui.resolve_locale"}"#, "en-US"),
    ] {
        assert_eq!(s(&h.call(request), "locale"), want, "{request}");
    }
}

#[test]
fn bundles_carry_the_whole_table_or_the_asked_keys() {
    let mut h = Harness::open();
    let all = h.call(r#"{"op":"ui.bundle","locale":"zh-CN"}"#);
    let table = field(&all, "bundle").as_object().unwrap();
    assert_eq!(table.len(), 104);
    assert_eq!(s(field(&all, "bundle"), "status.off"), "关闭");
    // placeholders are blank unless the raw templates are asked for
    let some = h.call(r#"{"op":"ui.bundle","keys":["f1.hunk","status.off","10","2"]}"#);
    assert_eq!(
        field(&some, "bundle").to_json_string(),
        r#"{"2":"2","10":"10","f1.hunk":"Block  (old lines ,  removed,  added)","status.off":"OFF"}"#
    );
    let raw = h.call(r#"{"op":"ui.bundle","keys":["f1.hunk"],"raw":true,"locale":"en-US"}"#);
    assert_eq!(
        s(field(&raw, "bundle"), "f1.hunk"),
        "Block {0} (old lines {1}, {2} removed, {3} added)"
    );
    let errors = h.call(r#"{"op":"ui.errors","locale":"ja-JP"}"#);
    assert_eq!(field(&errors, "errors").as_object().unwrap().len(), 31);
    assert_eq!(
        s(field(&errors, "errors"), "AI_DISABLED"),
        "AI は無効です。"
    );
    for bad in [
        r#"{"op":"ui.bundle","keys":"x"}"#,
        r#"{"op":"ui.bundle","keys":[1]}"#,
        r#"{"op":"ui.bundle","raw":"yes"}"#,
    ] {
        assert_eq!(err_code(&h.call(bad)), "INVALID_REQUEST", "{bad}");
    }
}

#[test]
fn candidate_rows_are_numbered_and_described_in_the_locale() {
    let mut h = Harness::open();
    let r = h.call(
        r#"{"op":"ui.candidate_rows","locale":"en-US","candidates":[
            {"label":"main function","source":"rule","validation":"valid","text":"fn main() {}","explain":"Entry point","assumptions":["a"]},
            {"label":"jmp foo","source":"rule","sources":["rule","ai"],"validation":"context_missing","text":null}]}"#,
    );
    let rows = field(&r, "rows").as_array().unwrap();
    assert_eq!(s(&rows[0], "description"), "[local rule] syntax ok");
    assert_eq!(s(&rows[0], "detail"), "Entry point · a");
    assert_eq!(
        s(&rows[0], "ariaLabel"),
        "1 / 2: main function, local rule, syntax ok"
    );
    assert_eq!(
        s(&rows[1], "description"),
        "[local rule +1] context missing"
    );
    assert_eq!(
        s(&rows[1], "ariaLabel"),
        "2 / 2: jmp foo, local rule +1, context missing, not insertable"
    );
    assert_eq!(field(&rows[1], "insertable"), &Json::Bool(false));
    let zh = h.call(r#"{"op":"ui.candidate_rows","locale":"zh-CN","candidates":[{"label":"x","source":"ai","validation":"unchecked","text":"t"}]}"#);
    assert_eq!(
        s(&field(&zh, "rows").as_array().unwrap()[0], "description"),
        "[AI] 未检查"
    );
    assert_eq!(
        field(
            &h.call(r#"{"op":"ui.candidate_rows","candidates":[]}"#),
            "rows"
        ),
        &Json::Arr(vec![])
    );
    for bad in [
        r#"{"op":"ui.candidate_rows"}"#,
        r#"{"op":"ui.candidate_rows","candidates":[{"source":"ai"}]}"#,
        r#"{"op":"ui.candidate_rows","candidates":[{"label":"x","assumptions":"a"}]}"#,
        r#"{"op":"ui.candidate_rows","candidates":[{"label":"x","source":["ai"]}]}"#,
        r#"{"op":"ui.candidate_rows","candidates":[5]}"#,
    ] {
        assert_eq!(err_code(&h.call(bad)), "INVALID_REQUEST", "{bad}");
    }
}

#[test]
fn overlay_placement_and_the_environment_check() {
    let mut h = Harness::open();
    let monitors = r#"[{"x":-1920,"y":0,"w":1920,"h":1080},{"x":0,"y":0,"w":2560,"h":1440}]"#;
    let r = h.call(&format!(r#"{{"op":"ui.place_overlay","caret":{{"x":100,"y":1430}},"size":{{"w":320,"h":200}},"monitors":{monitors},"dpi":250}}"#));
    assert_eq!(
        field(&r, "placement").to_json_string(),
        r#"{"x":100,"y":928,"w":800,"h":500}"#
    );
    let off = h.call(&format!(r#"{{"op":"ui.place_overlay","caret":{{"x":99999,"y":5}},"size":{{"w":320,"h":200}},"monitors":{monitors}}}"#));
    assert_eq!(
        field(&off, "placement"),
        &Json::Null,
        "hide, never steal focus"
    );
    for bad in [
        r#"{"op":"ui.place_overlay"}"#,
        r#"{"op":"ui.place_overlay","caret":{"x":1},"size":{"w":1,"h":1},"monitors":[]}"#,
        r#"{"op":"ui.place_overlay","caret":{"x":1,"y":1},"size":{"w":1,"h":1},"monitors":[{"x":0}]}"#,
        r#"{"op":"ui.place_overlay","caret":{"x":1,"y":1},"size":{"w":1,"h":1},"monitors":[],"dpi":"150"}"#,
    ] {
        assert_eq!(err_code(&h.call(bad)), "INVALID_REQUEST", "{bad}");
    }
    let ok =
        h.call(r#"{"op":"ui.check_env","platform":"win32","arch":"x64","build":22631,"dpi":150}"#);
    assert_eq!(
        (field(&ok, "supported"), field(&ok, "reasons")),
        (&Json::Bool(true), &Json::Arr(vec![]))
    );
    let no = h
        .call(r#"{"op":"ui.check_env","platform":"linux","arch":"arm64","build":19045,"dpi":300}"#);
    assert_eq!(field(&no, "supported"), &Json::Bool(false));
    assert_eq!(field(&no, "reasons").as_array().unwrap().len(), 4);
}

#[test]
fn the_schema_documents_every_new_operation() {
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
        assert!(
            raw.contains(&format!("\"title\": \"{op}\"")),
            "schema lacks the response of {op}"
        );
    }
}

#[test]
fn the_real_binary_serves_settings_diagnostics_and_texts() {
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
    let t = ask(&format!(
        r#"{{"op":"ui.text","session":"{sid}","id":2,"key":"cand.pick","args":["rust","fnm"]}}"#
    ));
    assert_eq!(s(&t, "text"), "rust: \"fnm\" - local candidates");
    let d = ask(&format!(
        r#"{{"op":"diag.record","session":"{sid}","id":3,"event":{{"component":"ai","error_code":"AI_TIMEOUT"}}}}"#
    ));
    assert!(
        field(field(&d, "event"), "timestamp").as_f64().unwrap() > 1.0e12,
        "the wall clock"
    );
    let m = ask(&format!(
        r#"{{"op":"settings.migrate","session":"{sid}","id":4,"doc":{{"effort":"max"}}}}"#
    ));
    assert_eq!(
        field(
            field(field(field(&m, "result"), "scenarios"), "realtime"),
            "effort"
        ),
        &Json::str("max")
    );
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "the core writes nothing to stderr");
}
