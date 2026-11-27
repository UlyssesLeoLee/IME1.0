//! The structured assertions of a test suite (detailed design §10.1).
//!
//! The IDE1.0 acceptance file `.aci.json` has its required fields: `assertion_id`, `aci_version`, `layer`, `scope`,
//! `expect`, `actual`, `status`, `severity`, `reasoning`, `captured_at`. The states of an assertion are the states of
//! the contract (`PASS`, `FAIL`, `SKIPPED`, `NOT_RUN`).
//!
//! This module holds the **summary** of a suite: how many assertions passed, how many failed, how many were skipped
//! and how many did not run, and whether the suite may be called green.
//!
//! # The two kinds of assertion
//!
//! [`Assertion`] is the state of a check: a name, a state and a message. It stays that small on purpose, because it
//! is what a caller that only ran a check knows.
//!
//! [`AssertionRecord`] is the entry of the acceptance file: the assertion **and** the evidence it carries — the
//! expectation, the value that was measured, the severity and the moment of the capture. A report holds records, so
//! the file carries the ten fields of detailed design §10.1 and not a summary of them.
//!
//! # What this module does not invent
//!
//! The module never writes a value it did not measure. A field that the caller did not state is written `null`, and
//! a suite never says `PASS` for a check that did not run. The module never reads the clock either: the moment of the
//! capture is given by the caller ([`AssertionEvidence::captured_at_ms`]).
//!
//! # The layer
//!
//! Detailed design §10.1 fixes the four layers `ut`, `it`, `st` and `e2e`, and says that `uat` is a term of the UI:
//! it falls into `e2e` when it enters the acceptance file. [`TestLayer`] is that enumeration and nothing more: a term
//! that is not one of the four is `None`, and the field stays `null` in the file.

use pime_core::jsval::iso_date;
use pime_core::json::Json;
use tase_contract::api::CheckStatus;

use crate::METRICS_VERSION;

/// The version of the acceptance file that this module writes.
///
/// Detailed design §10.1 fixes the ten fields of the file but not the version string: the value is the version of the
/// crate that owns the schema ([`METRICS_VERSION`]). A caller that writes a file of another version states it with
/// [`AciReport::with_aci_version`].
pub const ACI_VERSION: &str = METRICS_VERSION;

/// The layer of a test suite (detailed design §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TestLayer {
    /// The unit tests.
    Ut,
    /// The integration tests.
    It,
    /// The system tests.
    St,
    /// The end-to-end tests.
    E2e,
}

impl TestLayer {
    /// Every layer of the acceptance file.
    pub const ALL: [TestLayer; 4] = [
        TestLayer::Ut,
        TestLayer::It,
        TestLayer::St,
        TestLayer::E2e,
    ];

    /// The name of the layer in the acceptance file.
    pub fn as_str(self) -> &'static str {
        match self {
            TestLayer::Ut => "ut",
            TestLayer::It => "it",
            TestLayer::St => "st",
            TestLayer::E2e => "e2e",
        }
    }

    /// The layer of a test term.
    ///
    /// `uat` is a term of the UI and falls into `e2e` (detailed design §10.1). A term that is not one of the four
    /// layers is `None`: the module never writes a layer that the design book does not name.
    pub fn from_term(term: &str) -> Option<TestLayer> {
        match term {
            "ut" => Some(TestLayer::Ut),
            "it" => Some(TestLayer::It),
            "st" => Some(TestLayer::St),
            "e2e" | "uat" => Some(TestLayer::E2e),
            _ => None,
        }
    }
}

/// One assertion of a suite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assertion {
    /// The name of the assertion; it is the `assertion_id` of the acceptance file.
    pub name: String,
    /// The state of the assertion.
    pub status: CheckStatus,
    /// The message of the assertion, when there is one.
    pub message: Option<String>,
}

impl Assertion {
    /// An assertion that passed.
    pub fn passed(name: &str) -> Self {
        Assertion {
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: None,
        }
    }

    /// An assertion that failed, with the reason.
    pub fn failed(name: &str, message: &str) -> Self {
        Assertion {
            name: name.to_string(),
            status: CheckStatus::Fail,
            message: Some(message.to_string()),
        }
    }

    /// An assertion that was skipped.
    pub fn skipped(name: &str) -> Self {
        Assertion {
            name: name.to_string(),
            status: CheckStatus::Skipped,
            message: None,
        }
    }

    /// An assertion that did not run.
    pub fn not_run(name: &str) -> Self {
        Assertion {
            name: name.to_string(),
            status: CheckStatus::NotRun,
            message: None,
        }
    }

    /// The entry of the acceptance file that carries this assertion.
    pub fn into_record(self) -> AssertionRecord {
        AssertionRecord::new(self)
    }
}

/// The evidence of an assertion: the fields of the acceptance file that the assertion itself carries.
///
/// Detailed design §10.1 names the fields but does not fix their format: the expectation, the measured value and the
/// severity are the words of the caller, and the moment of the capture is the moment the caller saw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssertionEvidence {
    /// The expectation of the assertion (`expect`).
    pub expect: Option<String>,
    /// The value that was measured (`actual`).
    pub actual: Option<String>,
    /// The severity of the assertion (`severity`).
    pub severity: Option<String>,
    /// The moment the assertion was captured (`captured_at`).
    pub captured_at: Option<String>,
}

impl AssertionEvidence {
    /// The evidence of an assertion that measured nothing.
    pub fn new() -> Self {
        AssertionEvidence {
            expect: None,
            actual: None,
            severity: None,
            captured_at: None,
        }
    }

    /// The evidence with the expectation of the assertion.
    pub fn with_expect(mut self, expect: &str) -> Self {
        self.expect = Some(expect.to_string());
        self
    }

    /// The evidence with the value that was measured.
    pub fn with_actual(mut self, actual: &str) -> Self {
        self.actual = Some(actual.to_string());
        self
    }

    /// The evidence with the severity of the assertion.
    pub fn with_severity(mut self, severity: &str) -> Self {
        self.severity = Some(severity.to_string());
        self
    }

    /// The evidence with the moment of the capture, as the caller states it.
    pub fn with_captured_at(mut self, captured_at: &str) -> Self {
        self.captured_at = Some(captured_at.to_string());
        self
    }

    /// The evidence with the moment of the capture, in milliseconds since the Unix epoch.
    ///
    /// The moment is written in the ISO 8601 UTC form of `pime-core` (`new Date(ms).toISOString()`). A moment that
    /// cannot be rendered stays `None`: the file never holds a wrong time.
    pub fn captured_at_ms(mut self, at_ms: u64) -> Self {
        self.captured_at = iso_date(at_ms as f64);
        self
    }
}

/// One entry of the acceptance file: an assertion and the evidence it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertionRecord {
    /// The assertion itself: its name, its state and its message.
    pub assertion: Assertion,
    /// The evidence of the assertion.
    pub evidence: AssertionEvidence,
}

impl AssertionRecord {
    /// The entry of an assertion that measured nothing yet.
    pub fn new(assertion: Assertion) -> Self {
        AssertionRecord {
            assertion,
            evidence: AssertionEvidence::new(),
        }
    }

    /// The entry with the evidence of the assertion.
    pub fn with_evidence(mut self, evidence: AssertionEvidence) -> Self {
        self.evidence = evidence;
        self
    }

    /// The entry with the expectation of the assertion.
    pub fn with_expect(mut self, expect: &str) -> Self {
        self.evidence.expect = Some(expect.to_string());
        self
    }

    /// The entry with the value that was measured.
    pub fn with_actual(mut self, actual: &str) -> Self {
        self.evidence.actual = Some(actual.to_string());
        self
    }

    /// The entry with the severity of the assertion.
    pub fn with_severity(mut self, severity: &str) -> Self {
        self.evidence.severity = Some(severity.to_string());
        self
    }

    /// The entry with the moment of the capture, as the caller states it.
    pub fn with_captured_at(mut self, captured_at: &str) -> Self {
        self.evidence.captured_at = Some(captured_at.to_string());
        self
    }

    /// The entry with the moment of the capture, in milliseconds since the Unix epoch.
    ///
    /// See [`AssertionEvidence::captured_at_ms`].
    pub fn captured_at_ms(mut self, at_ms: u64) -> Self {
        self.evidence = self.evidence.captured_at_ms(at_ms);
        self
    }
}

impl From<Assertion> for AssertionRecord {
    fn from(assertion: Assertion) -> Self {
        AssertionRecord::new(assertion)
    }
}

/// The report of a suite of assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AciReport {
    /// The name of the suite.
    pub suite: String,
    /// The entries of the acceptance file: the assertions of the suite and the evidence they carry.
    pub assertions: Vec<AssertionRecord>,
    /// The version of the acceptance file.
    pub aci_version: String,
    /// The layer of the suite; `None` when the caller did not state it.
    pub layer: Option<TestLayer>,
    /// The scope the assertions of the suite belong to; `None` when the caller did not state it.
    pub scope: Option<String>,
}

impl AciReport {
    /// A report of a suite of assertions.
    ///
    /// The assertions carry no evidence yet: a caller that only ran the checks does not have to fill the fields of the
    /// acceptance file, and the file writes them `null` rather than guessing them.
    pub fn new(suite: &str, assertions: Vec<Assertion>) -> Self {
        AciReport {
            suite: suite.to_string(),
            assertions: assertions.into_iter().map(AssertionRecord::new).collect(),
            aci_version: ACI_VERSION.to_string(),
            layer: None,
            scope: None,
        }
    }

    /// A report of a suite of assertions with the evidence they carry.
    pub fn with_records(suite: &str, records: Vec<AssertionRecord>) -> Self {
        AciReport {
            suite: suite.to_string(),
            assertions: records,
            aci_version: ACI_VERSION.to_string(),
            layer: None,
            scope: None,
        }
    }

    /// The report with the version of the acceptance file.
    pub fn with_aci_version(mut self, version: &str) -> Self {
        self.aci_version = version.to_string();
        self
    }

    /// The report of a layer.
    pub fn with_layer(mut self, layer: TestLayer) -> Self {
        self.layer = Some(layer);
        self
    }

    /// The report of a scope.
    pub fn with_scope(mut self, scope: &str) -> Self {
        self.scope = Some(scope.to_string());
        self
    }

    /// The summary of the report.
    pub fn summary(&self) -> AciSummary {
        summarise(self)
    }
}

/// The summary of a suite of assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AciSummary {
    /// The name of the suite.
    pub suite: String,
    /// How many assertions passed.
    pub passed: usize,
    /// How many assertions failed.
    pub failed: usize,
    /// How many assertions were skipped.
    pub skipped: usize,
    /// How many assertions did not run.
    pub not_run: usize,
    /// How many assertions the suite holds.
    pub total: usize,
    /// Whether the suite may be called green.
    ///
    /// A suite is green when no assertion failed **and** no assertion did not run: a check that did not run is not a
    /// passed check (detailed design §10.3: "不能伪报 PASS"). A skipped assertion does not make the suite red: it was
    /// skipped on purpose.
    pub ok: bool,
}

/// The summary of a report.
///
/// See [`AciSummary::ok`] for what makes a suite green.
pub fn summarise(report: &AciReport) -> AciSummary {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut not_run = 0usize;

    for record in &report.assertions {
        match record.assertion.status {
            CheckStatus::Pass => passed += 1,
            CheckStatus::Fail => failed += 1,
            CheckStatus::Skipped => skipped += 1,
            CheckStatus::NotRun => not_run += 1,
        }
    }

    AciSummary {
        suite: report.suite.clone(),
        passed,
        failed,
        skipped,
        not_run,
        total: report.assertions.len(),
        ok: failed == 0 && not_run == 0,
    }
}

/// The report as the JSON of the acceptance file.
///
/// The keys of an assertion are the ten keys of detailed design §10.1. The ones the caller did not state are written
/// `null`: the module never writes a value it did not measure.
pub fn to_json(report: &AciReport) -> String {
    let summary = summarise(report);
    let assertions: Vec<Json> = report
        .assertions
        .iter()
        .map(|record| {
            Json::obj(vec![
                ("assertion_id", Json::str(&record.assertion.name)),
                ("aci_version", Json::str(&report.aci_version)),
                ("layer", layer_json(report.layer)),
                ("scope", text_json(&report.scope)),
                ("expect", text_json(&record.evidence.expect)),
                ("actual", text_json(&record.evidence.actual)),
                ("status", Json::str(record.assertion.status.as_str())),
                ("severity", text_json(&record.evidence.severity)),
                ("reasoning", text_json(&record.assertion.message)),
                ("captured_at", text_json(&record.evidence.captured_at)),
            ])
        })
        .collect();

    Json::obj(vec![
        ("suite", Json::str(&report.suite)),
        ("assertions", Json::Arr(assertions)),
        (
            "summary",
            Json::obj(vec![
                ("passed", Json::int(summary.passed as i64)),
                ("failed", Json::int(summary.failed as i64)),
                ("skipped", Json::int(summary.skipped as i64)),
                ("not_run", Json::int(summary.not_run as i64)),
                ("total", Json::int(summary.total as i64)),
                ("ok", Json::Bool(summary.ok)),
            ]),
        ),
    ])
    .to_json_string()
}

/// The layer of the file: the name of the layer, or `null` when there is none.
fn layer_json(layer: Option<TestLayer>) -> Json {
    match layer {
        Some(layer) => Json::str(layer.as_str()),
        None => Json::Null,
    }
}

/// The value of the caller, or `null` when there is none.
fn text_json(value: &Option<String>) -> Json {
    match value {
        Some(value) => Json::str(value),
        None => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json::{Limits, parse};

    /// A suite with one assertion of every state.
    fn full_report() -> AciReport {
        AciReport::new(
            "ut_planner",
            vec![
                Assertion::passed("plan_is_valid"),
                Assertion::failed("digest_is_stable", "the digest changed"),
                Assertion::skipped("network_is_off"),
                Assertion::not_run("template_is_approved"),
            ],
        )
    }

    /// A suite whose assertions carry the evidence of the acceptance file.
    fn measured_report() -> AciReport {
        AciReport::with_records(
            "ut_planner",
            vec![
                Assertion::passed("plan_is_valid")
                    .into_record()
                    .with_expect("the plan holds every step")
                    .with_actual("the plan holds every step")
                    .with_severity("high")
                    .captured_at_ms(1_700_000_000_000),
                Assertion::failed("digest_is_stable", "the digest changed")
                    .into_record()
                    .with_expect("the digest does not change")
                    .with_actual("the digest changed")
                    .with_severity("high")
                    .captured_at_ms(1_700_000_000_001),
            ],
        )
        .with_layer(TestLayer::Ut)
        .with_scope("tase-planner::plan_digest")
    }

    #[test]
    fn the_summary_counts_every_state() {
        let summary = summarise(&full_report());
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.not_run, 1);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.suite, "ut_planner");
    }

    #[test]
    fn a_suite_with_a_failure_is_not_green() {
        let summary = summarise(&full_report());
        assert!(!summary.ok);
    }

    #[test]
    fn a_suite_that_did_not_run_is_not_green_either() {
        let report = AciReport::new(
            "ut_planner",
            vec![Assertion::passed("a"), Assertion::not_run("b")],
        );
        let summary = summarise(&report);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.not_run, 1);
        assert!(
            !summary.ok,
            "a check that did not run is not a passed check"
        );
    }

    #[test]
    fn a_suite_that_passed_is_green() {
        let report = AciReport::new(
            "ut_planner",
            vec![Assertion::passed("a"), Assertion::passed("b")],
        );
        assert!(summarise(&report).ok);
    }

    #[test]
    fn a_skipped_assertion_does_not_make_the_suite_red() {
        let report = AciReport::new(
            "ut_planner",
            vec![Assertion::passed("a"), Assertion::skipped("b")],
        );
        let summary = summarise(&report);
        assert_eq!(summary.skipped, 1);
        assert!(summary.ok);
    }

    #[test]
    fn an_empty_suite_is_green_and_holds_nothing() {
        let summary = summarise(&AciReport::new("empty", Vec::new()));
        assert_eq!(summary.total, 0);
        assert_eq!(summary.passed, 0);
        assert!(summary.ok);
    }

    #[test]
    fn the_report_holds_the_ten_keys_of_the_acceptance_file() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("the report must be valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(assertions.len(), 4);
        let first = &assertions[0];
        for key in [
            "assertion_id",
            "aci_version",
            "layer",
            "scope",
            "expect",
            "actual",
            "status",
            "severity",
            "reasoning",
            "captured_at",
        ] {
            assert!(first.get(key).is_some(), "the key {key} is missing");
        }
    }

    #[test]
    fn the_status_of_an_assertion_is_the_status_of_the_contract() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(
            assertions[0].get("status").and_then(|s| s.as_str()),
            Some("PASS")
        );
        assert_eq!(
            assertions[1].get("status").and_then(|s| s.as_str()),
            Some("FAIL")
        );
        assert_eq!(
            assertions[2].get("status").and_then(|s| s.as_str()),
            Some("SKIPPED")
        );
        assert_eq!(
            assertions[3].get("status").and_then(|s| s.as_str()),
            Some("NOT_RUN")
        );
    }

    #[test]
    fn the_fields_that_were_not_measured_are_null_and_never_invented() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        let first = &assertions[0];
        for key in ["layer", "scope", "expect", "actual", "severity", "captured_at"] {
            assert!(
                first.get(key).expect(key).is_null(),
                "the key {key} must be null"
            );
        }
        assert!(first.get("reasoning").expect("the reasoning").is_null());
    }

    #[test]
    fn the_version_of_the_acceptance_file_is_never_null() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(
            assertions[0].get("aci_version").and_then(|v| v.as_str()),
            Some(ACI_VERSION)
        );
    }

    #[test]
    fn a_caller_may_state_the_version_of_the_acceptance_file() {
        let report = full_report().with_aci_version("1.0");
        let json = to_json(&report);
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(
            assertions[0].get("aci_version").and_then(|v| v.as_str()),
            Some("1.0")
        );
    }

    #[test]
    fn the_measured_fields_of_the_design_book_are_not_null() {
        let json = to_json(&measured_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        let first = &assertions[0];
        assert_eq!(
            first.get("expect").and_then(|v| v.as_str()),
            Some("the plan holds every step")
        );
        assert_eq!(
            first.get("actual").and_then(|v| v.as_str()),
            Some("the plan holds every step")
        );
        assert_eq!(
            first.get("severity").and_then(|v| v.as_str()),
            Some("high")
        );
        assert_eq!(
            first.get("captured_at").and_then(|v| v.as_str()),
            Some("2023-11-14T22:13:20.000Z")
        );
        assert_eq!(
            first.get("layer").and_then(|v| v.as_str()),
            Some("ut")
        );
        assert_eq!(
            first.get("scope").and_then(|v| v.as_str()),
            Some("tase-planner::plan_digest")
        );
        assert_eq!(
            first.get("aci_version").and_then(|v| v.as_str()),
            Some(ACI_VERSION)
        );
    }

    #[test]
    fn the_ten_keys_are_still_there_when_the_assertions_carry_their_evidence() {
        let json = to_json(&measured_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(assertions.len(), 2);
        for assertion in assertions {
            for key in [
                "assertion_id",
                "aci_version",
                "layer",
                "scope",
                "expect",
                "actual",
                "status",
                "severity",
                "reasoning",
                "captured_at",
            ] {
                assert!(assertion.get(key).is_some(), "the key {key} is missing");
            }
        }
    }

    #[test]
    fn the_layers_of_the_design_book_are_the_only_ones() {
        assert_eq!(TestLayer::Ut.as_str(), "ut");
        assert_eq!(TestLayer::It.as_str(), "it");
        assert_eq!(TestLayer::St.as_str(), "st");
        assert_eq!(TestLayer::E2e.as_str(), "e2e");
        assert_eq!(TestLayer::from_term("ut"), Some(TestLayer::Ut));
        assert_eq!(TestLayer::from_term("it"), Some(TestLayer::It));
        assert_eq!(TestLayer::from_term("st"), Some(TestLayer::St));
        assert_eq!(TestLayer::from_term("e2e"), Some(TestLayer::E2e));
    }

    #[test]
    fn the_term_uat_falls_into_e2e() {
        assert_eq!(TestLayer::from_term("uat"), Some(TestLayer::E2e));
    }

    #[test]
    fn a_term_that_is_not_a_layer_is_never_written() {
        assert_eq!(TestLayer::from_term("acceptance"), None);
        assert_eq!(TestLayer::from_term(""), None);
    }

    #[test]
    fn the_moment_of_the_capture_is_the_moment_the_caller_gave() {
        let evidence = AssertionEvidence::new().captured_at_ms(1_700_000_000_000);
        assert_eq!(
            evidence.captured_at.as_deref(),
            Some("2023-11-14T22:13:20.000Z")
        );
    }

    #[test]
    fn a_moment_that_cannot_be_rendered_stays_null() {
        let evidence = AssertionEvidence::new().captured_at_ms(u64::MAX);
        assert!(evidence.captured_at.is_none());
    }

    #[test]
    fn the_message_of_a_failed_assertion_is_the_reasoning_of_the_acceptance_file() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(
            assertions[1].get("reasoning").and_then(|r| r.as_str()),
            Some("the digest changed")
        );
        assert_eq!(
            assertions[0].get("assertion_id").and_then(|r| r.as_str()),
            Some("plan_is_valid")
        );
    }

    #[test]
    fn the_json_of_the_report_holds_the_summary() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let summary = value.get("summary").expect("the summary");
        assert_eq!(summary.get("passed").and_then(|n| n.as_i64()), Some(1));
        assert_eq!(summary.get("failed").and_then(|n| n.as_i64()), Some(1));
        assert_eq!(summary.get("total").and_then(|n| n.as_i64()), Some(4));
        assert_eq!(summary.get("ok").and_then(|b| b.as_bool()), Some(false));
    }

    #[test]
    fn the_summary_of_a_report_and_the_summary_of_the_report_agree() {
        let report = full_report();
        assert_eq!(report.summary(), summarise(&report));
    }

    #[test]
    fn the_summary_of_a_report_with_evidence_counts_the_same_assertions() {
        let report = measured_report();
        let summary = summarise(&report);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.total, 2);
        assert!(!summary.ok);
    }

    #[test]
    fn a_report_built_from_plain_assertions_still_holds_the_ten_keys() {
        // The way the worker builds a report: the assertions alone, without any evidence.
        let report = AciReport::new(
            "tase-worker",
            vec![Assertion::passed("cargo-check"), Assertion::not_run("cargo-test")],
        );
        let json = to_json(&report);
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        assert_eq!(assertions.len(), 2);
        for key in [
            "assertion_id",
            "aci_version",
            "layer",
            "scope",
            "expect",
            "actual",
            "status",
            "severity",
            "reasoning",
            "captured_at",
        ] {
            assert!(assertions[0].get(key).is_some(), "the key {key} is missing");
        }
        assert_eq!(
            assertions[0].get("status").and_then(|s| s.as_str()),
            Some("PASS")
        );
        assert_eq!(
            assertions[1].get("status").and_then(|s| s.as_str()),
            Some("NOT_RUN")
        );
    }
}
