//! The governance of the assets: the state of the knowledge card, the state of the language implementations and
//! the rules that move an implementation from one state to another (requirements §14.4).
//!
//! # The two axes
//!
//! The design book keeps two axes apart (requirements §14.4):
//!
//! | the axis | the states | the question it answers |
//! |---|---|---|
//! | the knowledge card | [`KnowledgeCardStatus::Documented`], [`KnowledgeCardStatus::Reviewed`] | may the card be read and quoted? |
//! | the language implementation | [`ImplementationStatus::Planned`], [`ImplementationStatus::Implemented`], [`ImplementationStatus::Tested`], [`ImplementationStatus::Approved`], [`ImplementationStatus::Deprecated`], [`ImplementationStatus::Revoked`] | may the asset be **instantiated** in this language? |
//!
//! A card that is `REVIEWED` still has no implementation, and an implementation that is `APPROVED` says nothing
//! about the card. An implementation that is approved for Rust says nothing about TypeScript: the instantiation
//! gate always asks for the implementation **of the requested language**.
//!
//! # The state machine
//!
//! | from | may go to |
//! |---|---|
//! | `PLANNED` | `IMPLEMENTED`, `REVOKED` |
//! | `IMPLEMENTED` | `TESTED`, `DEPRECATED`, `REVOKED` |
//! | `TESTED` | `APPROVED`, `IMPLEMENTED`, `DEPRECATED`, `REVOKED` |
//! | `APPROVED` | `DEPRECATED`, `REVOKED` |
//! | `DEPRECATED` | `APPROVED`, `REVOKED` |
//! | `REVOKED` | nothing: the state is terminal |
//!
//! Two transitions ask for evidence ([`GovernancePolicy`]):
//!
//! * `IMPLEMENTED → TESTED` asks for the evidence of the tests: the manifest of the tests and the digest of the
//!   evidence;
//! * `TESTED → APPROVED` (and `DEPRECATED → APPROVED`) asks for the license and for the signed evidence, when the
//!   policy requires them.
//!
//! **Every** state may go to `REVOKED`, and a revoked implementation is neither recommended nor instantiated.

use std::fmt;
use std::str::FromStr;

use tase_contract::TaseError;
use tase_registry::AssetEntry;

use crate::implementation::{Implementation, ImplementationRegistry};

/// The state of a knowledge card (requirements §14.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KnowledgeCardStatus {
    /// The card is written, nobody has reviewed it yet.
    Documented,
    /// The card has been reviewed.
    Reviewed,
}

impl KnowledgeCardStatus {
    /// The state on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeCardStatus::Documented => "DOCUMENTED",
            KnowledgeCardStatus::Reviewed => "REVIEWED",
        }
    }

    /// The state of a name; `None` when the name is not a state.
    pub fn from_name(name: &str) -> Option<KnowledgeCardStatus> {
        match name {
            "DOCUMENTED" => Some(KnowledgeCardStatus::Documented),
            "REVIEWED" => Some(KnowledgeCardStatus::Reviewed),
            _ => None,
        }
    }

    /// Whether the card may go to this state.
    ///
    /// A card goes from `DOCUMENTED` to `REVIEWED` when a reviewer accepts it, and back when the review is
    /// withdrawn. Nothing else.
    pub fn may_transition_to(self, to: KnowledgeCardStatus) -> bool {
        matches!(
            (self, to),
            (KnowledgeCardStatus::Documented, KnowledgeCardStatus::Reviewed)
                | (KnowledgeCardStatus::Reviewed, KnowledgeCardStatus::Documented)
        )
    }
}

impl fmt::Display for KnowledgeCardStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for KnowledgeCardStatus {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        KnowledgeCardStatus::from_name(s)
            .ok_or_else(|| TaseError::invalid_argument(format!("unknown card state {s:?}")))
    }
}

/// The state of a language implementation (requirements §14.4).
///
/// `PLANNED` is the state of an implementation that is announced but not written yet. It is not a state of the
/// design book: the design book starts at `IMPLEMENTED`, because it describes an implementation that exists. The
/// catalog needs the state before that, so that a plan may speak of an implementation that is on its way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImplementationStatus {
    /// The implementation is announced, but nothing is written yet.
    Planned,
    /// The implementation is written and compiles.
    Implemented,
    /// The implementation is written and its tests passed.
    Tested,
    /// The implementation is certified for its language: it may be instantiated.
    Approved,
    /// The implementation is certified but withdrawn: it may not be instantiated any more.
    Deprecated,
    /// The implementation is withdrawn for good: it is neither recommended nor instantiated.
    Revoked,
}

impl ImplementationStatus {
    /// The state on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ImplementationStatus::Planned => "PLANNED",
            ImplementationStatus::Implemented => "IMPLEMENTED",
            ImplementationStatus::Tested => "TESTED",
            ImplementationStatus::Approved => "APPROVED",
            ImplementationStatus::Deprecated => "DEPRECATED",
            ImplementationStatus::Revoked => "REVOKED",
        }
    }

    /// The state of a name; `None` when the name is not a state.
    pub fn from_name(name: &str) -> Option<ImplementationStatus> {
        match name {
            "PLANNED" => Some(ImplementationStatus::Planned),
            "IMPLEMENTED" => Some(ImplementationStatus::Implemented),
            "TESTED" => Some(ImplementationStatus::Tested),
            "APPROVED" => Some(ImplementationStatus::Approved),
            "DEPRECATED" => Some(ImplementationStatus::Deprecated),
            "REVOKED" => Some(ImplementationStatus::Revoked),
            _ => None,
        }
    }

    /// Whether the implementation may be instantiated in its language.
    ///
    /// Only `APPROVED` opens the gate (requirements §14.4).
    pub fn is_instantiable(self) -> bool {
        matches!(self, ImplementationStatus::Approved)
    }

    /// Whether the implementation may be recommended.
    ///
    /// A revoked implementation is not recommended any more.
    pub fn is_recommendable(self) -> bool {
        !matches!(self, ImplementationStatus::Revoked)
    }

    /// Whether the implementation may go to this state.
    pub fn may_transition_to(self, to: ImplementationStatus) -> bool {
        match self {
            ImplementationStatus::Planned => matches!(
                to,
                ImplementationStatus::Implemented | ImplementationStatus::Revoked
            ),
            ImplementationStatus::Implemented => matches!(
                to,
                ImplementationStatus::Tested
                    | ImplementationStatus::Deprecated
                    | ImplementationStatus::Revoked
            ),
            ImplementationStatus::Tested => matches!(
                to,
                ImplementationStatus::Approved
                    | ImplementationStatus::Implemented
                    | ImplementationStatus::Deprecated
                    | ImplementationStatus::Revoked
            ),
            ImplementationStatus::Approved => matches!(
                to,
                ImplementationStatus::Deprecated | ImplementationStatus::Revoked
            ),
            ImplementationStatus::Deprecated => matches!(
                to,
                ImplementationStatus::Approved | ImplementationStatus::Revoked
            ),
            ImplementationStatus::Revoked => false,
        }
    }
}

impl fmt::Display for ImplementationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ImplementationStatus {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ImplementationStatus::from_name(s)
            .ok_or_else(|| TaseError::invalid_argument(format!("unknown implementation state {s:?}")))
    }
}

/// The evidence that a caller presents when it moves an implementation from one state to another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    /// The license of the implementation (an SPDX name).
    pub license: Option<String>,
    /// Whether the release carries a signature.
    pub signed: bool,
    /// Where the manifest of the tests of the implementation lives.
    pub tests_manifest: Option<String>,
    /// The digest of the evidence of the tests.
    pub test_evidence_digest: Option<String>,
}

impl Evidence {
    /// The evidence of the tests: the manifest of the tests and the digest of the evidence.
    pub fn tests(tests_manifest: &str, test_evidence_digest: &str) -> Evidence {
        Evidence {
            tests_manifest: Some(tests_manifest.to_string()),
            test_evidence_digest: Some(test_evidence_digest.to_string()),
            ..Evidence::default()
        }
    }

    /// The evidence of the approval: the license and the signature.
    pub fn approval(license: &str, signed: bool) -> Evidence {
        Evidence {
            license: Some(license.to_string()),
            signed,
            ..Evidence::default()
        }
    }

    /// Whether the evidence of the tests is there.
    pub fn has_test_evidence(&self) -> bool {
        self.tests_manifest
            .as_deref()
            .map(|text| !text.trim().is_empty())
            .unwrap_or(false)
            && self
                .test_evidence_digest
                .as_deref()
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
    }

    /// Whether the evidence of the license is there.
    pub fn has_license(&self) -> bool {
        self.license
            .as_deref()
            .map(|text| !text.trim().is_empty())
            .unwrap_or(false)
    }
}

/// The policy of the governance: what the evidence must carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GovernancePolicy {
    /// Whether an implementation may only be approved when its license is there.
    pub require_license: bool,
    /// Whether an implementation may only be approved when the release carries a signature.
    pub require_signed_evidence: bool,
}

impl Default for GovernancePolicy {
    fn default() -> Self {
        // The default is the strict policy: an implementation is certified only when the license and the signature
        // are there. A deployment may loosen it with its own policy.
        GovernancePolicy {
            require_license: true,
            require_signed_evidence: true,
        }
    }
}

impl GovernancePolicy {
    /// A policy that asks for nothing.
    pub fn permissive() -> GovernancePolicy {
        GovernancePolicy {
            require_license: false,
            require_signed_evidence: false,
        }
    }

    /// Moves an implementation from one state to another, when the policy allows it.
    ///
    /// The refusal says why: the state machine is refused with
    /// [`ErrorCode::PolicyRejected`], and a missing evidence with the same code.
    pub fn may_transition(
        &self,
        from: ImplementationStatus,
        to: ImplementationStatus,
        evidence: &Evidence,
    ) -> Result<ImplementationStatus, TaseError> {
        if !from.may_transition_to(to) {
            return Err(TaseError::policy_rejected(format!(
                "an implementation may not go from {from} to {to}"
            )));
        }
        if to == ImplementationStatus::Tested && !evidence.has_test_evidence() {
            return Err(TaseError::policy_rejected(
                "the evidence of the tests is missing: an implementation may only be TESTED with the manifest of its tests and the digest of the evidence".to_string(),
            ));
        }
        if to == ImplementationStatus::Approved {
            self.may_approve(evidence)?;
        }
        Ok(to)
    }

    /// Whether the evidence is enough to approve an implementation.
    pub fn may_approve(&self, evidence: &Evidence) -> Result<(), TaseError> {
        if self.require_license && !evidence.has_license() {
            return Err(TaseError::policy_rejected(
                "the license of the implementation is missing: an unknown license is not a free license".to_string(),
            ));
        }
        if self.require_signed_evidence && !evidence.signed {
            return Err(TaseError::policy_rejected(
                "the signed evidence of the implementation is missing".to_string(),
            ));
        }
        Ok(())
    }

    /// The gate of the instantiation.
    ///
    /// The implementation must be `APPROVED` and the license of the asset must be on the list of the licenses the
    /// caller allows. An empty list allows nothing: an unknown license is not a free license.
    pub fn may_instantiate(
        &self,
        implementation: &Implementation,
        asset_license: Option<&str>,
        allowed_licenses: &[String],
    ) -> Result<(), TaseError> {
        if !implementation.status.is_instantiable() {
            return Err(TaseError::policy_rejected(format!(
                "the implementation {} is {}: only an APPROVED implementation may be instantiated",
                implementation.id, implementation.status
            )));
        }
        if !license_allows(asset_license, allowed_licenses) {
            return Err(TaseError::policy_rejected(format!(
                "the license of the implementation {} is not on the list of the allowed licenses",
                implementation.id
            )));
        }
        Ok(())
    }
}

/// Whether the license is on the list of the licenses that are allowed.
///
/// An asset without a license is not allowed: an unknown license is not a free license. An empty list allows
/// nothing.
pub fn license_allows(license: Option<&str>, allowed: &[String]) -> bool {
    let Some(license) = license else {
        return false;
    };
    let license = license.trim();
    if license.is_empty() {
        return false;
    }
    allowed
        .iter()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(license))
}

/// The state of one implementation of an asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplementationState {
    /// The id of the implementation.
    pub id: String,
    /// The language of the implementation.
    pub language: String,
    /// The state of the implementation.
    pub status: ImplementationStatus,
}

/// The governance of an asset: what may be done with it, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceState {
    /// The id of the asset.
    pub asset_id: String,
    /// The state of the knowledge card.
    pub card_status: KnowledgeCardStatus,
    /// The state of every implementation of the asset.
    pub implementations: Vec<ImplementationState>,
    /// Whether the asset may be recommended in the requested language.
    pub recommendable: bool,
    /// Whether the asset may be instantiated in the requested language.
    pub instantiable: bool,
    /// Why the asset may (or may not) be recommended and instantiated.
    pub reasons: Vec<String>,
}

/// The governance of an asset, read from its card and from the implementations that are registered for it.
///
/// The recommendability and the instantiability are **derived**, never declared: a card that is revoked in the
/// requested language is not recommended, and only an approved implementation of that language is instantiable.
pub fn assess(
    entry: &AssetEntry,
    implementations: &ImplementationRegistry,
    language: &str,
) -> GovernanceState {
    let card_status =
        KnowledgeCardStatus::from_name(&entry.status).unwrap_or(KnowledgeCardStatus::Documented);

    let mut states: Vec<ImplementationState> = Vec::new();
    for implementation in implementations.for_asset(&entry.id) {
        states.push(ImplementationState {
            id: implementation.id.clone(),
            language: implementation.language.clone(),
            status: implementation.status,
        });
    }

    let in_language = implementations.for_asset_and_language(&entry.id, language);
    let revoked = in_language
        .iter()
        .any(|implementation| implementation.status == ImplementationStatus::Revoked);
    let approved = in_language
        .iter()
        .any(|implementation| implementation.status.is_instantiable());

    let mut reasons: Vec<String> = Vec::new();
    if revoked {
        reasons.push(format!(
            "the implementation of the asset in {language} is revoked: it must not be recommended"
        ));
    }
    if !approved {
        reasons.push(format!(
            "the asset has no approved implementation in {language}: it is a knowledge card"
        ));
    }

    GovernanceState {
        asset_id: entry.id.clone(),
        card_status,
        implementations: states,
        recommendable: !revoked,
        instantiable: approved,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implementation::ImplementationRegistry;

    /// The card states and the transitions that the design book allows.
    const CARD_TRANSITIONS: [(KnowledgeCardStatus, KnowledgeCardStatus); 2] = [
        (KnowledgeCardStatus::Documented, KnowledgeCardStatus::Reviewed),
        (KnowledgeCardStatus::Reviewed, KnowledgeCardStatus::Documented),
    ];

    /// Every transition of the implementation state machine.
    const IMPLEMENTATION_TRANSITIONS: [(ImplementationStatus, ImplementationStatus); 13] = [
        (ImplementationStatus::Planned, ImplementationStatus::Implemented),
        (ImplementationStatus::Planned, ImplementationStatus::Revoked),
        (ImplementationStatus::Implemented, ImplementationStatus::Tested),
        (ImplementationStatus::Implemented, ImplementationStatus::Deprecated),
        (ImplementationStatus::Implemented, ImplementationStatus::Revoked),
        (ImplementationStatus::Tested, ImplementationStatus::Approved),
        (ImplementationStatus::Tested, ImplementationStatus::Implemented),
        (ImplementationStatus::Tested, ImplementationStatus::Deprecated),
        (ImplementationStatus::Tested, ImplementationStatus::Revoked),
        (ImplementationStatus::Approved, ImplementationStatus::Deprecated),
        (ImplementationStatus::Approved, ImplementationStatus::Revoked),
        (ImplementationStatus::Deprecated, ImplementationStatus::Approved),
        (ImplementationStatus::Deprecated, ImplementationStatus::Revoked),
    ];

    /// An implementation with the given state.
    fn implementation(status: ImplementationStatus) -> Implementation {
        Implementation {
            id: "algorithm.graph.dijkstra.rust.binary_heap".to_string(),
            asset_id: "algorithm.graph.dijkstra".to_string(),
            language: "rust".to_string(),
            version: "1.0.0".to_string(),
            status,
            engine: "tase-template".to_string(),
            path: Some("templates/rust/graph/dijkstra".to_string()),
            sha256: Some("a".repeat(64)),
            parameters_schema: Some("schemas/dijkstra-params.json".to_string()),
            tests_manifest: Some("tests/dijkstra.yaml".to_string()),
        }
    }

    #[test]
    fn the_states_of_a_card_are_the_two_of_the_design_book() {
        assert_eq!(
            KnowledgeCardStatus::from_name("DOCUMENTED"),
            Some(KnowledgeCardStatus::Documented)
        );
        assert_eq!(
            KnowledgeCardStatus::from_name("REVIEWED"),
            Some(KnowledgeCardStatus::Reviewed)
        );
        assert_eq!(KnowledgeCardStatus::from_name("APPROVED"), None);
        assert_eq!(
            KnowledgeCardStatus::Documented.to_string(),
            KnowledgeCardStatus::Documented.as_str()
        );
        assert!("NONSENSE".parse::<KnowledgeCardStatus>().is_err());
    }

    #[test]
    fn the_states_of_an_implementation_are_the_ones_of_the_design_book() {
        for status in [
            ImplementationStatus::Planned,
            ImplementationStatus::Implemented,
            ImplementationStatus::Tested,
            ImplementationStatus::Approved,
            ImplementationStatus::Deprecated,
            ImplementationStatus::Revoked,
        ] {
            assert_eq!(ImplementationStatus::from_name(status.as_str()), Some(status));
            assert_eq!(status.to_string(), status.as_str());
            assert_eq!(status.as_str().parse::<ImplementationStatus>().unwrap(), status);
        }
        assert_eq!(ImplementationStatus::from_name("DRAFT"), None);
        assert!("DRAFT".parse::<ImplementationStatus>().is_err());
    }

    #[test]
    fn every_allowed_transition_of_the_card_is_allowed() {
        for (from, to) in CARD_TRANSITIONS {
            assert!(from.may_transition_to(to), "{from} -> {to}");
        }
    }

    #[test]
    fn every_other_transition_of_the_card_is_refused() {
        let all = [
            KnowledgeCardStatus::Documented,
            KnowledgeCardStatus::Reviewed,
        ];
        for from in all {
            for to in all {
                let allowed = matches!(
                    (from, to),
                    (KnowledgeCardStatus::Documented, KnowledgeCardStatus::Reviewed)
                        | (KnowledgeCardStatus::Reviewed, KnowledgeCardStatus::Documented)
                );
                assert_eq!(from.may_transition_to(to), allowed, "{from} -> {to}");
            }
        }
    }

    #[test]
    fn every_allowed_transition_of_an_implementation_is_allowed() {
        for (from, to) in IMPLEMENTATION_TRANSITIONS {
            assert!(from.may_transition_to(to), "{from} -> {to}");
        }
    }

    #[test]
    fn every_other_transition_of_an_implementation_is_refused() {
        let all = [
            ImplementationStatus::Planned,
            ImplementationStatus::Implemented,
            ImplementationStatus::Tested,
            ImplementationStatus::Approved,
            ImplementationStatus::Deprecated,
            ImplementationStatus::Revoked,
        ];
        let allowed_pairs = IMPLEMENTATION_TRANSITIONS;
        for from in all {
            for to in all {
                let allowed = allowed_pairs.contains(&(from, to));
                assert_eq!(from.may_transition_to(to), allowed, "{from} -> {to}");
            }
        }
    }

    #[test]
    fn a_revoked_implementation_is_terminal() {
        for to in [
            ImplementationStatus::Planned,
            ImplementationStatus::Implemented,
            ImplementationStatus::Tested,
            ImplementationStatus::Approved,
            ImplementationStatus::Deprecated,
            ImplementationStatus::Revoked,
        ] {
            assert!(!ImplementationStatus::Revoked.may_transition_to(to), "{to}");
        }
        assert!(!ImplementationStatus::Revoked.is_recommendable());
        assert!(!ImplementationStatus::Revoked.is_instantiable());
    }

    #[test]
    fn only_the_approved_implementation_is_instantiable() {
        assert!(ImplementationStatus::Approved.is_instantiable());
        for status in [
            ImplementationStatus::Planned,
            ImplementationStatus::Implemented,
            ImplementationStatus::Tested,
            ImplementationStatus::Deprecated,
            ImplementationStatus::Revoked,
        ] {
            assert!(!status.is_instantiable(), "{status}");
        }
    }

    #[test]
    fn the_evidence_of_the_tests_is_needed_to_be_tested() {
        let policy = GovernancePolicy::default();
        let empty = Evidence::default();
        let error = policy
            .may_transition(
                ImplementationStatus::Implemented,
                ImplementationStatus::Tested,
                &empty,
            )
            .expect_err("there is no evidence");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert!(error.message.contains("tests"), "{error}");
        // with the evidence the transition is allowed
        let evidence = Evidence::tests("tests/dijkstra.yaml", &"b".repeat(64));
        assert_eq!(
            policy
                .may_transition(
                    ImplementationStatus::Implemented,
                    ImplementationStatus::Tested,
                    &evidence
                )
                .expect("the evidence is there"),
            ImplementationStatus::Tested
        );
    }

    #[test]
    fn the_license_and_the_signature_are_needed_to_be_approved() {
        let policy = GovernancePolicy::default();
        let tested = Evidence::tests("tests/dijkstra.yaml", &"b".repeat(64));
        // the tests alone are not enough to approve
        let error = policy
            .may_transition(ImplementationStatus::Tested, ImplementationStatus::Approved, &tested)
            .expect_err("the license is missing");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert!(error.message.contains("license"), "{error}");
        // the license alone is not enough either
        let licensed = Evidence::approval("MIT", false);
        let error = policy
            .may_transition(ImplementationStatus::Tested, ImplementationStatus::Approved, &licensed)
            .expect_err("the signature is missing");
        assert!(error.message.contains("signed"), "{error}");
        // both are enough
        let both = Evidence {
            tests_manifest: tested.tests_manifest.clone(),
            test_evidence_digest: tested.test_evidence_digest.clone(),
            ..Evidence::approval("MIT", true)
        };
        assert_eq!(
            policy
                .may_transition(ImplementationStatus::Tested, ImplementationStatus::Approved, &both)
                .expect("the evidence is there"),
            ImplementationStatus::Approved
        );
    }

    #[test]
    fn a_permissive_policy_asks_for_no_evidence() {
        let policy = GovernancePolicy::permissive();
        assert_eq!(
            policy
                .may_transition(
                    ImplementationStatus::Tested,
                    ImplementationStatus::Approved,
                    &Evidence::default()
                )
                .expect("the policy asks for nothing"),
            ImplementationStatus::Approved
        );
    }

    #[test]
    fn an_illegal_transition_is_refused_by_the_policy() {
        let policy = GovernancePolicy::permissive();
        let error = policy
            .may_transition(
                ImplementationStatus::Planned,
                ImplementationStatus::Approved,
                &Evidence::default(),
            )
            .expect_err("the state machine refuses it");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert!(error.message.contains("PLANNED"), "{error}");
    }

    #[test]
    fn a_license_that_is_not_on_the_list_is_not_allowed() {
        let policy = GovernancePolicy::default();
        let approved = implementation(ImplementationStatus::Approved);
        assert!(
            policy
                .may_instantiate(&approved, Some("MIT"), &["MIT".to_string()])
                .is_ok()
        );
        // the license is compared without case
        assert!(
            policy
                .may_instantiate(&approved, Some("mit"), &["MIT".to_string()])
                .is_ok()
        );
        // a license that is not on the list is refused
        let error = policy
            .may_instantiate(&approved, Some("Apache-2.0"), &["MIT".to_string()])
            .expect_err("the license is not allowed");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        // an asset without a license is not allowed either
        assert!(
            policy
                .may_instantiate(&approved, None, &["MIT".to_string()])
                .is_err()
        );
        // an empty list allows nothing
        assert!(policy.may_instantiate(&approved, Some("MIT"), &[]).is_err());
    }

    #[test]
    fn an_implementation_that_is_not_approved_is_not_instantiable() {
        let policy = GovernancePolicy::default();
        for status in [
            ImplementationStatus::Planned,
            ImplementationStatus::Implemented,
            ImplementationStatus::Tested,
            ImplementationStatus::Deprecated,
            ImplementationStatus::Revoked,
        ] {
            let error = policy
                .may_instantiate(
                    &implementation(status),
                    Some("MIT"),
                    &["MIT".to_string()],
                )
                .expect_err("the implementation is not approved");
            assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        }
    }

    #[test]
    fn the_governance_of_an_asset_without_implementation_says_it_is_a_knowledge_card() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = tase_registry::Registry::load(&dir).expect("the catalog is valid");
        let implementations = ImplementationRegistry::from_registry(&registry).expect("the registry is read");
        // A pattern carries no implementation: it is a shape, not one algorithm.
        let entry = registry
            .get("pattern.gof.creational.abstract_factory")
            .expect("the asset is there");
        let state = assess(entry, &implementations, "rust");
        assert_eq!(state.card_status, KnowledgeCardStatus::Documented);
        assert!(state.implementations.is_empty());
        assert!(state.recommendable);
        assert!(!state.instantiable);
        assert!(state.reasons.iter().any(|reason| reason.contains("knowledge card")));
    }

    #[test]
    fn an_algorithm_with_an_implementation_is_not_only_a_card() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = tase_registry::Registry::load(&dir).expect("the catalog is valid");
        let implementations = ImplementationRegistry::from_registry(&registry).expect("the registry is read");
        // The fifty algorithms of the catalogue carry their implementation (the crate `tase-impls`), in the state
        // TESTED. The card is still a card: the two axes are separate (requirements §14.4).
        let entry = registry.get("algorithm.graph.dijkstra").expect("the asset is there");
        let state = assess(entry, &implementations, "rust");
        assert_eq!(state.card_status, KnowledgeCardStatus::Documented);
        assert_eq!(state.implementations.len(), 1, "the dijkstra carries its implementation");
        assert_eq!(state.implementations[0].status, ImplementationStatus::Tested);
        assert!(state.recommendable, "a tested implementation may be recommended");
        assert!(
            !state.instantiable,
            "TESTED is not APPROVED: only an approved implementation may be instantiated (D-61)"
        );
    }

    #[test]
    fn a_revoked_implementation_is_not_recommendable() {
        let mut registry = ImplementationRegistry::new();
        registry
            .register(implementation(ImplementationStatus::Revoked))
            .expect("the implementation is registered");
        let entry = tase_registry::AssetEntry {
            id: "algorithm.graph.dijkstra".to_string(),
            version: "1.0.0".to_string(),
            kind: "algorithm".to_string(),
            category: "graph".to_string(),
            names: Default::default(),
            aliases: Vec::new(),
            status: "DOCUMENTED".to_string(),
            intent: "做什么。".to_string(),
            constraints: Vec::new(),
            counterexamples: Vec::new(),
            alternatives: Vec::new(),
            complexity_time: None,
            complexity_space: None,
            implementations: Vec::new(),
            license: Some("MIT".to_string()),
            references: Vec::new(),
            digest: String::new(),
            card_path: None,
        };
        let state = assess(&entry, &registry, "rust");
        assert!(!state.recommendable);
        assert!(!state.instantiable);
        // an approved implementation in another language says nothing about this one
        let state = assess(&entry, &registry, "typescript");
        assert!(state.recommendable);
        assert!(!state.instantiable);
    }
}
