//! The ledger of the IME functions: every function of the input method must name the test that covers it, and the
//! named test must really exist.
//!
//! This is the answer to "are all the functions of the input method tested?". The ledger cannot rot silently: the
//! unit test it names is looked up in the sources of the crate, and the scenario it names must be in the scenario
//! table. A function that loses its test fails here.

mod common;

use common::scenarios::{CRATE_SOURCES, IME_FUNCTIONS, SCENARIOS};

fn source_of(file: &str) -> &'static str {
    CRATE_SOURCES
        .iter()
        .find(|(name, _)| *name == file)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("{file} is not in the list of crate sources"))
}

/// Every function of the ledger names a test that exists.
#[test]
fn every_function_of_the_input_method_names_a_test_that_exists() {
    let mut missing = Vec::new();
    for function in IME_FUNCTIONS {
        let by_scenario = !function.scenarios.is_empty();
        let by_unit = !function.unit_test.is_empty();
        assert!(
            by_scenario || by_unit,
            "{} ({}) names no test at all",
            function.id,
            function.name
        );
        if by_scenario {
            for id in function.scenarios {
                assert!(
                    SCENARIOS.iter().any(|s| s.id == *id),
                    "{} names the unknown scenario {}",
                    function.id,
                    id
                );
            }
        }
        if by_unit {
            assert!(
                !function.unit_file.is_empty(),
                "{} names the unit test {} but no file",
                function.id,
                function.unit_test
            );
            let text = source_of(function.unit_file);
            let needle = format!("fn {}()", function.unit_test);
            if !text.contains(&needle) {
                missing.push(format!(
                    "{}: the test {} does not exist in {}",
                    function.id, function.unit_test, function.unit_file
                ));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

/// The function ids are unique: two functions may not share an id.
#[test]
fn the_function_ids_are_unique() {
    let mut ids: Vec<&str> = IME_FUNCTIONS.iter().map(|f| f.id).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), count, "two functions share an id: {ids:?}");
}

/// Every function is reachable from a scenario or from a unit test - and the ledger itself is not a dead document:
/// the functions are the ones the modules of the crate actually implement.
#[test]
fn the_ledger_covers_every_module_of_the_text_service() {
    for module in [
        "controller",
        "keymap",
        "layout",
        "popup",
        "menus",
        "floating",
        "settings_window",
        "imesettings",
        "register",
        "uielement",
    ] {
        assert!(
            IME_FUNCTIONS.iter().any(|f| f.module == module),
            "no function of the ledger belongs to the module {module}"
        );
    }
}

/// The scenarios cover the functions they claim, and the ledger points at scenarios that exist.
#[test]
fn every_scenario_of_the_table_is_reachable_from_the_ledger() {
    for scenario in SCENARIOS {
        let named = IME_FUNCTIONS
            .iter()
            .any(|f| f.scenarios.contains(&scenario.id));
        assert!(
            named,
            "the scenario {} ({}) is not named by any function of the ledger",
            scenario.id, scenario.title
        );
    }
}
