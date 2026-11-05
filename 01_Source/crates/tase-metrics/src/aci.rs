//! The structured assertions of a test suite (detailed design §10.1).
//!
//! The IDE1.0 acceptance file `.aci.json` has its required fields: `assertion_id`, `aci_version`, `layer`, `scope`,
//! `expect`, `actual`, `status`, `severity`, `reasoning`, `captured_at`. The states of an assertion are the states of
//! the contract (`PASS`, `FAIL`, `SKIPPED`, `NOT_RUN`).
//!
//! This module holds the **summary** of a suite: how many assertions passed, how many failed, how many were skipped
//! and how many did not run, and whether the suite may be called green.
//!
//! # What this module does not invent
//!
//! The module knows the name of an assertion, its state and, when there is one, a message. It does not know the
//! expectation, the actual value, the severity nor the time of the capture: those belong to the layer that owns the
//! evidence (the test itself). When [`to_json`] writes an assertion, the fields it does not know are written `null` —
//! it never writes a value it did not measure, and a suite never says `PASS` for a check that did not run.

use pime_core::json::Json;
use tase_contract::api::CheckStatus;

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
}

/// The report of a suite of assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AciReport {
    /// The name of the suite.
    pub suite: String,
    /// The assertions of the suite.
    pub assertions: Vec<Assertion>,
}

impl AciReport {
    /// A report of a suite.
    pub fn new(suite: &str, assertions: Vec<Assertion>) -> Self {
        AciReport {
            suite: suite.to_string(),
            assertions,
        }
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

    for assertion in &report.assertions {
        match assertion.status {
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
/// The keys of an assertion are the ten keys of detailed design §10.1. The ones this module does not know
/// (`aci_version`, `layer`, `scope`, `expect`, `actual`, `severity`, `captured_at`) are written `null`: the module
/// never writes a value it did not measure.
pub fn to_json(report: &AciReport) -> String {
    let summary = summarise(report);
    let assertions: Vec<Json> = report
        .assertions
        .iter()
        .map(|assertion| {
            Json::obj(vec![
                ("assertion_id", Json::str(&assertion.name)),
                ("aci_version", Json::Null),
                ("layer", Json::Null),
                ("scope", Json::Null),
                ("expect", Json::Null),
                ("actual", Json::Null),
                ("status", Json::str(assertion.status.as_str())),
                ("severity", Json::Null),
                (
                    "reasoning",
                    match &assertion.message {
                        Some(message) => Json::str(message),
                        None => Json::Null,
                    },
                ),
                ("captured_at", Json::Null),
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
    fn the_fields_that_are_not_measured_are_null_and_never_invented() {
        let json = to_json(&full_report());
        let value = parse(&json, Limits::DEFAULT).expect("valid JSON");
        let assertions = value
            .get("assertions")
            .and_then(|a| a.as_array())
            .expect("the assertions");
        let first = &assertions[0];
        for key in [
            "aci_version",
            "layer",
            "scope",
            "expect",
            "actual",
            "severity",
            "captured_at",
        ] {
            assert!(
                first.get(key).expect(key).is_null(),
                "the key {key} must be null"
            );
        }
        assert!(first.get("reasoning").expect("the reasoning").is_null());
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
}
