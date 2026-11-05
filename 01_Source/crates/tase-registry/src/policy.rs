//! The license and the approval policy of the registry (requirements §14.4, design §19.1 and §19.3).
//!
//! The policy answers two questions:
//!
//! * **what may be done with an asset?** - a knowledge card may be read, searched and quoted; it may not be
//!   instantiated. Only an asset that has an implementation of the **requested language** with the state
//!   `APPROVED` may be instantiated (requirements §14.4: an implementation that is approved for Rust says
//!   nothing about TypeScript);
//! * **may this asset be used here?** - the license of the asset must be on the list of the licenses the
//!   caller allows. An asset without a license is not allowed.
//!
//! The state of a card and the state of an implementation are two axes:
//!
//! | the card is | the implementations are | the state of the asset |
//! |---|---|---|
//! | `DOCUMENTED` | none | `KNOWLEDGE_ONLY` |
//! | `REVIEWED` | none | `KNOWLEDGE_ONLY` (or `REVIEW_REQUIRED` when the policy allows unapproved assets) |
//! | any | one of them is `APPROVED` | `APPROVED` |
//!
//! `allow_unapproved` only changes the state that is **reported**; it never lets an asset be instantiated.
//! The instantiation gate is always the approved implementation of the requested language.

use std::fmt;
use std::str::FromStr;

use tase_contract::TaseError;

use crate::manifest::AssetEntry;

/// The state of the approval of an asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Approval {
    /// The asset has an implementation that is approved.
    Approved,
    /// The card of the asset is reviewed, but no implementation is approved.
    ReviewRequired,
    /// The asset is a knowledge card: it may be read, it may not be instantiated.
    KnowledgeOnly,
}

impl Approval {
    /// The state on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Approval::Approved => "APPROVED",
            Approval::ReviewRequired => "REVIEW_REQUIRED",
            Approval::KnowledgeOnly => "KNOWLEDGE_ONLY",
        }
    }

    /// The state of a name.
    pub fn from_name(name: &str) -> Option<Approval> {
        match name {
            "APPROVED" => Some(Approval::Approved),
            "REVIEW_REQUIRED" => Some(Approval::ReviewRequired),
            "KNOWLEDGE_ONLY" => Some(Approval::KnowledgeOnly),
            _ => None,
        }
    }

    /// Whether the asset may be instantiated (only an approved implementation may).
    pub fn may_instantiate(self) -> bool {
        matches!(self, Approval::Approved)
    }
}

impl fmt::Display for Approval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Approval {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Approval::from_name(s)
            .ok_or_else(|| TaseError::invalid_argument(format!("unknown approval state {s:?}")))
    }
}

/// The policy of the registry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Whether a card that is reviewed may be used without an approved implementation.
    ///
    /// The default is `false`: an asset without an approved implementation is a knowledge card and nothing
    /// else. This flag never opens the instantiation gate.
    pub allow_unapproved: bool,
}

impl Policy {
    /// The state of an asset: what may be done with it.
    pub fn approval_of(&self, entry: &AssetEntry) -> Approval {
        // an approved implementation of any language is what makes an asset an executable template
        if entry.implementations.iter().any(|impl_| impl_.is_approved()) {
            return Approval::Approved;
        }
        match entry.status.as_str() {
            "REVIEWED" if self.allow_unapproved => Approval::ReviewRequired,
            _ => Approval::KnowledgeOnly,
        }
    }

    /// Whether the asset may be instantiated in this language.
    ///
    /// Only an approved implementation of the **requested** language opens the gate (requirements §14.4).
    /// Every other case is refused with [`ErrorCode::PolicyRejected`], and the message says why: the asset is
    /// a knowledge card.
    pub fn may_instantiate(&self, entry: &AssetEntry, language: &str) -> Result<(), TaseError> {
        if entry.approved_implementation(language).is_some() {
            return Ok(());
        }
        Err(TaseError::policy_rejected(format!(
            "the asset {} has no approved implementation in {language}: a knowledge card is not an executable template",
            entry.id
        )))
    }

    /// Whether the license of the asset is on the list of the licenses that are allowed.
    ///
    /// An asset without a license is not allowed: an unknown license is not a free license.
    pub fn license_allows(&self, entry: &AssetEntry, allowed: &[String]) -> bool {
        let Some(license) = entry.license.as_deref() else {
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

    /// Whether the asset may be instantiated in this language **and** its license is allowed.
    ///
    /// This is the gate the executor should call: the language gate comes first, then the license gate.
    pub fn may_instantiate_with_license(
        &self,
        entry: &AssetEntry,
        language: &str,
        allowed_licenses: &[String],
    ) -> Result<(), TaseError> {
        self.may_instantiate(entry, language)?;
        if !self.license_allows(entry, allowed_licenses) {
            return Err(TaseError::policy_rejected(format!(
                "the license of the asset {} is not on the list of the allowed licenses",
                entry.id
            )));
        }
        Ok(())
    }

    /// The languages the asset may be instantiated in.
    pub fn instantiable_languages(&self, entry: &AssetEntry) -> Vec<String> {
        let mut languages: Vec<String> = entry
            .implementations
            .iter()
            .filter(|impl_| impl_.is_approved())
            .map(|impl_| impl_.language.clone())
            .collect();
        languages.sort();
        languages.dedup();
        languages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Registry;
    use crate::manifest::{AssetEntry, ImplementationEntry};
    use std::path::Path;

    /// An asset with the given card state and implementations.
    fn asset(status: &str, implementations: Vec<ImplementationEntry>) -> AssetEntry {
        AssetEntry {
            id: "algorithm.graph.kruskal".to_string(),
            version: "1.0.0".to_string(),
            kind: "algorithm".to_string(),
            category: "graph".to_string(),
            names: Default::default(),
            aliases: Vec::new(),
            status: status.to_string(),
            intent: "做什么。".to_string(),
            constraints: Vec::new(),
            counterexamples: Vec::new(),
            alternatives: Vec::new(),
            complexity_time: None,
            complexity_space: None,
            implementations,
            license: Some("MIT".to_string()),
            references: Vec::new(),
            digest: String::new(),
            card_path: None,
        }
    }

    /// One implementation of an asset.
    fn implementation(language: &str, status: &str) -> ImplementationEntry {
        ImplementationEntry {
            implementation_id: format!("algorithm.graph.kruskal.{language}"),
            language: language.to_string(),
            version: "1.0.0".to_string(),
            status: status.to_string(),
            engine: "tase-template".to_string(),
            path: "templates/rust/graph/kruskal".to_string(),
            sha256: "a".repeat(64),
            parameters_schema: "schemas/kruskal-params.json".to_string(),
            tests_manifest: "tests/kruskal.yaml".to_string(),
        }
    }

    #[test]
    fn a_documented_card_without_implementation_is_a_knowledge_card() {
        let entry = asset("DOCUMENTED", Vec::new());
        assert_eq!(Policy::default().approval_of(&entry), Approval::KnowledgeOnly);
        assert_eq!(
            Policy {
                allow_unapproved: true
            }
            .approval_of(&entry),
            Approval::KnowledgeOnly
        );
    }

    #[test]
    fn a_reviewed_card_without_implementation_is_a_knowledge_card_by_default() {
        let entry = asset("REVIEWED", Vec::new());
        assert_eq!(Policy::default().approval_of(&entry), Approval::KnowledgeOnly);
    }

    #[test]
    fn a_reviewed_card_may_be_reported_as_review_required() {
        let entry = asset("REVIEWED", Vec::new());
        let policy = Policy {
            allow_unapproved: true,
        };
        assert_eq!(policy.approval_of(&entry), Approval::ReviewRequired);
        // the state that is reported is not the state that opens the gate
        assert!(policy.may_instantiate(&entry, "rust").is_err());
    }

    #[test]
    fn an_approved_implementation_makes_the_asset_approved() {
        let entry = asset("DOCUMENTED", vec![implementation("rust", "APPROVED")]);
        assert_eq!(Policy::default().approval_of(&entry), Approval::Approved);
        assert!(Approval::Approved.may_instantiate());
        assert!(!Approval::KnowledgeOnly.may_instantiate());
    }

    #[test]
    fn an_implementation_that_is_not_approved_does_not_make_the_asset_approved() {
        for status in ["IMPLEMENTED", "TESTED", "DEPRECATED", "REVOKED"] {
            let entry = asset("REVIEWED", vec![implementation("rust", status)]);
            assert_eq!(
                Policy::default().approval_of(&entry),
                Approval::KnowledgeOnly,
                "{status}"
            );
        }
    }

    #[test]
    fn only_an_approved_implementation_of_the_requested_language_opens_the_gate() {
        let entry = asset("REVIEWED", vec![implementation("rust", "APPROVED")]);
        let policy = Policy::default();
        assert!(policy.may_instantiate(&entry, "rust").is_ok());
        // the language is compared without case
        assert!(policy.may_instantiate(&entry, "RUST").is_ok());
        // an approved Rust implementation says nothing about TypeScript
        let error = policy.may_instantiate(&entry, "typescript").expect_err("no TypeScript");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert!(error.message.contains("knowledge card"), "{error}");
    }

    #[test]
    fn a_card_without_an_implementation_may_never_be_instantiated() {
        let entry = asset("REVIEWED", Vec::new());
        let policy = Policy {
            allow_unapproved: true,
        };
        let error = policy.may_instantiate(&entry, "rust").expect_err("there is no implementation");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert_eq!(policy.instantiable_languages(&entry), Vec::<String>::new());
    }

    #[test]
    fn a_license_that_is_not_on_the_list_is_not_allowed() {
        let entry = asset("DOCUMENTED", Vec::new());
        let policy = Policy::default();
        assert!(policy.license_allows(&entry, &["MIT".to_string()]));
        assert!(policy.license_allows(&entry, &["mit".to_string()]));
        assert!(!policy.license_allows(&entry, &["Apache-2.0".to_string()]));
        // an empty list allows nothing
        assert!(!policy.license_allows(&entry, &[]));
        // an asset without a license is not allowed either
        let mut unknown = entry.clone();
        unknown.license = None;
        assert!(!policy.license_allows(&unknown, &["MIT".to_string()]));
    }

    #[test]
    fn the_language_gate_comes_before_the_license_gate() {
        let entry = asset("DOCUMENTED", vec![implementation("rust", "APPROVED")]);
        let policy = Policy::default();
        // the license is not allowed: the refusal says so
        let error = policy
            .may_instantiate_with_license(&entry, "rust", &["Apache-2.0".to_string()])
            .expect_err("the license is not allowed");
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert!(error.message.contains("license"), "{error}");
        // the language is wrong: the refusal says that
        let error = policy
            .may_instantiate_with_license(&entry, "typescript", &["MIT".to_string()])
            .expect_err("there is no TypeScript implementation");
        assert!(error.message.contains("knowledge card"), "{error}");
        // everything is in order
        assert!(policy
            .may_instantiate_with_license(&entry, "rust", &["MIT".to_string()])
            .is_ok());
    }

    #[test]
    fn every_asset_of_the_repository_is_a_knowledge_card_and_nothing_else() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = Registry::load(&dir).expect("the catalog of the repository is valid");
        let policy = Policy::default();
        assert_eq!(registry.len(), 93);
        for entry in registry.entries() {
            assert_eq!(
                policy.approval_of(entry),
                Approval::KnowledgeOnly,
                "{} must not be presented as an executable template",
                entry.id
            );
            for language in ["rust", "typescript", "c", "python"] {
                let error = policy
                    .may_instantiate(entry, language)
                    .expect_err("there is no certified implementation");
                assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
            }
        }
    }

    #[test]
    fn the_state_of_an_asset_travels_by_name() {
        for state in [Approval::Approved, Approval::ReviewRequired, Approval::KnowledgeOnly] {
            assert_eq!(Approval::from_name(state.as_str()), Some(state));
            assert_eq!(state.as_str().parse::<Approval>().unwrap(), state);
            assert_eq!(state.to_string(), state.as_str());
        }
        assert!(Approval::from_name("ACTIVE").is_none());
        assert!("ACTIVE".parse::<Approval>().is_err());
    }
}
