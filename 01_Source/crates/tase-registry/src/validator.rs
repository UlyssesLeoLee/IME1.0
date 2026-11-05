//! The mismatch rules of an instance (design §3.4).
//!
//! The validator checks a request to instantiate an asset against the asset itself. The rules of design §3.4
//! are:
//!
//! * a version that does not hit, a schema that is not valid, a symbol that exists, a permission that goes
//!   too far: **refuse and give an actionable error**;
//! * a template that is revoked: the job that has not run stops and says so;
//! * a template whose score is too low may be proposed again as a plan - it may never silently fall back to
//!   another template.
//!
//! The validator of the registry turns those rules into eight mismatches. It checks them in this order and
//! the **first** one is the answer of [`Validator::validate_instance`]; [`Validator::validate_instance_all`]
//! returns all of them, in the same order:
//!
//! | # | the check | the mismatch |
//! |---|---|---|
//! | 1 | the asset has an approved implementation of the requested language | [`Mismatch::MissingImplementation`] |
//! | 2 | the license of the asset is on the list of the allowed licenses | [`Mismatch::LicenseNotAllowed`] |
//! | 3 | the pinned digest is the digest of the asset | [`Mismatch::DigestMismatch`] |
//! | 4 | the pinned version is the version of the asset | [`Mismatch::VersionMismatch`] |
//! | 5 | every parameter is a name the asset knows | [`Mismatch::UnknownParameter`] |
//! | 6 | every required parameter was given | [`Mismatch::MissingParameter`] |
//! | 7 | every value has the type the asset declares | [`Mismatch::TypeMismatch`] |
//! | 8 | no constraint of the caller contradicts the asset | [`Mismatch::ConstraintViolation`] |
//!
//! The vocabulary of the validator:
//!
//! * `params` are the parameters of the instance. The names the asset knows are the names of its
//!   applicability constraints, the parameters the validator was told are required and the reserved name
//!   `@version`;
//! * `constraints` are the conditions of the caller (the runtime context). A condition the asset does not
//!   declare is not an error - the asset may not care about it - but a condition the asset does declare must
//!   hold: the same type and the same value.
//!
//! The reserved name `@version` carries the version of the asset the caller believes it is using. A template
//! parameter may not be named `@version`, so the name cannot collide with a real parameter.

use std::fmt;

use crate::digest;
use crate::manifest::AssetEntry;
use crate::policy::Policy;

/// The reserved parameter that carries the version of the asset the caller means.
pub const RESERVED_VERSION: &str = "@version";

/// The ways an instance does not match its asset (design §3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mismatch {
    /// A parameter that the instance needs was not given.
    MissingParameter,
    /// A parameter that the asset does not know was given.
    UnknownParameter,
    /// A value has another type than the asset declares.
    TypeMismatch,
    /// A condition of the caller contradicts the asset.
    ConstraintViolation,
    /// The asset has no approved implementation of the requested language.
    MissingImplementation,
    /// The license of the asset is not on the list of the allowed licenses.
    LicenseNotAllowed,
    /// The pinned digest is not the digest of the asset.
    DigestMismatch,
    /// The pinned version is not the version of the asset.
    VersionMismatch,
}

impl Mismatch {
    /// The mismatch on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Mismatch::MissingParameter => "MISSING_PARAMETER",
            Mismatch::UnknownParameter => "UNKNOWN_PARAMETER",
            Mismatch::TypeMismatch => "TYPE_MISMATCH",
            Mismatch::ConstraintViolation => "CONSTRAINT_VIOLATION",
            Mismatch::MissingImplementation => "MISSING_IMPLEMENTATION",
            Mismatch::LicenseNotAllowed => "LICENSE_NOT_ALLOWED",
            Mismatch::DigestMismatch => "DIGEST_MISMATCH",
            Mismatch::VersionMismatch => "VERSION_MISMATCH",
        }
    }

    /// The error of the contract that the mismatch becomes.
    ///
    /// A missing implementation and a license that is not allowed are refusals of the policy; the other
    /// mismatches are a validation of the request.
    pub fn to_error(self) -> tase_contract::TaseError {
        match self {
            Mismatch::MissingImplementation => tase_contract::TaseError::policy_rejected(
                "the asset has no approved implementation of the requested language",
            ),
            Mismatch::LicenseNotAllowed => tase_contract::TaseError::policy_rejected(
                "the license of the asset is not on the list of the allowed licenses",
            ),
            other => tase_contract::TaseError::validation_failed(format!(
                "the instance does not match the asset: {}",
                other.as_str()
            )),
        }
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The type of a value that is written as a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueKind {
    Bool,
    Number,
    Text,
}

/// The type of a value.
fn value_kind(value: &str) -> ValueKind {
    let text = value.trim();
    if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") {
        return ValueKind::Bool;
    }
    if text.parse::<f64>().is_ok_and(|number| number.is_finite()) {
        return ValueKind::Number;
    }
    ValueKind::Text
}

/// Whether the value of the caller has another type than the asset declares.
fn type_mismatch(declared: &str, given: &str) -> bool {
    match value_kind(declared) {
        ValueKind::Bool => value_kind(given) != ValueKind::Bool,
        ValueKind::Number => value_kind(given) != ValueKind::Number,
        // a text is a text: the design book has no other type to compare it with
        ValueKind::Text => false,
    }
}

/// Whether the value of the caller contradicts the value the asset declares.
fn constraint_violation(declared: &str, given: &str) -> bool {
    // a value of another type is a question of type, not a contradiction
    if value_kind(declared) != ValueKind::Text || value_kind(given) != ValueKind::Text {
        return false;
    }
    !declared.trim().eq_ignore_ascii_case(given.trim())
}

/// The validator of an instance against its asset.
#[derive(Clone, Debug, Default)]
pub struct Validator {
    policy: Policy,
    allowed_licenses: Vec<String>,
    required_parameters: Vec<String>,
}

impl Validator {
    /// A validator with the default policy (no unapproved asset is allowed).
    pub fn new(policy: Policy) -> Self {
        Validator {
            policy,
            allowed_licenses: Vec::new(),
            required_parameters: Vec::new(),
        }
    }

    /// The licenses the caller allows. With an empty list no license is allowed.
    pub fn with_allowed_licenses(mut self, licenses: Vec<String>) -> Self {
        self.allowed_licenses = licenses;
        self
    }

    /// The parameters the caller must give. The registry does not know them: the executor that owns the
    /// schema of the parameters tells the validator.
    pub fn with_required_parameters(mut self, names: Vec<String>) -> Self {
        self.required_parameters = names;
        self
    }

    /// The policy of the validator.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The licenses the validator allows.
    pub fn allowed_licenses(&self) -> &[String] {
        &self.allowed_licenses
    }

    /// Whether the digest is the digest of the asset.
    pub fn validate_digest(&self, entry: &AssetEntry, digest: &str) -> Result<(), Mismatch> {
        validate_digest(entry, digest)
    }

    /// Checks an instance against its asset and returns the first mismatch.
    pub fn validate_instance(
        &self,
        entry: &AssetEntry,
        language: &str,
        params: &[(String, String)],
        constraints: &[(String, String)],
        digest: Option<&str>,
    ) -> Result<(), Mismatch> {
        match self
            .validate_instance_all(entry, language, params, constraints, digest)
            .into_iter()
            .next()
        {
            Some(mismatch) => Err(mismatch),
            None => Ok(()),
        }
    }

    /// Checks an instance against its asset and returns every mismatch, in the order of the rules.
    pub fn validate_instance_all(
        &self,
        entry: &AssetEntry,
        language: &str,
        params: &[(String, String)],
        constraints: &[(String, String)],
        digest: Option<&str>,
    ) -> Vec<Mismatch> {
        let mut out = Vec::new();

        // 1. the implementation gate: a knowledge card is not an executable template
        if self.policy.may_instantiate(entry, language).is_err() {
            out.push(Mismatch::MissingImplementation);
        }

        // 2. the license gate
        if !self
            .policy
            .license_allows(entry, &self.allowed_licenses)
        {
            out.push(Mismatch::LicenseNotAllowed);
        }

        // 3. the pinned digest
        if let Some(digest) = digest
            && validate_digest(entry, digest).is_err()
        {
            out.push(Mismatch::DigestMismatch);
        }

        // 4. the pinned version
        if let Some((_, value)) = params.iter().find(|(name, _)| name == RESERVED_VERSION)
            && value.trim() != entry.version
        {
            out.push(Mismatch::VersionMismatch);
        }

        // 5. the parameters the asset does not know
        for (name, _) in params {
            if name == RESERVED_VERSION {
                continue;
            }
            if !self.knows(entry, name) {
                out.push(Mismatch::UnknownParameter);
            }
        }

        // 6. the required parameters that were not given
        for required in &self.required_parameters {
            if !params.iter().any(|(name, _)| name == required) {
                out.push(Mismatch::MissingParameter);
            }
        }

        // 7. the types and 8. the contradictions
        for (name, value) in constraints {
            let Some((_, declared)) = entry.constraints.iter().find(|(key, _)| key == name) else {
                // the asset does not care about this condition
                continue;
            };
            if type_mismatch(declared, value) {
                out.push(Mismatch::TypeMismatch);
            }
            if constraint_violation(declared, value) {
                out.push(Mismatch::ConstraintViolation);
            }
        }

        out
    }

    /// Whether the asset knows this name.
    fn knows(&self, entry: &AssetEntry, name: &str) -> bool {
        entry.constraints.iter().any(|(key, _)| key == name)
            || self.required_parameters.iter().any(|key| key == name)
    }
}

/// Whether the digest is the digest of the asset.
///
/// The digest of the asset and the digest of the caller may carry the `sha256:` prefix or not; the 64 hex
/// digits are what is compared. An asset without a digest never matches.
pub fn validate_digest(entry: &AssetEntry, digest: &str) -> Result<(), Mismatch> {
    match (digest::hex_of(&entry.digest), digest::hex_of(digest)) {
        (Some(expected), Some(given)) if expected.eq_ignore_ascii_case(given) => Ok(()),
        _ => Err(Mismatch::DigestMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::asset_digest;
    use crate::manifest::{AssetEntry, ImplementationEntry};
    use crate::policy::Policy;

    /// An asset with an approved Rust implementation.
    fn asset() -> AssetEntry {
        AssetEntry {
            id: "algorithm.graph.kruskal".to_string(),
            version: "1.0.0".to_string(),
            kind: "algorithm".to_string(),
            category: "graph".to_string(),
            names: Default::default(),
            aliases: Vec::new(),
            status: "REVIEWED".to_string(),
            intent: "做什么。".to_string(),
            constraints: vec![
                ("graph".to_string(), "undirected_connected".to_string()),
                ("negative_edges".to_string(), "false".to_string()),
            ],
            counterexamples: Vec::new(),
            alternatives: Vec::new(),
            complexity_time: None,
            complexity_space: None,
            implementations: vec![ImplementationEntry {
                implementation_id: "algorithm.graph.kruskal.rust".to_string(),
                language: "rust".to_string(),
                version: "1.0.0".to_string(),
                status: "APPROVED".to_string(),
                engine: "tase-template".to_string(),
                path: "templates/rust/graph/kruskal".to_string(),
                sha256: "a".repeat(64),
                parameters_schema: "schemas/kruskal-params.json".to_string(),
                tests_manifest: "tests/kruskal.yaml".to_string(),
            }],
            license: Some("MIT".to_string()),
            references: Vec::new(),
            digest: asset_digest("algorithm.graph.kruskal", "1.0.0", b"the body"),
            card_path: None,
        }
    }

    /// A validator that allows the license of the asset.
    fn validator() -> Validator {
        Validator::new(Policy::default()).with_allowed_licenses(vec!["MIT".to_string()])
    }

    #[test]
    fn an_instance_that_matches_its_asset_is_accepted() {
        let entry = asset();
        let validator = validator();
        let digest = entry.digest.clone();
        assert!(validator
            .validate_instance(&entry, "rust", &[], &[], Some(&digest))
            .is_ok());
    }

    #[test]
    fn an_asset_without_an_approved_implementation_has_no_implementation() {
        let mut entry = asset();
        entry.implementations.clear();
        let error = validator()
            .validate_instance(&entry, "rust", &[], &[], None)
            .expect_err("there is no implementation");
        assert_eq!(error, Mismatch::MissingImplementation);
        assert_eq!(error.to_error().code, tase_contract::ErrorCode::PolicyRejected);
    }

    #[test]
    fn an_implementation_of_another_language_has_no_implementation() {
        let entry = asset();
        let error = validator()
            .validate_instance(&entry, "typescript", &[], &[], None)
            .expect_err("there is no TypeScript implementation");
        assert_eq!(error, Mismatch::MissingImplementation);
    }

    #[test]
    fn a_license_that_is_not_allowed_is_refused() {
        let entry = asset();
        let validator = Validator::new(Policy::default());
        let error = validator
            .validate_instance(&entry, "rust", &[], &[], None)
            .expect_err("the license is not allowed");
        assert_eq!(error, Mismatch::LicenseNotAllowed);
        assert_eq!(error.to_error().code, tase_contract::ErrorCode::PolicyRejected);
    }

    #[test]
    fn a_digest_that_is_not_the_digest_of_the_asset_is_refused() {
        let entry = asset();
        let wrong = asset_digest("algorithm.graph.kruskal", "1.0.0", b"another body");
        let error = validator()
            .validate_instance(&entry, "rust", &[], &[], Some(&wrong))
            .expect_err("the digest is not the digest of the asset");
        assert_eq!(error, Mismatch::DigestMismatch);
        // the bare hex part is the same digest
        let hex = digest::hex_of(&entry.digest).unwrap();
        assert!(validator()
            .validate_instance(&entry, "rust", &[], &[], Some(hex))
            .is_ok());
    }

    #[test]
    fn a_version_that_is_not_the_version_of_the_asset_is_refused() {
        let entry = asset();
        let params = vec![(RESERVED_VERSION.to_string(), "0.9.0".to_string())];
        let error = validator()
            .validate_instance(&entry, "rust", &params, &[], None)
            .expect_err("the version does not hit");
        assert_eq!(error, Mismatch::VersionMismatch);
        // the version that hits is accepted
        let params = vec![(RESERVED_VERSION.to_string(), "1.0.0".to_string())];
        assert!(validator()
            .validate_instance(&entry, "rust", &params, &[], None)
            .is_ok());
    }

    #[test]
    fn a_parameter_that_the_asset_does_not_know_is_refused() {
        let entry = asset();
        let params = vec![("nothing".to_string(), "value".to_string())];
        let error = validator()
            .validate_instance(&entry, "rust", &params, &[], None)
            .expect_err("the asset does not know the parameter");
        assert_eq!(error, Mismatch::UnknownParameter);
        // a name the asset declares is known
        let params = vec![("graph".to_string(), "undirected_connected".to_string())];
        assert!(validator()
            .validate_instance(&entry, "rust", &params, &[], None)
            .is_ok());
    }

    #[test]
    fn a_required_parameter_that_was_not_given_is_refused() {
        let entry = asset();
        let validator = validator().with_required_parameters(vec!["entity".to_string()]);
        let error = validator
            .validate_instance(&entry, "rust", &[], &[], None)
            .expect_err("the parameter is missing");
        assert_eq!(error, Mismatch::MissingParameter);
        // the parameter that was given is accepted
        let params = vec![("entity".to_string(), "User".to_string())];
        assert!(validator
            .validate_instance(&entry, "rust", &params, &[], None)
            .is_ok());
    }

    #[test]
    fn a_value_of_another_type_is_refused() {
        let entry = asset();
        // the asset declares that there are no negative edges: a boolean
        let constraints = vec![("negative_edges".to_string(), "yes".to_string())];
        let error = validator()
            .validate_instance(&entry, "rust", &[], &constraints, None)
            .expect_err("the value is not a boolean");
        assert_eq!(error, Mismatch::TypeMismatch);
        // the boolean that the asset declares is accepted
        let constraints = vec![("negative_edges".to_string(), "false".to_string())];
        assert!(validator()
            .validate_instance(&entry, "rust", &[], &constraints, None)
            .is_ok());
    }

    #[test]
    fn a_condition_that_contradicts_the_asset_is_refused() {
        let entry = asset();
        let constraints = vec![("graph".to_string(), "directed".to_string())];
        let error = validator()
            .validate_instance(&entry, "rust", &[], &constraints, None)
            .expect_err("the condition contradicts the asset");
        assert_eq!(error, Mismatch::ConstraintViolation);
        // the condition the asset declares is accepted
        let constraints = vec![("graph".to_string(), "undirected_connected".to_string())];
        assert!(validator()
            .validate_instance(&entry, "rust", &[], &constraints, None)
            .is_ok());
    }

    #[test]
    fn a_condition_that_the_asset_does_not_declare_is_not_an_error() {
        let entry = asset();
        let constraints = vec![("memory_budget_mb".to_string(), "128".to_string())];
        assert!(validator()
            .validate_instance(&entry, "rust", &[], &constraints, None)
            .is_ok());
    }

    #[test]
    fn every_mismatch_is_reported_in_the_order_of_the_rules() {
        let mut entry = asset();
        entry.implementations.clear();
        let params = vec![
            (RESERVED_VERSION.to_string(), "0.9.0".to_string()),
            ("nothing".to_string(), "value".to_string()),
        ];
        let constraints = vec![
            ("negative_edges".to_string(), "yes".to_string()),
            ("graph".to_string(), "directed".to_string()),
        ];
        let validator = Validator::new(Policy::default())
            .with_allowed_licenses(vec!["MIT".to_string()])
            .with_required_parameters(vec!["entity".to_string()]);
        let all = validator.validate_instance_all(
            &entry,
            "rust",
            &params,
            &constraints,
            Some("sha256:0000000000000000000000000000000000000000000000000000000000000000"),
        );
        assert_eq!(
            all,
            vec![
                Mismatch::MissingImplementation,
                Mismatch::DigestMismatch,
                Mismatch::VersionMismatch,
                Mismatch::UnknownParameter,
                Mismatch::MissingParameter,
                Mismatch::TypeMismatch,
                Mismatch::ConstraintViolation,
            ]
        );
        // the first one is the answer of validate_instance
        assert_eq!(
            validator
                .validate_instance(&entry, "rust", &params, &constraints, None)
                .expect_err("the instance does not match"),
            Mismatch::MissingImplementation
        );
    }

    #[test]
    fn a_mismatch_travels_by_name_and_becomes_an_error_of_the_contract() {
        let mismatches = [
            Mismatch::MissingParameter,
            Mismatch::UnknownParameter,
            Mismatch::TypeMismatch,
            Mismatch::ConstraintViolation,
            Mismatch::MissingImplementation,
            Mismatch::LicenseNotAllowed,
            Mismatch::DigestMismatch,
            Mismatch::VersionMismatch,
        ];
        for mismatch in mismatches {
            assert_eq!(mismatch.to_string(), mismatch.as_str());
            assert_eq!(mismatch.to_error().code, match mismatch {
                Mismatch::MissingImplementation | Mismatch::LicenseNotAllowed => {
                    tase_contract::ErrorCode::PolicyRejected
                }
                _ => tase_contract::ErrorCode::ValidationFailed,
            });
        }
    }
}
