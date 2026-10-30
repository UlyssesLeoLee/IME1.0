//! Protocol 0.3 integration tests for the edit path (snapshots, proposals, plans, Prepare/Apply), grants and budget,
//! driven over the real JSON Lines dispatch with an injected clock, plus the sample request file run through
//! `Core::handle_line` and through the real `pime-core` binary.

mod common;

use common::*;
use pime_core::json::{Json, parse};
use pime_core::protocol::Core;
use pime_core::sha256::sha256_hex;
use pime_core::textrange::{Edit, apply_edits};
use std::io::Write;
use std::process::{Command, Stdio};

const URI: &str = "file:///w/a.rs";
const SEL0: &str = r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}"#;
const BASE: &str = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";
const PROPOSED: &str = "fn a() { 1 }\n\nfn b() {}\n\nfn c() { 3 }\n";

fn state(text: &str, version: i64) -> String {
    format!(
        r#"{{"uri":{},"version":{version},"text":{}}}"#,
        q(URI),
        q(text)
    )
}

impl Harness {
    fn snapshot(&mut self, text: &str, version: i64) -> String {
        let r = self.call(&format!(
            r#"{{"op":"ctx.snapshot","uri":{},"version":{version},"text":{},"languageId":"rust","selection":{SEL0},"isDirty":true,"fileName":"a.rs"}}"#,
            q(URI),
            q(text)
        ));
        assert!(is_ok(&r), "{r:?}");
        s(r.get("snapshot").unwrap(), "snapshot_id").to_string()
    }

    fn authorize(&mut self, scope: &str, once: bool) -> String {
        let r = self.call(&format!(
            r#"{{"op":"grant.authorize","provider_id":"mock:127.0.0.1:9","workspace_uris":["file:///w"],"scope":"{scope}","once":{once}}}"#
        ));
        assert!(is_ok(&r), "{r:?}");
        s(r.get("grant").unwrap(), "id").to_string()
    }

    fn propose(&mut self, snap: &str, text: &str) -> Json {
        let r = self.call(&format!(
            r#"{{"op":"edit.propose","snapshot_id":"{snap}","proposed_text":{},"language":"rust"}}"#,
            q(text)
        ));
        assert!(is_ok(&r), "{:?}", s(&r, "error"));
        r
    }

    fn plan_full(&mut self, proposal: &str, ids: &str) -> String {
        let r = self.call(&format!(
            r#"{{"op":"edit.plan","kind":"full","proposal":"{proposal}","hunk_ids":{ids}}}"#
        ));
        assert!(is_ok(&r), "{r:?}");
        s(r.get("plan").unwrap(), "id").to_string()
    }

    fn prepare(&mut self, plan: &str, st: &str, extra: &str) -> Json {
        self.call(&format!(
            r#"{{"op":"edit.prepare","plan":"{plan}","state":{st}{extra}}}"#
        ))
    }

    fn begin(&mut self, token: &str, st: &str) -> Json {
        self.call(&format!(
            r#"{{"op":"edit.begin_apply","token":"{token}","state":{st}}}"#
        ))
    }

    fn status(&mut self, plan: &str) -> String {
        let r = self.call(&format!(r#"{{"op":"edit.status","plan":"{plan}"}}"#));
        s(&r, "status").to_string()
    }
}

fn edits_of(r: &Json) -> Vec<Edit> {
    r.get("edits")
        .and_then(Json::as_array)
        .unwrap()
        .iter()
        .map(|e| Edit {
            start: e.get("start").and_then(Json::as_i64).unwrap() as usize,
            end: e.get("end").and_then(Json::as_i64).unwrap() as usize,
            text: s(e, "text").to_string(),
        })
        .collect()
}

fn int(v: &Json, key: &str) -> Option<i64> {
    v.get(key).and_then(Json::as_i64)
}

#[test]
fn the_whole_f1_edit_flow_over_jsonl() {
    let mut h = Harness::open();
    let snap = h.snapshot(BASE, 7);
    let meta = h.call(&format!(
        r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":14,"before":4,"after":6}}"#
    ));
    let ex = meta.get("excerpt").unwrap();
    assert_eq!(s(ex, "text"), "\nfn b() {}");
    assert_eq!(int(ex, "cursorInExcerpt"), Some(4));
    assert_eq!(ex.get("truncated"), Some(&Json::Bool(true)));

    let prop = h.propose(&snap, PROPOSED);
    let proposal = s(&prop, "proposal").to_string();
    let hunks = prop.get("hunks").and_then(Json::as_array).unwrap();
    assert_eq!(hunks.len(), 2);
    assert_eq!(s(&hunks[0], "id"), "h1");
    assert_eq!(s(&hunks[0], "header"), "@@ -1,1 +1,1 @@");
    assert_eq!(int(&hunks[1], "oldStartLine"), Some(5));
    let lines = hunks[0].get("lines").and_then(Json::as_array).unwrap();
    let rendered: Vec<String> = lines
        .iter()
        .map(|l| format!("{}{}", s(l, "kind"), s(l, "text")))
        .collect();
    assert_eq!(rendered, ["-fn a() {}", "+fn a() { 1 }", " ", " fn b() {}"]);
    assert_eq!(s(&prop, "after_hash"), sha256_hex(PROPOSED));
    let stats = prop.get("stats").unwrap();
    assert_eq!(
        (
            int(stats, "hunks"),
            int(stats, "added"),
            int(stats, "removed")
        ),
        (Some(2), Some(2), Some(2))
    );
    let all = prop.get("result").unwrap();
    assert_eq!(s(all, "hash"), sha256_hex(PROPOSED));
    assert_eq!(s(all.get("validation").unwrap(), "level"), "valid");

    // keep only the first hunk
    let sel = h.call(&format!(
        r#"{{"op":"edit.select","proposal":"{proposal}","hunk_ids":["h1"]}}"#
    ));
    let expected = "fn a() { 1 }\n\nfn b() {}\n\nfn c() {}\n";
    let result = sel.get("result").unwrap();
    assert_eq!(s(result, "hash"), sha256_hex(expected));
    assert_eq!(s(result, "preview"), expected);
    assert_eq!(int(result, "length"), Some(expected.len() as i64));
    assert_eq!(result.get("truncated"), Some(&Json::Bool(false)));
    let full = h.call(&format!(
        r#"{{"op":"edit.result","proposal":"{proposal}","hunk_ids":["h1"]}}"#
    ));
    assert_eq!(s(&full, "text"), expected);

    let grant = h.authorize("L3", false);
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let tok = h.prepare(
        &plan,
        &state(BASE, 7),
        &format!(r#","confirmed":true,"grant_id":"{grant}""#),
    );
    assert!(is_ok(&tok), "{tok:?}");
    let token = s(&tok, "token").to_string();
    assert_eq!(token.len(), 32);

    let begin = h.begin(&token, &state(BASE, 7));
    assert!(is_ok(&begin), "{begin:?}");
    assert_eq!(int(&begin, "apply_timeout_ms"), Some(5000));
    // the editor writes the edits (UTF-16 offsets) as ONE change
    let after = apply_edits(BASE, &edits_of(&begin)).unwrap();
    assert_eq!(after, expected);
    // replaying the token never reaches the editor again
    let replay = h.begin(&token, &state(BASE, 7));
    assert_eq!(err_code(&replay), "TOKEN_INVALID");
    let fin = h.call(&format!(
        r#"{{"op":"edit.finish_apply","token":"{token}","outcome":"accepted","state":{}}}"#,
        state(&after, 8)
    ));
    assert!(is_ok(&fin), "{fin:?}");
    assert_eq!(s(&fin, "state"), "APPLIED");
    assert_eq!(int(&fin, "newVersion"), Some(8));
    assert_eq!(s(&fin, "newHash"), sha256_hex(expected));
    assert_eq!(s(&fin, "newHash"), s(result, "hash"));
    assert_eq!(h.status(&plan), "APPLIED");
    assert_eq!(h.status("0000"), "UNKNOWN_PLAN");
    // purge drops the applied plan and the spent token
    assert!(is_ok(&h.call(r#"{"op":"edit.purge"}"#)));
    assert_eq!(h.status(&plan), "UNKNOWN_PLAN");
}

#[test]
fn only_the_literal_true_confirms_and_every_check_stops_the_prepare() {
    let mut h = Harness::open();
    let snap = h.snapshot(BASE, 7);
    let proposal = s(&h.propose(&snap, PROPOSED), "proposal").to_string();
    let grant = h.authorize("L3", false);
    let plan = h.plan_full(&proposal, r#"["h1","h2"]"#);
    let g = format!(r#","grant_id":"{grant}""#);
    let ok = format!(r#","confirmed":true{g}"#);
    let st = state(BASE, 7);
    for confirmed in [
        r#""#,
        r#","confirmed":"yes""#,
        r#","confirmed":1"#,
        r#","confirmed":null"#,
        r#","confirmed":false"#,
    ] {
        let r = h.prepare(&plan, &st, &format!("{confirmed}{g}"));
        assert_eq!(err_code(&r), "USER_CONFIRMATION_REQUIRED", "{confirmed:?}");
    }
    // grant: none, unknown, and a bare `grant_ok` that is not the literal true
    let r = h.prepare(&plan, &st, r#","confirmed":true"#);
    assert_eq!(err_code(&r), "GRANT_DENIED");
    let r = h.prepare(&plan, &st, r#","confirmed":true,"grant_id":"nope""#);
    assert_eq!(err_code(&r), "GRANT_DENIED");
    let r = h.prepare(&plan, &st, r#","confirmed":true,"grant_ok":"true""#);
    assert_eq!(err_code(&r), "GRANT_DENIED");
    // stale content with the same version, another version, another uri
    let r = h.prepare(
        &plan,
        &state("fn a() {}\n\nfn b() {}\n\nfn X() {}\n", 7),
        &ok,
    );
    assert_eq!(err_code(&r), "STALE_DOCUMENT");
    assert_eq!(h.status(&plan), "STALE");
    let plan = h.plan_full(&proposal, r#"["h1","h2"]"#);
    assert_eq!(
        err_code(&h.prepare(&plan, &state(BASE, 8), &ok)),
        "STALE_DOCUMENT"
    );
    let plan = h.plan_full(&proposal, r#"["h1","h2"]"#);
    let other = st.replace("file:///w/a.rs", "file:///w/other.rs");
    assert_eq!(err_code(&h.prepare(&plan, &other, &ok)), "STALE_DOCUMENT");
    // closed, no state at all, read-only
    let plan = h.plan_full(&proposal, r#"["h1","h2"]"#);
    let closed = st.replace(r#""version":7"#, r#""version":7,"isClosed":true"#);
    assert_eq!(err_code(&h.prepare(&plan, &closed, &ok)), "DOCUMENT_CLOSED");
    let r = h.call(&format!(r#"{{"op":"edit.prepare","plan":"{plan}"{ok}}}"#));
    assert_eq!(err_code(&r), "DOCUMENT_CLOSED", "no state at all");
    let read_only = st.replace(r#""version":7"#, r#""version":7,"editable":false"#);
    assert_eq!(
        err_code(&h.prepare(&plan, &read_only, &ok)),
        "EDIT_REJECTED"
    );
    // validation
    let r = h.prepare(
        &plan,
        &st,
        &format!(r#","confirmed":true,"validation_ok":false{g}"#),
    );
    assert_eq!(err_code(&r), "VALIDATION_FAILED");
    // an unknown plan
    let r = h.prepare("00000000000000000000000000000000", &st, &ok);
    assert_eq!(err_code(&r), "TOKEN_INVALID");
    // the good one still works afterwards (validation_ok null or absent is fine)
    let r = h.prepare(
        &plan,
        &st,
        &format!(r#","confirmed":true,"validation_ok":null{g}"#),
    );
    assert!(is_ok(&r), "{r:?}");
    // a malformed state object is a protocol error, not a silent pass
    let r = h.call(&format!(
        r#"{{"op":"edit.prepare","plan":"{plan}"{ok},"state":{{"uri":"u"}}}}"#
    ));
    assert_eq!(
        (err_code(&r), s(&r, "field")),
        ("INVALID_REQUEST", "state.version")
    );
}

#[test]
fn invalidation_and_revoked_grants_block_the_apply() {
    let mut h = Harness::open();
    let snap = h.snapshot(BASE, 7);
    let proposal = s(&h.propose(&snap, PROPOSED), "proposal").to_string();
    let grant = h.authorize("L3", false);
    let ok = format!(r#","confirmed":true,"grant_id":"{grant}""#);
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    let inv = h.call(&format!(r#"{{"op":"edit.invalidate","uri":{}}}"#, q(URI)));
    assert_eq!(int(&inv, "invalidated"), Some(1));
    let r = h.begin(&token, &state(BASE, 7));
    assert_eq!(err_code(&r), "STALE_DOCUMENT");
    assert_eq!(h.status(&plan), "STALE");
    // a revoked grant stops a new prepare
    let plan2 = h.plan_full(&proposal, r#"["h1"]"#);
    let revoke = h.call(&format!(r#"{{"op":"grant.revoke","grant_id":"{grant}"}}"#));
    assert_eq!(revoke.get("revoked"), Some(&Json::Bool(true)));
    assert_eq!(
        err_code(&h.prepare(&plan2, &state(BASE, 7), &ok)),
        "GRANT_DENIED"
    );
    // a used one-request grant is still honoured by prepare until it is revoked
    let once = h.authorize("L3", true);
    assert!(is_ok(&h.call(&format!(
        r#"{{"op":"grant.consume","grant_id":"{once}"}}"#
    ))));
    let r = h.prepare(
        &plan2,
        &state(BASE, 7),
        &format!(r#","confirmed":true,"grant_id":"{once}""#),
    );
    assert!(is_ok(&r), "{r:?}");
}

#[test]
fn a_write_that_is_never_reported_is_edit_unknown_and_time_comes_from_the_clock() {
    let mut h = Harness::open();
    let snap = h.snapshot(BASE, 7);
    let proposal = s(&h.propose(&snap, PROPOSED), "proposal").to_string();
    let grant = h.authorize("L3", false);
    let ok = format!(r#","confirmed":true,"grant_id":"{grant}""#);
    let plan = h.plan_full(&proposal, r#"["h1","h2"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    assert!(is_ok(&h.begin(&token, &state(BASE, 7))));
    h.clock.advance(5001);
    let finish = |h: &mut Harness, token: &str, outcome: &str, st: &str| {
        h.call(&format!(
            r#"{{"op":"edit.finish_apply","token":"{token}","outcome":"{outcome}"{st}}}"#
        ))
    };
    let after = format!(r#","state":{}"#, state(PROPOSED, 8));
    let r = finish(&mut h, &token, "accepted", &after);
    assert_eq!(
        err_code(&r),
        "EDIT_UNKNOWN",
        "a late finish is never APPLIED"
    );
    assert_eq!(h.status(&plan), "UNKNOWN");
    assert_eq!(
        err_code(&h.prepare(&plan, &state(BASE, 7), &ok)),
        "EDIT_UNKNOWN"
    );

    // "error" (the editor threw or the bridge gave up) and "rejected"
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    h.begin(&token, &state(BASE, 7));
    assert_eq!(
        err_code(&finish(&mut h, &token, "error", "")),
        "EDIT_UNKNOWN"
    );
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    h.begin(&token, &state(BASE, 7));
    let untouched = format!(r#","state":{}"#, state(BASE, 7));
    assert_eq!(
        err_code(&finish(&mut h, &token, "rejected", &untouched)),
        "EDIT_REJECTED"
    );
    // a result that does not match the proposal is unknown, not applied
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    h.begin(&token, &state(BASE, 7));
    let wrong = format!(r#","state":{}"#, state("something else\n", 8));
    assert_eq!(
        err_code(&finish(&mut h, &token, "accepted", &wrong)),
        "EDIT_UNKNOWN"
    );
    let r = finish(&mut h, &token, "maybe", "");
    assert_eq!(
        (err_code(&r), s(&r, "field")),
        ("INVALID_REQUEST", "outcome")
    );
    // a finish without a begin is refused
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    assert_eq!(
        err_code(&finish(&mut h, &token, "accepted", &after)),
        "TOKEN_INVALID"
    );

    // a plan expires ten minutes after it was created, a token after one minute
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    h.clock.advance(10 * 60 * 1000);
    assert_eq!(
        err_code(&h.prepare(&plan, &state(BASE, 7), &ok)),
        "STALE_RESULT"
    );
    let plan = h.plan_full(&proposal, r#"["h1"]"#);
    let token = s(&h.prepare(&plan, &state(BASE, 7), &ok), "token").to_string();
    h.clock.advance(60 * 1000);
    assert_eq!(err_code(&h.begin(&token, &state(BASE, 7))), "TOKEN_INVALID");
}

#[test]
fn the_insert_path_uses_explicit_edits_and_a_selection() {
    let mut h = Harness::open();
    let text = "one\ntwo\nthree \u{1F600} four\nfive\n";
    let snap = h.snapshot(text, 3);
    let grant = h.authorize("L1", false);
    let sel = r#"{"start":{"line":2,"character":6},"end":{"line":2,"character":6}}"#;
    let off = "one\ntwo\nthree ".encode_utf16().count();
    let make_plan = |h: &mut Harness| {
        let r = h.call(&format!(
            r#"{{"op":"edit.plan","kind":"insert","snapshot_id":"{snap}","targets":[{}],"selection":{sel},"edits":[{{"start":{off},"end":{off},"text":{}}}]}}"#,
            q(URI),
            q("\u{1F600}\n")
        ));
        assert!(is_ok(&r), "{r:?}");
        s(r.get("plan").unwrap(), "id").to_string()
    };
    let st = |selection: &str| {
        format!(
            r#"{{"uri":{},"version":3,"text":{},"selection":{selection}}}"#,
            q(URI),
            q(text)
        )
    };
    let ok = format!(r#","confirmed":true,"grant_id":"{grant}""#);
    let moved = r#"{"start":{"line":3,"character":0},"end":{"line":3,"character":0}}"#;
    let p1 = make_plan(&mut h);
    assert_eq!(
        err_code(&h.prepare(&p1, &st(moved), &ok)),
        "STALE_SELECTION"
    );
    let p2 = make_plan(&mut h);
    let no_selection = format!(r#"{{"uri":{},"version":3,"text":{}}}"#, q(URI), q(text));
    assert_eq!(
        err_code(&h.prepare(&p2, &no_selection, &ok)),
        "STALE_SELECTION"
    );
    let p3 = make_plan(&mut h);
    let token = s(&h.prepare(&p3, &st(sel), &ok), "token").to_string();
    let begin = h.begin(&token, &st(sel));
    let edits = edits_of(&begin);
    assert_eq!(
        (edits[0].start, edits[0].end),
        (off, off),
        "UTF-16 column 6 is right before the emoji"
    );
    let after = apply_edits(text, &edits).unwrap();
    let fin = h.call(&format!(
        r#"{{"op":"edit.finish_apply","token":"{token}","outcome":"accepted","state":{}}}"#,
        state(&after, 4)
    ));
    assert!(is_ok(&fin), "{fin:?}");
    // the validation of an insert is a preview that creates nothing
    let v = h.call(&format!(
        r#"{{"op":"ctx.validate","snapshot_id":"{snap}","language":"rust","edits":[{{"start":0,"end":0,"text":"fn x( {{\n"}}]}}"#
    ));
    assert_eq!(s(v.get("result").unwrap(), "level"), "invalid");
    let v = h.call(&format!(
        r#"{{"op":"ctx.validate","snapshot_id":"{snap}","language":"rust"}}"#
    ));
    assert_eq!(s(v.get("result").unwrap(), "level"), "valid");
}

#[test]
fn plans_refuse_other_files_unwritable_contexts_and_bad_edits() {
    let mut h = Harness::open();
    let snap = h.snapshot(BASE, 7);
    let plan = |h: &mut Harness, extra: &str, edits: &str| {
        h.call(&format!(
            r#"{{"op":"edit.plan","kind":"full","snapshot_id":"{snap}","edits":{edits}{extra}}}"#
        ))
    };
    let one = r#"[{"start":0,"end":2,"text":"FN"}]"#;
    let two_files = r#","targets":["file:///w/a.rs","file:///w/other.rs"]"#;
    assert_eq!(err_code(&plan(&mut h, two_files, one)), "CROSS_FILE_DENIED");
    assert!(is_ok(&plan(
        &mut h,
        r#","targets":["file:///w/a.rs"]"#,
        one
    )));
    assert_eq!(err_code(&plan(&mut h, "", "[]")), "PATCH_INVALID");
    assert_eq!(err_code(&plan(&mut h, "", "5")), "PATCH_INVALID");
    let beyond = r#"[{"start":0,"end":99,"text":"x"}]"#;
    assert_eq!(err_code(&plan(&mut h, "", beyond)), "TARGET_RANGE_INVALID");
    let negative = r#"[{"start":-1,"end":2,"text":"x"}]"#;
    assert_eq!(
        err_code(&plan(&mut h, "", negative)),
        "TARGET_RANGE_INVALID"
    );
    let overlap = r#"[{"start":0,"end":5,"text":"a"},{"start":3,"end":7,"text":"b"}]"#;
    assert_eq!(err_code(&plan(&mut h, "", overlap)), "PATCH_INVALID");
    assert_eq!(
        err_code(&plan(&mut h, "", r#"[{"start":0,"end":1}]"#)),
        "PATCH_INVALID"
    );
    let r = h.call(&format!(
        r#"{{"op":"edit.plan","kind":"rewrite","snapshot_id":"{snap}","edits":{one}}}"#
    ));
    assert_eq!(err_code(&r), "PATCH_INVALID");
    // an L4 (disk) or L1 snapshot can never be whole-file written
    for level in ["L4", "L1"] {
        let r = h.call(&format!(
            r#"{{"op":"ctx.snapshot","uri":{},"version":1,"text":"x\n","level":"{level}"}}"#,
            q(URI)
        ));
        let id = s(r.get("snapshot").unwrap(), "snapshot_id").to_string();
        let r = h.call(&format!(
            r#"{{"op":"edit.plan","kind":"full","snapshot_id":"{id}","edits":[{{"start":0,"end":1,"text":"y"}}]}}"#
        ));
        assert_eq!(err_code(&r), "FULL_CONTEXT_REQUIRED", "{level}");
    }
    // a missing text is CONTEXT_UNAVAILABLE
    let r = h.call(&format!(
        r#"{{"op":"ctx.snapshot","uri":{},"version":1}}"#,
        q(URI)
    ));
    assert_eq!(err_code(&r), "CONTEXT_UNAVAILABLE");
    // a selection of no hunk is no edit at all; the snapshot of a plan must be the proposal's
    let proposal = s(&h.propose(&snap, PROPOSED), "proposal").to_string();
    let r = h.call(&format!(
        r#"{{"op":"edit.plan","kind":"full","proposal":"{proposal}","hunk_ids":["zz"]}}"#
    ));
    assert_eq!(err_code(&r), "PATCH_INVALID");
    let r = h.call(&format!(
        r#"{{"op":"edit.plan","kind":"full","proposal":"{proposal}","hunk_ids":["h1"],"snapshot_id":"other"}}"#
    ));
    assert_eq!(err_code(&r), "INVALID_REQUEST");
    let r = h.call(r#"{"op":"edit.select","proposal":"nope","hunk_ids":[]}"#);
    assert_eq!(err_code(&r), "TOKEN_INVALID");
}

#[test]
fn releasing_a_snapshot_forgets_its_text_and_proposals_and_the_store_is_bounded() {
    let mut h = Harness::open();
    let snap = h.snapshot(BASE, 7);
    let proposal = s(&h.propose(&snap, PROPOSED), "proposal").to_string();
    let release = format!(r#"{{"op":"ctx.release","snapshot_id":"{snap}"}}"#);
    assert_eq!(h.call(&release).get("released"), Some(&Json::Bool(true)));
    assert_eq!(h.call(&release).get("released"), Some(&Json::Bool(false)));
    let select = format!(r#"{{"op":"edit.select","proposal":"{proposal}","hunk_ids":["h1"]}}"#);
    assert_eq!(err_code(&h.call(&select)), "TOKEN_INVALID");
    let excerpt = format!(r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":0}}"#);
    assert_eq!(err_code(&h.call(&excerpt)), "CONTEXT_UNAVAILABLE");
    let propose = format!(r#"{{"op":"edit.propose","snapshot_id":"{snap}","proposed_text":"x"}}"#);
    assert_eq!(err_code(&h.call(&propose)), "CONTEXT_UNAVAILABLE");
    // the oldest snapshot falls out when more than eight are kept
    let first = h.snapshot("first\n", 1);
    for i in 0..8 {
        h.snapshot("more\n", 2 + i);
    }
    let excerpt = format!(r#"{{"op":"ctx.excerpt","snapshot_id":"{first}","cursor":0}}"#);
    assert_eq!(err_code(&h.call(&excerpt)), "CONTEXT_UNAVAILABLE");
}

#[test]
fn excerpt_applies_the_policy_and_never_splits_characters() {
    let mut h = Harness::open();
    let text = format!("{}\u{1F600}{}", "x".repeat(300), "y".repeat(300));
    let snap = h.snapshot(&text, 1);
    let r = h.call(&format!(
        r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":300,"before":80,"after":80}}"#
    ));
    let ex = r.get("excerpt").unwrap();
    assert_eq!((int(ex, "start"), int(ex, "end")), (Some(220), Some(380)));
    let wide = h.call(&format!(
        r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":300,"before":200,"after":80,"policy":{{"before":80,"after":80}}}}"#
    ));
    assert_eq!(err_code(&wide), "GRANT_DENIED");
    // a cursor in the middle of the emoji moves the bounds to character boundaries
    let r = h.call(&format!(
        r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":302,"before":1,"after":1}}"#
    ));
    let ex = r.get("excerpt").unwrap();
    assert_eq!((int(ex, "start"), int(ex, "end")), (Some(302), Some(303)));
    assert_eq!(s(ex, "text"), "y");
    let bad = h.call(&format!(r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}"}}"#));
    assert_eq!(
        (err_code(&bad), s(&bad, "field")),
        ("INVALID_REQUEST", "cursor")
    );
}

#[test]
fn text_survives_unicode_and_control_characters() {
    let mut h = Harness::open();
    let text = "a\u{0}b\u{1}\r\n\u{1F600}\t\"quoted\" \\ \u{2028}\n";
    let snap = h.snapshot(text, 1);
    let r = h.call(&format!(
        r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":0,"before":0,"after":100}}"#
    ));
    assert_eq!(s(r.get("excerpt").unwrap(), "text"), text);
}

#[test]
fn a_one_megabyte_document_goes_through_the_whole_edit_flow() {
    let mut h = Harness::open();
    let line = "let value = compute(items, \u{4e2d}\u{6587}, \u{1F600});\n";
    let units_per_line = line.encode_utf16().count();
    let n = 1_048_576 / units_per_line;
    let mut lines: Vec<String> = (0..n).map(|_| line.to_string()).collect();
    let base: String = lines.concat();
    lines[3] = "let changed = 1;\n".to_string();
    lines[5] = "let also_changed = 2;\r\n".to_string();
    let proposed: String = lines.concat();
    assert!(base.len() > 1_000_000);

    let snapshot_line = format!(
        r#"{{"op":"ctx.snapshot","uri":{},"version":9,"text":{},"selection":{SEL0},"id":1,"session":"{}"}}"#,
        q(URI),
        q(&base),
        h.session
    );
    assert!(
        snapshot_line.len() > 64 * 1024,
        "a bulk line over the 0.2 limit"
    );
    let big = parse(&h.core.handle_line(&snapshot_line), big_limits()).unwrap();
    assert!(
        is_ok(&big),
        "a 1 MiB snapshot is accepted: {}",
        s(&big, "error")
    );
    let snap = s(big.get("snapshot").unwrap(), "snapshot_id").to_string();
    assert_eq!(s(big.get("snapshot").unwrap(), "sha256"), sha256_hex(&base));
    let prop = h.propose(&snap, &proposed);
    let hunks = prop.get("hunks").and_then(Json::as_array).unwrap();
    assert_eq!(hunks.len(), 2);
    // hunk geometry is in UTF-16 units: the first changed line starts after three full lines
    assert_eq!(
        int(&hunks[0], "startOffset"),
        Some((3 * units_per_line) as i64)
    );
    let proposal = s(&prop, "proposal").to_string();
    let grant = h.authorize("L3", false);
    let plan = h.plan_full(&proposal, r#"["h1","h2"]"#);
    let ok = format!(r#","confirmed":true,"grant_id":"{grant}""#);
    let st = format!(r#"{{"uri":{},"version":9,"text":{}}}"#, q(URI), q(&base));
    let token = s(&h.prepare(&plan, &st, &ok), "token").to_string();
    let begin = h.begin(&token, &st);
    assert!(is_ok(&begin), "{}", s(&begin, "error"));
    let after = apply_edits(&base, &edits_of(&begin)).unwrap();
    assert_eq!(after, proposed);
    let fin = h.call(&format!(
        r#"{{"op":"edit.finish_apply","token":"{token}","outcome":"accepted","state":{{"uri":{},"version":10,"text":{}}}}}"#,
        q(URI),
        q(&after)
    ));
    assert!(is_ok(&fin), "{}", s(&fin, "error"));
    assert_eq!(s(&fin, "newHash"), sha256_hex(&proposed));
    // the whole result text can be fetched for the Diff view (a reply over 1 MiB is allowed)
    let text = h.call(&format!(
        r#"{{"op":"edit.result","proposal":"{proposal}","hunk_ids":["h1","h2"]}}"#
    ));
    assert_eq!(s(&text, "text"), proposed);
    // the same size through a 0.2 operation is still refused
    let reply = h.core.handle_line(&format!(
        r#"{{"op":"validate","language":"rust","text":{}}}"#,
        q(&base)
    ));
    assert_eq!(reply, r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
}

#[test]
fn grants_report_what_to_persist_and_a_new_session_can_import_it() {
    let mut h = Harness::open();
    let ws = s(
        &h.call(r#"{"op":"grant.workspace_token","uris":["file:///b","file:///a"]}"#),
        "workspace",
    )
    .to_string();
    assert_eq!(
        ws,
        pime_core::grants::workspace_token(&["file:///a".into(), "file:///b".into()])
    );
    let r = h.call(r#"{"op":"grant.authorize","provider_id":"A:h","workspace_uris":["file:///b","file:///a"],"scope":"L1"}"#);
    let g = r.get("grant").unwrap();
    assert_eq!(s(g, "workspace"), ws);
    assert_eq!(
        (g.get("once"), g.get("revoked")),
        (Some(&Json::Bool(false)), Some(&Json::Bool(false)))
    );
    assert_eq!(
        int(g, "expiresAt").unwrap() - int(g, "createdAt").unwrap(),
        24 * 3600 * 1000
    );
    assert_eq!(
        r.get("persist").and_then(Json::as_array).map(Vec::len),
        Some(1)
    );
    let id = s(g, "id").to_string();
    assert_eq!(id.len(), 24);
    // a one-request grant is never persisted
    let r = h.call(&format!(
        r#"{{"op":"grant.authorize","provider_id":"A:h","workspace":"{ws}","scope":"L3","once":true,"ttl_ms":600000}}"#
    ));
    assert_eq!(
        r.get("persist").and_then(Json::as_array).map(Vec::len),
        Some(1)
    );
    // find / check / require follow the JS broker
    let crit = |scope: &str| format!(r#""provider_id":"A:h","workspace":"{ws}","scope":"{scope}""#);
    let find = h.call(&format!(r#"{{"op":"grant.find",{}}}"#, crit("L1")));
    assert_eq!(s(find.get("grant").unwrap(), "id"), id);
    let none = h.call(&format!(r#"{{"op":"grant.find",{}}}"#, crit("L3")));
    assert_eq!(
        none.get("grant"),
        Some(&Json::Null),
        "one-request grants are never found for reuse"
    );
    let chk = h.call(&format!(
        r#"{{"op":"grant.check","grant_id":"{id}","provider_id":"B:h","workspace":"{ws}","scope":"L1"}}"#
    ));
    assert_eq!(s(chk.get("result").unwrap(), "reason"), "provider-mismatch");
    let req = h.call(&format!(
        r#"{{"op":"grant.require","grant_id":"{id}",{}}}"#,
        crit("L3")
    ));
    assert_eq!(err_code(&req), "GRANT_DENIED");
    assert!(s(&req, "message").contains("scope-mismatch"));
    let list = h.call(r#"{"op":"grant.list"}"#);
    assert_eq!(
        list.get("grants").and_then(Json::as_array).map(Vec::len),
        Some(2)
    );
    let exported = h.call(r#"{"op":"grant.export"}"#);
    let grants = exported.get("grants").unwrap().clone();
    assert_eq!(grants.as_array().map(Vec::len), Some(1));

    // another session (another run of the extension) imports what was persisted
    let other = h.open_another(77);
    h.session = other;
    let imported = h.call(&format!(
        r#"{{"op":"grant.import","grants":{}}}"#,
        grants.to_json_string()
    ));
    assert_eq!(int(&imported, "imported"), Some(1));
    let find = h.call(&format!(r#"{{"op":"grant.find",{}}}"#, crit("L1")));
    assert_eq!(s(find.get("grant").unwrap(), "id"), id);
    // revocation reports the new persisted set; key deletion revokes by prefix
    let r = h.call(r#"{"op":"grant.revoke_provider_prefix","prefix":"A:"}"#);
    assert_eq!(int(&r, "count"), Some(1));
    assert_eq!(
        r.get("persist").and_then(Json::as_array).map(Vec::len),
        Some(0)
    );
    assert_eq!(
        int(&h.call(r#"{"op":"grant.revoke_all"}"#), "count"),
        Some(0)
    );
    assert_eq!(
        h.call(r#"{"op":"grant.list"}"#)
            .get("grants")
            .and_then(Json::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"grant.import","grants":5}"#)),
        "INVALID_REQUEST"
    );
    let bad_scope =
        h.call(r#"{"op":"grant.authorize","provider_id":"A","workspace":"w","scope":"L9"}"#);
    assert_eq!(err_code(&bad_scope), "GRANT_DENIED");
    let no_ws = h.call(r#"{"op":"grant.authorize","provider_id":"A","scope":"L1"}"#);
    assert_eq!(err_code(&no_ws), "INVALID_REQUEST");
    // grants expire with the injected clock
    let r = h.call(r#"{"op":"grant.authorize","provider_id":"A:h","workspace":"w","scope":"L1","ttl_ms":1000}"#);
    let id2 = s(r.get("grant").unwrap(), "id").to_string();
    let valid = format!(r#"{{"op":"grant.still_valid","grant_id":"{id2}"}}"#);
    assert_eq!(h.call(&valid).get("valid"), Some(&Json::Bool(true)));
    h.clock.advance(1000);
    assert_eq!(h.call(&valid).get("valid"), Some(&Json::Bool(false)));
}

#[test]
fn budget_accounting_over_the_protocol() {
    let mut h = Harness::open();
    let r = h.call(
        r#"{"op":"budget.configure","max_requests":2,"max_tokens":0,"price_per_1k_tokens":2}"#,
    );
    assert!(is_ok(&r));
    let t1 = s(
        &h.call(r#"{"op":"budget.reserve","estimate":80}"#),
        "ticket",
    )
    .to_string();
    let t2 = s(
        &h.call(r#"{"op":"budget.reserve","estimate":10}"#),
        "ticket",
    )
    .to_string();
    assert_eq!(
        err_code(&h.call(r#"{"op":"budget.reserve","estimate":1}"#)),
        "AI_BUDGET_EXCEEDED"
    );
    let settle = h.call(&format!(
        r#"{{"op":"budget.settle","ticket":"{t1}","usage":{{"input":20,"output":10}}}}"#
    ));
    assert_eq!(settle.get("settled"), Some(&Json::Bool(true)));
    let again = h.call(&format!(
        r#"{{"op":"budget.settle","ticket":"{t1}","usage":{{"input":999,"output":1}}}}"#
    ));
    assert_eq!(
        again.get("settled"),
        Some(&Json::Bool(false)),
        "settling twice changes nothing"
    );
    h.call(&format!(r#"{{"op":"budget.release","ticket":"{t2}"}}"#));
    let snap = h.call(r#"{"op":"budget.snapshot"}"#);
    assert_eq!(
        snap.get("budget").unwrap().to_json_string(),
        r#"{"requests":1,"tokens":30,"estimated":false,"cost":0.06}"#
    );
    let t3 = s(&h.call(r#"{"op":"budget.reserve","estimate":5}"#), "ticket").to_string();
    h.call(&format!(r#"{{"op":"budget.settle","ticket":"{t3}"}}"#));
    let snap = h.call(r#"{"op":"budget.snapshot"}"#);
    assert_eq!(
        snap.get("budget").unwrap().get("estimated"),
        Some(&Json::Bool(true))
    );
    assert!(is_ok(&h.call(r#"{"op":"budget.reset"}"#)));
    let snap = h.call(r#"{"op":"budget.snapshot"}"#);
    assert_eq!(
        snap.get("budget").unwrap().to_json_string(),
        r#"{"requests":0,"tokens":0,"estimated":false,"cost":0}"#
    );
    assert_eq!(
        err_code(&h.call(r#"{"op":"budget.reserve","estimate":-1}"#)),
        "INVALID_REQUEST"
    );
    let bad = h.call(r#"{"op":"budget.settle","ticket":"t","usage":{"input":1}}"#);
    assert_eq!(err_code(&bad), "INVALID_REQUEST");
}

#[test]
fn every_new_operation_needs_a_session() {
    let mut h = Harness::open();
    let ops = [
        "ctx.snapshot",
        "ctx.excerpt",
        "ctx.validate",
        "ctx.release",
        "edit.propose",
        "edit.select",
        "edit.result",
        "edit.plan",
        "edit.prepare",
        "edit.begin_apply",
        "edit.finish_apply",
        "edit.invalidate",
        "edit.discard",
        "edit.purge",
        "edit.status",
        "grant.workspace_token",
        "grant.authorize",
        "grant.check",
        "grant.find",
        "grant.require",
        "grant.consume",
        "grant.revoke",
        "grant.revoke_provider",
        "grant.revoke_provider_prefix",
        "grant.revoke_all",
        "grant.still_valid",
        "grant.list",
        "grant.export",
        "grant.import",
        "budget.configure",
        "budget.reserve",
        "budget.settle",
        "budget.release",
        "budget.snapshot",
        "budget.reset",
    ];
    assert_eq!(
        ops.len() + 4,
        pime_core::ops::OPS.len(),
        "the test lists every operation except session.open/close and settings.apply/resolve"
    );
    for op in ops {
        let reply = h.core.handle_line(&format!(r#"{{"op":"{op}","id":1}}"#));
        assert!(reply.contains(r#""error":"NO_SESSION""#), "{op}: {reply}");
        let unknown = format!(r#"{{"op":"{op}","id":1,"session":"{}"}}"#, "0".repeat(32));
        let reply = h.core.handle_line(&unknown);
        assert!(reply.contains(r#""error":"NO_SESSION""#), "{op}: {reply}");
    }
    // a closed session answers NO_SESSION for the edit path as well
    let snap = h.snapshot(BASE, 1);
    let release = format!(
        r#"{{"op":"ctx.release","snapshot_id":"{snap}","session":"{}"}}"#,
        h.session
    );
    h.call(r#"{"op":"session.close"}"#);
    assert!(h.core.handle_line(&release).contains("NO_SESSION"));
}

#[test]
fn ids_of_every_family_are_hex_and_do_not_repeat_across_sessions() {
    let mut a = Harness::open();
    let second = a.open_another(3);
    let first = a.session.clone();
    let mut seen = std::collections::HashSet::new();
    for session in [first, second] {
        a.session = session;
        let snap = a.snapshot(BASE, 1);
        let proposal = s(&a.propose(&snap, PROPOSED), "proposal").to_string();
        let grant = a.authorize("L3", false);
        let plan = a.plan_full(&proposal, r#"["h1"]"#);
        let ok = format!(r#","confirmed":true,"grant_id":"{grant}""#);
        let token = s(&a.prepare(&plan, &state(BASE, 1), &ok), "token").to_string();
        for id in [a.session.clone(), snap, proposal, grant, plan, token] {
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
            assert!(seen.insert(id.clone()), "duplicate id {id}");
        }
    }
}

#[test]
fn sessions_do_not_see_each_others_snapshots_or_grants() {
    let mut h = Harness::open();
    let first = h.session.clone();
    let snap = h.snapshot(BASE, 1);
    let grant = h.authorize("L3", false);
    h.session = h.open_another(5);
    let excerpt = format!(r#"{{"op":"ctx.excerpt","snapshot_id":"{snap}","cursor":0}}"#);
    assert_eq!(err_code(&h.call(&excerpt)), "CONTEXT_UNAVAILABLE");
    let valid = format!(r#"{{"op":"grant.still_valid","grant_id":"{grant}"}}"#);
    assert_eq!(h.call(&valid).get("valid"), Some(&Json::Bool(false)));
    h.session = first;
    assert!(is_ok(&h.call(&excerpt)));
    assert_eq!(h.call(&valid).get("valid"), Some(&Json::Bool(true)));
}

// ---------------------------------------------------------------- sample request file

/// Runs the sample lines, replacing `$SESSION`-style placeholders with ids learned from earlier answers.
fn run_samples(samples: &[String], mut send: impl FnMut(&str) -> String) -> Vec<Json> {
    let mut known: Vec<(&str, String)> = Vec::new();
    let mut out = Vec::new();
    for line in samples {
        let mut line = line.clone();
        for (name, value) in &known {
            line = line.replace(name, value);
        }
        assert!(!line.contains('$'), "unresolved placeholder in {line}");
        let reply = send(&line);
        let j = parse(&reply, big_limits()).unwrap_or_else(|e| panic!("bad reply {e}: {reply}"));
        let mut learn = |name: &'static str, v: Option<&str>| {
            if let Some(v) = v {
                known.retain(|(n, _)| *n != name);
                known.push((name, v.to_string()));
            }
        };
        learn("$SESSION", j.get("session").and_then(Json::as_str));
        learn(
            "$SNAPSHOT",
            j.get("snapshot")
                .and_then(|x| x.get("snapshot_id"))
                .and_then(Json::as_str),
        );
        learn("$PROPOSAL", j.get("proposal").and_then(Json::as_str));
        learn(
            "$GRANT",
            j.get("grant")
                .and_then(|x| x.get("id"))
                .and_then(Json::as_str),
        );
        learn(
            "$PLAN",
            j.get("plan")
                .and_then(|x| x.get("id"))
                .and_then(Json::as_str),
        );
        learn("$TOKEN", j.get("token").and_then(Json::as_str));
        out.push(j);
    }
    out
}

fn check_samples(r: &[Json]) {
    assert_eq!(r.len(), 22);
    assert_eq!((s(&r[0], "protocol"), s(&r[1], "protocol")), ("0.3", "0.3"));
    assert_eq!(s(r[3].get("scenario").unwrap(), "providerId"), "mockA");
    assert_eq!(s(r[4].get("snapshot").unwrap(), "eol"), "LF");
    assert_eq!(s(r[5].get("excerpt").unwrap(), "text"), "\nfn b() {}");
    assert_eq!(s(r[6].get("result").unwrap(), "level"), "valid");
    assert_eq!(
        r[7].get("hunks").and_then(Json::as_array).map(Vec::len),
        Some(2)
    );
    assert_eq!(
        s(r[8].get("result").unwrap(), "hash"),
        sha256_hex("fn a() { 1 }\n\nfn b() {}\n\nfn c() {}\n")
    );
    assert_eq!(s(r[9].get("grant").unwrap(), "scope"), "L3");
    assert_eq!(s(r[10].get("plan").unwrap(), "status"), "REVIEW_READY");
    assert_eq!(err_code(&r[11]), "USER_CONFIRMATION_REQUIRED");
    assert_eq!(s(&r[12], "token").len(), 32);
    assert_eq!(
        r[13].get("edits").and_then(Json::as_array).map(Vec::len),
        Some(1)
    );
    assert_eq!(s(&r[14], "state"), "APPLIED");
    assert_eq!(err_code(&r[15]), "TOKEN_INVALID");
    assert!(is_ok(&r[16]));
    assert_eq!(s(&r[17], "ticket").len(), 16);
    assert_eq!(int(r[18].get("budget").unwrap(), "requests"), Some(1));
    assert_eq!(
        r[19].get("grants").and_then(Json::as_array).map(Vec::len),
        Some(1)
    );
    assert_eq!(int(&r[20], "count"), Some(1));
    assert_eq!(err_code(&r[21]), "NO_SESSION");
    // ids are echoed
    for (i, resp) in r.iter().enumerate() {
        assert_eq!(
            int(resp, "id"),
            Some(i as i64 + 1),
            "id echo on line {}",
            i + 1
        );
    }
}

fn samples() -> Vec<String> {
    let path = format!(
        "{}/../../tests/fixtures/core-jsonl-v0.3.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .expect("sample file")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_v0_3_sample_requests_via_handle_line() {
    let mut core = Core::new();
    check_samples(&run_samples(&samples(), |line| core.handle_line(line)));
}

#[test]
fn the_v0_3_sample_requests_via_the_real_binary() {
    use std::io::BufRead;
    let mut child = Command::new(env!("CARGO_BIN_EXE_pime-core"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pime-core");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut reader = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let answers = run_samples(&samples(), |line| {
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut out = String::new();
        reader.read_line(&mut out).unwrap();
        out.trim_end().to_string()
    });
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "the core writes nothing to stderr");
    check_samples(&answers);
}

#[test]
fn the_v0_3_schema_describes_every_operation_and_error() {
    let path = format!(
        "{}/../../schemas/core-protocol-v0.3.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(path).expect("schema file");
    let schema = parse(&raw, big_limits()).expect("schema is valid JSON");
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
    ]
    .iter()
    .chain(pime_core::ops::OPS.iter())
    {
        assert!(
            raw.contains(&format!("\"const\": \"{op}\"")),
            "schema lacks request op {op}"
        );
    }
    for err in [
        "INVALID_JSON",
        "PAYLOAD_TOO_LARGE",
        "UNKNOWN_OPERATION",
        "NO_SESSION",
        "INVALID_REQUEST",
    ] {
        assert!(raw.contains(err), "schema lacks error {err}");
    }
}
