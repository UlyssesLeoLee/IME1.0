//! The registry of the implementations: the record of the language implementations that exist for the assets of
//! the catalog (requirements §14.4, detailed design §19.5).
//!
//! # One shape, not two
//!
//! The catalog does not invent a second shape of the implementation:
//!
//! * the manifest of an asset holds a [`tase_registry::ImplementationEntry`] - the fields `implementation_id`,
//!   `language`, `version`, `status`, `engine`, `path`, `sha256`, `parameters_schema` and `tests_manifest`;
//! * the wire shape is [`tase_contract::api::AssetImplementation`] - the same fields;
//! * the record of the catalog is [`Implementation`] - the same fields under the name of the catalog
//!   (`id` for the id of the implementation) plus the asset it belongs to.
//!
//! [`Implementation::from_entry`] carries a manifest entry into the catalog without losing a field, and
//! [`Implementation::to_contract`] carries a record back to the wire shape. The catalog refuses to fabricate what
//! the manifest does not carry: a field that the manifest leaves empty stays empty.
//!
//! # Nothing is invented
//!
//! The catalog of this repository holds **no implementation at all**: every one of the 93 assets is a knowledge
//! card. The registry built out of it is therefore empty, and every query about an implementation answers with
//! nothing.

use tase_contract::TaseError;
use tase_contract::api::AssetImplementation;
use tase_registry::{ImplementationEntry, Registry};

use crate::governance::ImplementationStatus;

/// One implementation of an asset (requirements §14.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Implementation {
    /// The id of the implementation.
    pub id: String,
    /// The id of the asset the implementation belongs to.
    pub asset_id: String,
    /// The language of the implementation.
    pub language: String,
    /// The version of the implementation.
    pub version: String,
    /// The state of the implementation.
    pub status: ImplementationStatus,
    /// The engine that renders the implementation.
    pub engine: String,
    /// Where the implementation lives.
    pub path: Option<String>,
    /// The digest of the package of the implementation.
    pub sha256: Option<String>,
    /// Where the schema of the parameters of the implementation lives.
    pub parameters_schema: Option<String>,
    /// Where the manifest of the tests of the implementation lives.
    pub tests_manifest: Option<String>,
}

impl Implementation {
    /// The record of the catalog, built out of the entry of a manifest.
    ///
    /// A field that the manifest leaves empty stays empty: the catalog does not invent a path, a digest or a
    /// schema.
    pub fn from_entry(asset_id: &str, entry: &ImplementationEntry) -> Implementation {
        Implementation {
            id: entry.implementation_id.clone(),
            asset_id: asset_id.to_string(),
            language: entry.language.clone(),
            version: entry.version.clone(),
            status: ImplementationStatus::from_name(&entry.status)
                .unwrap_or(ImplementationStatus::Planned),
            engine: entry.engine.clone(),
            path: non_empty(&entry.path),
            sha256: non_empty(&entry.sha256),
            parameters_schema: non_empty(&entry.parameters_schema),
            tests_manifest: non_empty(&entry.tests_manifest),
        }
    }

    /// The wire shape of the contract.
    ///
    /// The contract wants every field of an implementation: a record that carries none of them cannot be put on the
    /// wire, and the refusal says so instead of filling the gaps.
    pub fn to_contract(&self) -> Result<AssetImplementation, TaseError> {
        let id = tase_contract::ImplementationId::parse(&self.id)?;
        Ok(AssetImplementation {
            implementation_id: id,
            language: self.language.clone(),
            version: self.version.clone(),
            status: self.status.as_str().to_string(),
            engine: self.engine.clone(),
            path: self.path.clone().ok_or_else(|| {
                TaseError::invalid_argument(format!(
                    "the implementation {} carries no path: it cannot be put on the wire",
                    self.id
                ))
            })?,
            sha256: self.sha256.clone().ok_or_else(|| {
                TaseError::invalid_argument(format!(
                    "the implementation {} carries no digest: it cannot be put on the wire",
                    self.id
                ))
            })?,
            parameters_schema: self.parameters_schema.clone().ok_or_else(|| {
                TaseError::invalid_argument(format!(
                    "the implementation {} carries no schema of its parameters: it cannot be put on the wire",
                    self.id
                ))
            })?,
            tests_manifest: self.tests_manifest.clone().ok_or_else(|| {
                TaseError::invalid_argument(format!(
                    "the implementation {} carries no manifest of its tests: it cannot be put on the wire",
                    self.id
                ))
            })?,
        })
    }

    /// Whether the implementation is written in this language (the language is compared without case).
    pub fn is_in_language(&self, language: &str) -> bool {
        self.language.trim().eq_ignore_ascii_case(language.trim())
    }

    /// Whether the implementation may be instantiated in its language.
    pub fn is_instantiable(&self) -> bool {
        self.status.is_instantiable()
    }
}

/// The text of a field of the manifest, when it is not empty.
fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// The registry of the implementations of the catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImplementationRegistry {
    implementations: Vec<Implementation>,
}

impl ImplementationRegistry {
    /// An empty registry.
    pub fn new() -> ImplementationRegistry {
        ImplementationRegistry::default()
    }

    /// Reads the implementations that the manifests of a catalog hold.
    ///
    /// The catalog of this repository holds none: the registry that comes out is empty.
    pub fn from_registry(registry: &Registry) -> Result<ImplementationRegistry, TaseError> {
        let mut out = ImplementationRegistry::new();
        for entry in registry.entries() {
            for implementation in &entry.implementations {
                out.register(Implementation::from_entry(&entry.id, implementation))?;
            }
        }
        Ok(out)
    }

    /// Registers an implementation.
    ///
    /// An id that is already registered is refused: one id names one implementation.
    pub fn register(&mut self, implementation: Implementation) -> Result<(), TaseError> {
        if implementation.id.trim().is_empty() {
            return Err(TaseError::invalid_argument(
                "the id of an implementation must not be empty".to_string(),
            ));
        }
        if implementation.asset_id.trim().is_empty() {
            return Err(TaseError::invalid_argument(format!(
                "the implementation {} belongs to no asset",
                implementation.id
            )));
        }
        if implementation.language.trim().is_empty() {
            return Err(TaseError::invalid_argument(format!(
                "the implementation {} is written in no language",
                implementation.id
            )));
        }
        if self.get(&implementation.id).is_some() {
            return Err(TaseError::invalid_argument(format!(
                "the implementation {} is already registered",
                implementation.id
            )));
        }
        self.implementations.push(implementation);
        Ok(())
    }

    /// The number of the registered implementations.
    pub fn len(&self) -> usize {
        self.implementations.len()
    }

    /// Whether no implementation is registered.
    pub fn is_empty(&self) -> bool {
        self.implementations.is_empty()
    }

    /// The registered implementations, in the order they were registered.
    pub fn implementations(&self) -> &[Implementation] {
        &self.implementations
    }

    /// The implementation with this id.
    pub fn get(&self, id: &str) -> Option<&Implementation> {
        self.implementations
            .iter()
            .find(|implementation| implementation.id == id)
    }

    /// The implementations of this asset.
    pub fn for_asset(&self, asset_id: &str) -> Vec<&Implementation> {
        self.implementations
            .iter()
            .filter(|implementation| implementation.asset_id == asset_id)
            .collect()
    }

    /// The implementations of this asset that are written in this language.
    pub fn for_asset_and_language(&self, asset_id: &str, language: &str) -> Vec<&Implementation> {
        self.implementations
            .iter()
            .filter(|implementation| {
                implementation.asset_id == asset_id && implementation.is_in_language(language)
            })
            .collect()
    }

    /// The implementations that are written in this language.
    pub fn in_language(&self, language: &str) -> Vec<&Implementation> {
        self.implementations
            .iter()
            .filter(|implementation| implementation.is_in_language(language))
            .collect()
    }

    /// The implementations that are in this state.
    pub fn with_status(&self, status: ImplementationStatus) -> Vec<&Implementation> {
        self.implementations
            .iter()
            .filter(|implementation| implementation.status == status)
            .collect()
    }

    /// The approved implementation of this asset in this language.
    ///
    /// This is the only way an asset may be instantiated (requirements §14.4): an implementation that is approved
    /// for Rust says nothing about TypeScript.
    pub fn approved_for(&self, asset_id: &str, language: &str) -> Option<&Implementation> {
        self.for_asset_and_language(asset_id, language)
            .into_iter()
            .find(|implementation| implementation.status.is_instantiable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The catalog of the repository.
    fn real_registry() -> Registry {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        Registry::load(&dir).expect("the catalog of the repository is valid")
    }

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
    fn the_repository_holds_no_implementation_at_all() {
        let registry = real_registry();
        let implementations = ImplementationRegistry::from_registry(&registry).expect("the registry is read");
        assert!(implementations.is_empty());
        assert_eq!(implementations.len(), 0);
        assert!(implementations.for_asset("algorithm.graph.dijkstra").is_empty());
        assert!(implementations.in_language("rust").is_empty());
        assert!(
            implementations
                .approved_for("algorithm.graph.dijkstra", "rust")
                .is_none()
        );
        assert!(
            implementations
                .with_status(ImplementationStatus::Approved)
                .is_empty()
        );
    }

    #[test]
    fn a_registered_implementation_is_found_by_its_asset_and_its_language() {
        let mut registry = ImplementationRegistry::new();
        registry
            .register(implementation(ImplementationStatus::Approved))
            .expect("the implementation is registered");
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
        let found = registry
            .approved_for("algorithm.graph.dijkstra", "rust")
            .expect("the implementation is there");
        assert_eq!(found.id, "algorithm.graph.dijkstra.rust.binary_heap");
        // the language is compared without case
        assert!(
            registry
                .approved_for("algorithm.graph.dijkstra", "RUST")
                .is_some()
        );
        // another language says nothing
        assert!(
            registry
                .approved_for("algorithm.graph.dijkstra", "typescript")
                .is_none()
        );
        // another asset says nothing
        assert!(registry.approved_for("algorithm.graph.prim", "rust").is_none());
        assert_eq!(registry.for_asset("algorithm.graph.dijkstra").len(), 1);
        assert_eq!(registry.in_language("rust").len(), 1);
        assert_eq!(
            registry.with_status(ImplementationStatus::Approved).len(),
            1
        );
        assert!(registry.with_status(ImplementationStatus::Revoked).is_empty());
    }

    #[test]
    fn an_id_that_is_already_registered_is_refused() {
        let mut registry = ImplementationRegistry::new();
        registry
            .register(implementation(ImplementationStatus::Approved))
            .expect("the implementation is registered");
        let error = registry
            .register(implementation(ImplementationStatus::Tested))
            .expect_err("the id is already registered");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn an_implementation_without_an_id_a_language_or_an_asset_is_refused() {
        let mut registry = ImplementationRegistry::new();
        let mut empty = implementation(ImplementationStatus::Approved);
        empty.id = String::new();
        assert!(registry.register(empty).is_err());

        let mut no_asset = implementation(ImplementationStatus::Approved);
        no_asset.asset_id = String::new();
        assert!(registry.register(no_asset).is_err());

        let mut no_language = implementation(ImplementationStatus::Approved);
        no_language.language = String::new();
        assert!(registry.register(no_language).is_err());

        assert!(registry.is_empty());
    }

    #[test]
    fn the_record_keeps_the_fields_of_the_manifest() {
        let entry = ImplementationEntry {
            implementation_id: "algorithm.graph.dijkstra.rust.binary_heap".to_string(),
            language: "rust".to_string(),
            version: "1.0.0".to_string(),
            status: "APPROVED".to_string(),
            engine: "tase-template".to_string(),
            path: "templates/rust/graph/dijkstra".to_string(),
            sha256: "a".repeat(64),
            parameters_schema: "schemas/dijkstra-params.json".to_string(),
            tests_manifest: "tests/dijkstra.yaml".to_string(),
        };
        let record = Implementation::from_entry("algorithm.graph.dijkstra", &entry);
        assert_eq!(record.id, entry.implementation_id);
        assert_eq!(record.asset_id, "algorithm.graph.dijkstra");
        assert_eq!(record.language, entry.language);
        assert_eq!(record.version, entry.version);
        assert_eq!(record.status, ImplementationStatus::Approved);
        assert_eq!(record.engine, entry.engine);
        assert_eq!(record.path.as_deref(), Some(entry.path.as_str()));
        assert_eq!(record.sha256.as_deref(), Some(entry.sha256.as_str()));
        assert_eq!(
            record.parameters_schema.as_deref(),
            Some(entry.parameters_schema.as_str())
        );
        assert_eq!(
            record.tests_manifest.as_deref(),
            Some(entry.tests_manifest.as_str())
        );
    }

    #[test]
    fn a_field_that_the_manifest_leaves_empty_stays_empty() {
        let entry = ImplementationEntry {
            implementation_id: "algorithm.graph.dijkstra.rust.binary_heap".to_string(),
            language: "rust".to_string(),
            version: "1.0.0".to_string(),
            status: "IMPLEMENTED".to_string(),
            engine: "tase-template".to_string(),
            path: String::new(),
            sha256: String::new(),
            parameters_schema: String::new(),
            tests_manifest: String::new(),
        };
        let record = Implementation::from_entry("algorithm.graph.dijkstra", &entry);
        assert!(record.path.is_none());
        assert!(record.sha256.is_none());
        assert!(record.parameters_schema.is_none());
        assert!(record.tests_manifest.is_none());
        // and the record cannot be put on the wire: the gaps are not filled
        let error = record
            .to_contract()
            .expect_err("the record carries no path");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_wire_shape_is_the_one_of_the_contract() {
        let record = implementation(ImplementationStatus::Approved);
        let wire = record.to_contract().expect("the record is complete");
        assert_eq!(
            wire.implementation_id.as_str(),
            "algorithm.graph.dijkstra.rust.binary_heap"
        );
        assert_eq!(wire.language, "rust");
        assert_eq!(wire.version, "1.0.0");
        assert_eq!(wire.status, "APPROVED");
        assert_eq!(wire.engine, "tase-template");
        assert_eq!(wire.path, "templates/rust/graph/dijkstra");
        assert_eq!(wire.sha256, "a".repeat(64));
        assert_eq!(wire.parameters_schema, "schemas/dijkstra-params.json");
        assert_eq!(wire.tests_manifest, "tests/dijkstra.yaml");
    }
}
