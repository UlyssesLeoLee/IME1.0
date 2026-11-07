//! Parity of the candidate ranking, the context classification and the privacy policy with the JavaScript mirror
//! of the input method core (01_Source/editor-bridges/vscode/src/core/candidates.js).
//!
//! `tests/fixtures/tase-ranking-cases.json` is read by BOTH sides: this test runs the crate `tase-ime-core` (the
//! only truth source of the order, of the score parts and of the refusal reasons) and
//! tests/js/ut-tase-candidates.test.js runs the mirror. The fixture holds the input of every case and the expected
//! result; the expected results are printed by this test (TASE_PARITY_EMIT=1), so the golden file is refreshed
//! from the truth source and never by hand.
//!
//! The test lives in one home with the other parity tests (01_Source/tests/rust) and is registered in the crate of
//! the truth source:
//!
//! ```toml
//! [[test]]
//! name = "parity_tase_ime"
//! path = "../../tests/rust/parity_tase_ime.rs"
//! ```

use pime_core::json::{Json, Limits, parse};
use tase_ime_core::candidate::Candidate;
use tase_ime_core::context::{ContextKind, InputContext};
use tase_ime_core::policy::{Decision, PrivacyPolicy};
use tase_ime_core::rank::{RankRequest, rank};

/// The tolerance of the comparison: the ranking itself compares the scores rounded to 1e-9.
const EPSILON: f64 = 1e-9;

/// The path of a fixture: the convention of the other parity tests is `<crate>/../../tests/fixtures/<name>`. The
/// environment variable wins over it (a crate that lives outside the repository names the directory), the search
/// upwards is the fallback.
fn fixture_path(name: &str) -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("TASE_PARITY_FIXTURES") {
        return std::path::PathBuf::from(dir).join(name);
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let conventional = manifest.join("../../tests/fixtures").join(name);
    if conventional.exists() {
        return conventional;
    }
    let mut dir = manifest.to_path_buf();
    for _ in 0..8 {
        let candidate = dir.join("tests").join("fixtures").join(name);
        if candidate.exists() {
            return candidate;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    conventional
}

fn load_fixture() -> Json {
    let path = fixture_path("tase-ranking-cases.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
    parse(
        &raw,
        Limits {
            max_bytes: 4 << 20,
            max_depth: 32,
        },
    )
    .expect("the fixture is valid JSON")
}

fn s<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or("")
}

fn list<'a>(doc: &'a Json, key: &str) -> &'a Vec<Json> {
    doc.get(key)
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("the fixture lacks {key}"))
}

/// The context of a case: the kind, the language hint, the generation and the excerpt.
fn context_of(value: &Json) -> InputContext {
    let kind = ContextKind::parse(s(value, "kind")).unwrap_or(ContextKind::Unknown);
    let language = value.get("language_hint").and_then(Json::as_str);
    let generation = value.get("focus_context_generation").and_then(Json::as_i64).unwrap_or(0) as u64;
    match value.get("excerpt").and_then(Json::as_str) {
        Some(excerpt) => InputContext::with_excerpt(kind, language, generation, excerpt),
        None => InputContext::safe(kind, language, generation),
    }
}

/// The candidates of a case.
fn candidates_of(value: &Json) -> Vec<Candidate> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| Candidate::from_json(item).expect("the candidate of the fixture"))
                .collect()
        })
        .unwrap_or_default()
}

/// The request of a case: the query, the language and the context.
fn request_of(case: &Json) -> RankRequest {
    let request = case.get("request").cloned().unwrap_or(Json::Null);
    let query = s(&request, "query").to_string();
    let language = request.get("language").and_then(Json::as_str).map(|l| l.to_string());
    let context = context_of(case.get("context").unwrap_or(&Json::Null));
    RankRequest::new(&query, language.as_deref(), context)
}

/// The policy of a case: the two permissions.
fn policy_of(case: &Json) -> PrivacyPolicy {
    let policy = case.get("policy").cloned().unwrap_or(Json::Null);
    PrivacyPolicy::new(
        policy
            .get("allow_excerpt_in_code_context")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        policy
            .get("allow_agent_orchestration")
            .and_then(Json::as_bool)
            .unwrap_or(false),
    )
}

/// A score rounded to the resolution of the comparison (the same rounding the ranking uses).
fn round9(value: f64) -> f64 {
    (value * 1e9).round() / 1e9
}

/// The decision of the policy as the fixture writes it.
fn decision_json(decision: Decision) -> Json {
    let kind = if decision.is_allowed() {
        "allow"
    } else if decision.is_read_only() {
        "allow_read_only"
    } else {
        "deny"
    };
    Json::obj(vec![
        ("kind", Json::str(kind)),
        (
            "reason",
            decision.reason().map_or(Json::Null, Json::str),
        ),
    ])
}

/// What the crate answers for one case: the order, the parts, the decision and the generation checks.
fn expectations(case: &Json) -> Json {
    let request = request_of(case);
    let candidates = candidates_of(case.get("candidates").unwrap_or(&Json::Null));
    let context = context_of(case.get("context").unwrap_or(&Json::Null));
    let policy = policy_of(case);
    let ranked = rank(&candidates, &request);

    let order: Vec<Json> = ranked
        .iter()
        .map(|entry| Json::str(&entry.candidate.candidate_id))
        .collect();
    let scores: Vec<Json> = ranked
        .iter()
        .map(|entry| {
            Json::obj(vec![
                ("candidate_id", Json::str(&entry.candidate.candidate_id)),
                ("total", Json::Num(round9(entry.score.total))),
                (
                    "language_match",
                    Json::Num(round9(entry.score.language_match)),
                ),
                (
                    "keyword_match",
                    Json::Num(round9(entry.score.keyword_match)),
                ),
                ("applicability", Json::Num(round9(entry.score.applicability))),
                ("risk_penalty", Json::Num(round9(entry.score.risk_penalty))),
            ])
        })
        .collect();
    let stale: Vec<Json> = case
        .get("stale_checks")
        .and_then(Json::as_array)
        .map(|checks| {
            checks
                .iter()
                .map(|check| {
                    let current = check.get("current_generation").and_then(Json::as_i64).unwrap_or(0) as u64;
                    Json::obj(vec![
                        ("current_generation", Json::Num(current as f64)),
                        ("is_stale", Json::Bool(context.is_stale(current))),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();

    Json::obj(vec![
        ("order", Json::Arr(order)),
        ("scores", Json::Arr(scores)),
        ("decision", decision_json(policy.decide(&context))),
        (
            "excerpt_allowed",
            Json::Bool(policy.excerpt_allowed(&context)),
        ),
        (
            "may_share_with_agent",
            Json::Bool(policy.may_share_with_agent(&context)),
        ),
        ("stale", Json::Arr(stale)),
    ])
}

/// The comparison of two JSON values: the numbers are compared with the tolerance of the ranking.
fn same(got: &Json, want: &Json) -> bool {
    match (got, want) {
        (Json::Num(a), Json::Num(b)) => (a - b).abs() <= EPSILON,
        (Json::Str(a), Json::Str(b)) => a == b,
        (Json::Bool(a), Json::Bool(b)) => a == b,
        (Json::Null, Json::Null) => true,
        (Json::Arr(a), Json::Arr(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| same(x, y))
        }
        (Json::Obj(a), Json::Obj(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, value)| {
                    b.iter()
                        .find(|(other_key, _)| other_key == key)
                        .is_some_and(|(_, other)| same(value, other))
                })
        }
        _ => false,
    }
}

fn report(name: &str, total: usize, failures: &[String]) {
    println!(
        "parity {name}: {}/{} entries match",
        total - failures.len(),
        total
    );
    assert!(
        failures.is_empty(),
        "{name}: {} mismatches (first 8):\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The order, the score parts, the decision and the generation of every case of the fixture.
#[test]
fn the_ranking_and_the_privacy_policy_match_the_fixture() {
    let fixture = load_fixture();
    let cases = list(&fixture, "cases");
    assert!(
        cases.len() >= 20,
        "the fixture must cover every rule of the design, it has {} cases",
        cases.len()
    );
    let mut failures = Vec::new();
    for case in cases {
        let name = s(case, "name").to_string();
        let got = expectations(case);
        let want = case.get("expect").cloned().unwrap_or(Json::Null);
        if !same(&got, &want) {
            failures.push(format!(
                "{name}:\n  got  {}\n  want {}",
                got.to_json_string(),
                want.to_json_string()
            ));
        }
    }
    report("tase-ranking", cases.len(), &failures);
}

/// A candidate that is broken is refused with the same code and the same message on both sides.
#[test]
fn a_broken_candidate_is_refused_with_the_same_reason() {
    let fixture = load_fixture();
    let broken = list(&fixture, "invalid_candidates");
    assert!(
        broken.len() >= 8,
        "the fixture must cover the broken shapes of a candidate"
    );
    let mut failures = Vec::new();
    for case in broken {
        let candidate = case.get("candidate").cloned().unwrap_or(Json::Null);
        let want = case.get("error").cloned().unwrap_or(Json::Null);
        let got = match Candidate::from_json(&candidate) {
            Ok(_) => Json::obj(vec![
                ("code", Json::str("NO_ERROR")),
                ("message", Json::str("the candidate was accepted")),
            ]),
            Err(error) => Json::obj(vec![
                ("code", Json::str(error.code)),
                ("message", Json::str(&error.message)),
            ]),
        };
        if !same(&got, &want) {
            failures.push(format!(
                "{}: got {}, want {}",
                candidate.to_json_string(),
                got.to_json_string(),
                want.to_json_string()
            ));
        }
    }
    report("tase-invalid-candidates", broken.len(), &failures);
}

/// The expectations of the fixture, printed from the truth source: the golden file is refreshed with them.
/// `TASE_PARITY_EMIT=1 cargo test --test parity_tase_ime -- --nocapture` prints the JSON of the merge script.
#[test]
fn the_expectations_are_printed_for_a_refresh() {
    if std::env::var("TASE_PARITY_EMIT").is_err() {
        return;
    }
    let fixture = load_fixture();
    let cases = list(&fixture, "cases");
    let printed: Vec<Json> = cases
        .iter()
        .map(|case| {
            Json::obj(vec![
                ("name", Json::str(s(case, "name"))),
                ("expect", expectations(case)),
            ])
        })
        .collect();
    println!("TASE_PARITY_EXPECTATIONS {}", Json::Arr(printed).to_json_string());
}
