//! The versions of the contract.
//!
//! The contract is versioned separately from the crates that implement it: the six TASE crates are released on their
//! own schedule, but a client that speaks `1.0` must keep working against a server that speaks `1.1`. The rule is
//! therefore on the *minor* line: `1.0.x` and `1.1.x` are both accepted, `1.2` is not (and never will be without a
//! new line of the contract).

/// The version of the contract this build speaks.
pub const CONTRACT_VERSION: &str = "1.1.0";

/// The version of the schema of an asset manifest (design §19.1).
pub const ASSET_SCHEMA_VERSION: &str = "tase.asset.v1";

/// The contract lines this build accepts. The `1.0` line is still here because M0 ships the old interface as a
/// compatibility layer.
pub const SUPPORTED_CONTRACT_LINES: &[&str] = &["1.0", "1.1"];

/// Whether a contract version is one this build accepts.
///
/// A version is `major.minor` or `major.minor.patch`; the `1.0` and the `1.1` lines are accepted, everything else
/// (a different major, a different minor, a version that is not a number) is refused.
pub fn is_supported_contract_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return false;
    }
    let Some(major) = parts[0].parse::<u32>().ok() else {
        return false;
    };
    let Some(minor) = parts[1].parse::<u32>().ok() else {
        return false;
    };
    if parts.len() == 3 && parts[2].parse::<u32>().is_err() {
        return false;
    }
    matches!((major, minor), (1, 0) | (1, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_versions_of_this_build_are_the_ones_the_book_names() {
        assert_eq!(CONTRACT_VERSION, "1.1.0");
        assert_eq!(ASSET_SCHEMA_VERSION, "tase.asset.v1");
    }

    #[test]
    fn the_old_line_and_the_new_line_are_both_accepted() {
        for version in ["1.0", "1.0.0", "1.0.7", "1.1", "1.1.0", "1.1.99"] {
            assert!(
                is_supported_contract_version(version),
                "{version} must be accepted"
            );
        }
    }

    #[test]
    fn every_other_version_is_refused() {
        for version in [
            "",
            "1",
            "1.2",
            "1.2.0",
            "2.0.0",
            "0.9.9",
            "1.0.0.0",
            "1.0.x",
            "1.0.0-beta",
            " 1.0.0",
            "1..0",
            "v1.0.0",
        ] {
            assert!(
                !is_supported_contract_version(version),
                "{version:?} must be refused"
            );
        }
    }

    #[test]
    fn the_version_of_this_build_is_accepted() {
        assert!(is_supported_contract_version(CONTRACT_VERSION));
    }

    #[test]
    fn the_supported_lines_are_the_two_the_book_knows() {
        assert_eq!(SUPPORTED_CONTRACT_LINES, ["1.0", "1.1"]);
    }
}
