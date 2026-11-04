//! IME-S01 ... IME-S21: the behaviour of the text service, scenario by scenario.
//!
//! Every scenario drives the text service the way a TSF host drives it and checks what the user must see. The
//! scenarios are the cross-module part of the IME test suite: the unit tests of the crate check one module at a
//! time, these check the input method as a whole (typing, the window, the composition, the host, the failure paths).
//!
//! The evidence (a JSON file per scenario) is written only when `PIME_IME_EVIDENCE_DIR` is set.

mod common;

use common::scenarios::{IME_FUNCTIONS, SCENARIOS};

/// Every scenario of the table runs, and the table is the only place a scenario may live.
#[test]
fn every_scenario_of_the_table_passes() {
    let mut failures = Vec::new();
    for scenario in SCENARIOS {
        // Each scenario is its own test case: one broken scenario must not hide the others.
        let result = std::panic::catch_unwind(|| common::scenarios::run(scenario.id));
        if let Err(panic) = result {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".to_string());
            failures.push(format!("{} ({}): {}", scenario.id, scenario.title, message));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} scenarios failed:\n{}",
        failures.len(),
        SCENARIOS.len(),
        failures.join("\n")
    );
}

/// The ids are unique and stable: the ledger and the evidence files refer to them.
#[test]
fn the_scenario_ids_are_unique() {
    let mut ids: Vec<&str> = SCENARIOS.iter().map(|s| s.id).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), count, "two scenarios share an id: {ids:?}");
    for id in &ids {
        assert!(
            id.starts_with("IME-S"),
            "a scenario id must look like IME-S01: {id}"
        );
    }
}

/// Every scenario belongs to a function of the ledger.
#[test]
fn every_scenario_belongs_to_a_known_function() {
    for scenario in SCENARIOS {
        assert!(
            IME_FUNCTIONS.iter().any(|f| f.id == scenario.function),
            "{} names the unknown function {}",
            scenario.id,
            scenario.function
        );
    }
}
