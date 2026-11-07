//! TASE catalog: the recommendation layer of the unified standard asset catalog (detailed design §19, basic
//! design §17, requirements §14.4).
//!
//! The crate sits on top of [`tase_registry`], which already loads the 93 assets of `assets/tase-catalog`, and adds
//! the three things the design book asks of the catalog and the registry does not hold:
//!
//! * [`assets`] - the view of an asset: the layer, the tags and the aliases that the catalog derives from the
//!   manifest (basic design §17.1);
//! * [`index`] - the index of the catalog: the way to find an asset by its id, its name, its alias, its category,
//!   its layer, its language or its tag, without touching the disk again;
//! * [`recommend`] - the selection and the ranking: the candidates, the breakdown of the score, the
//!   counterexamples and the risks, and the reasons why a candidate was refused;
//! * [`governance`] - the state of the knowledge card, the state of the language implementations and the rules
//!   that move an implementation from one state to another;
//! * [`implementation`] - the registry of the implementations that exist for the assets.
//!
//! # A knowledge card is not an executable implementation
//!
//! **The catalog of this repository holds 93 knowledge cards and not one language implementation.** Every asset is
//! `DOCUMENTED` and every `implementations` list is empty. The crate keeps that difference visible everywhere:
//!
//! * [`governance::ImplementationStatus::Approved`] is the only state that opens the gate of the instantiation, and
//!   no asset of this repository is in that state;
//! * the answer of [`recommend::recommend`] carries `instantiable: false` and the reason for every one of the 93
//!   assets;
//! * [`implementation::ImplementationRegistry::from_registry`] returns an **empty** registry for this repository:
//!   the catalog never invents an implementation that was not written.
//!
//! # The answer carries the counterexamples and the risks
//!
//! Requirements §14.4 says it in one line: "反例、许可和风险必须随机器结果返回，防止 Agent 为了减少 Token
//! 无条件套用不适合的算法". Every candidate of a recommendation therefore carries the counterexamples of its card,
//! the risks the card declares, its license and its alternatives - never a bare name.
//!
//! # The rubric of the score
//!
//! The score is the sum of four parts, and the sum of the parts is the score:
//!
//! | the part | the weight | what it measures |
//! |---|---|---|
//! | `intent` | 60 | the share of the intent the card covers |
//! | `constraints` | 15 | the properties the card declares in the caller's favour |
//! | `complexity` | 10 | the declared cost of the card against the budget of the caller |
//! | `implementation` | 15 | the state of the implementation of the card in the requested language |
//!
//! The parts are integers and the order of the candidates is the score and then the id: the answer is the same for
//! the same request, every time. There is no clock, no randomness and no hash order in this crate.
//!
//! # The gate
//!
//! A candidate that does not answer the request is not recommended, and the refusal says why
//! (detailed design §19.2: `rejected=[{asset_id,reason}]`). A card that says **nothing** about a property is
//! neither refused nor rewarded on it: the risk is reported instead.
//!
//! The crate has no third-party dependency: the manifests and the digests come from `tase-registry`, the DTOs, the
//! identifiers and the error codes from `tase-contract`.

#![forbid(unsafe_code)]

pub mod assets;
pub mod governance;
pub mod implementation;
pub mod index;
pub mod recommend;

use std::path::Path;

use tase_contract::TaseError;
use tase_registry::Registry;

pub use assets::{AssetView, Layer, Traits, tags_of, traits_of};
pub use governance::{
    Evidence, GovernancePolicy, GovernanceState, ImplementationState, ImplementationStatus,
    KnowledgeCardStatus, assess, license_allows,
};
pub use implementation::{Implementation, ImplementationRegistry};
pub use index::{CatalogIndex, CatalogQuery};
pub use recommend::{
    Complexity, ComplexityBudget, ComplexityRank, ConfidenceSource, Constraints, RecommendRequest,
    Recommendation, RecommendationResult, RejectedCandidate, RejectionReason, ScoreBreakdown,
    ScoreComponent, recommend,
};

/// The version of the catalog.
pub const CATALOG_VERSION: &str = "1.1.0";

/// The catalog: the index, the registry of the implementations and the policy of the governance.
///
/// The catalog is built once, out of the registry that has already read the manifests. Every answer it gives is a
/// lookup and a pure computation: the catalog never goes back to the disk.
#[derive(Clone, Debug)]
pub struct Catalog {
    index: CatalogIndex,
    implementations: ImplementationRegistry,
    policy: GovernancePolicy,
}

impl Catalog {
    /// Loads a catalog from the disk.
    ///
    /// The directory holds the `catalog.json` of the index and the `assets/<asset_id>/manifest.json` of every
    /// asset, the way the registry reads it.
    pub fn load(dir: &Path) -> Result<Catalog, TaseError> {
        let registry = Registry::load(dir)?;
        Catalog::from_registry(&registry)
    }

    /// Builds the catalog out of a registry that has already read the manifests.
    pub fn from_registry(registry: &Registry) -> Result<Catalog, TaseError> {
        let index = CatalogIndex::build(registry)?;
        let implementations = ImplementationRegistry::from_registry(registry)?;
        Ok(Catalog {
            index,
            implementations,
            policy: GovernancePolicy::default(),
        })
    }

    /// The index of the catalog.
    pub fn index(&self) -> &CatalogIndex {
        &self.index
    }

    /// The registry of the implementations.
    pub fn implementations(&self) -> &ImplementationRegistry {
        &self.implementations
    }

    /// The policy of the governance.
    pub fn policy(&self) -> &GovernancePolicy {
        &self.policy
    }

    /// Replaces the policy of the governance.
    pub fn with_policy(mut self, policy: GovernancePolicy) -> Catalog {
        self.policy = policy;
        self
    }

    /// The number of the assets of the catalog.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The asset with this id, this name or this alias.
    pub fn inspect(&self, id_or_alias: &str) -> Option<&AssetView> {
        let key = id_or_alias.trim();
        if let Some(view) = self.index.get(key) {
            return Some(view);
        }
        self.index
            .resolve_alias(key)
            .and_then(|id| self.index.get(id))
    }

    /// The candidates of a request, with the breakdown of the score, the counterexamples and the risks.
    pub fn recommend(&self, request: &RecommendRequest) -> Result<RecommendationResult, TaseError> {
        recommend(&self.index, &self.implementations, request)
    }

    /// The governance of an asset in a language: what may be done with it, and why.
    pub fn governance_of(&self, id_or_alias: &str, language: &str) -> Option<GovernanceState> {
        let view = self.inspect(id_or_alias)?;
        Some(assess(
            &view.entry,
            &self.implementations,
            language,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The catalog of the repository.
    fn real_catalog() -> Catalog {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        Catalog::load(&dir).expect("the catalog of the repository is valid")
    }

    #[test]
    fn the_catalog_of_the_repository_holds_ninety_three_knowledge_cards() {
        let catalog = real_catalog();
        assert_eq!(catalog.len(), 93);
        assert!(!catalog.is_empty());
        assert_eq!(catalog.index().len(), 93);
        // not one implementation
        assert!(catalog.implementations().is_empty());
        for view in catalog.index().views() {
            assert_eq!(view.entry.status, "DOCUMENTED", "{}", view.id());
            assert!(view.entry.implementations.is_empty(), "{}", view.id());
        }
    }

    #[test]
    fn an_asset_is_inspected_by_its_id_and_by_its_alias() {
        let catalog = real_catalog();
        let by_id = catalog.inspect("algorithm.graph.kruskal").expect("the asset is there");
        assert_eq!(by_id.id(), "algorithm.graph.kruskal");
        let by_alias = catalog.inspect("MST").expect("the alias is there");
        assert_eq!(by_alias.id(), "algorithm.graph.kruskal");
        let by_name = catalog.inspect("最小生成树").expect("the name is there");
        assert_eq!(by_name.id(), "algorithm.graph.kruskal");
        assert!(catalog.inspect("nothing at all").is_none());
    }

    #[test]
    fn the_governance_of_an_asset_of_the_repository_says_it_is_a_knowledge_card() {
        let catalog = real_catalog();
        let state = catalog
            .governance_of("algorithm.graph.dijkstra", "rust")
            .expect("the asset is there");
        assert_eq!(state.card_status, KnowledgeCardStatus::Documented);
        assert!(state.recommendable);
        assert!(!state.instantiable);
        assert!(state.implementations.is_empty());
    }

    #[test]
    fn the_catalog_recommends_without_going_back_to_the_disk() {
        let catalog = real_catalog();
        let request = RecommendRequest {
            intent: "单例".to_string(),
            language: "rust".to_string(),
            ..RecommendRequest::default()
        };
        let result = catalog.recommend(&request).expect("the answer is there");
        assert_eq!(result.recommended[0].asset_id, "pattern.gof.creational.singleton");
        assert!(!result.recommended[0].instantiable);
    }
}
