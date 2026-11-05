//! The manifest of an asset of the catalog (design §19.1) and the index of the catalog.
//!
//! The manifest of an asset is the file `assets/<asset_id>/manifest.json` of the catalog. Its shape is the one
//! of design §19.1 and it is parsed field by field: a field that is missing, a field of the wrong type or a
//! field that carries a state the design book does not allow is an [`ErrorCode::InvalidArgument`].
//!
//! The states are the honest ones of requirements §14.4:
//!
//! * a **knowledge card** is `DOCUMENTED` or `REVIEWED`;
//! * a **language implementation** is `IMPLEMENTED`, `TESTED`, `APPROVED`, `DEPRECATED` or `REVOKED`.
//!
//! The two axes are separate: a card that is `REVIEWED` still has no implementation, and an implementation that
//! is `APPROVED` says nothing about the card. An implementation that is not `APPROVED` must carry the digest of
//! its package, and a digest that is there must be a real one: design §19.1 forbids publishing a placeholder
//! digest.

use std::collections::BTreeMap;

use pime_core::json::Json;
use tase_contract::{AssetId, ImplementationId, TaseError};

use crate::digest::hex_of;

/// The states a knowledge card may have (requirements §14.4).
pub const ASSET_STATUSES: [&str; 2] = ["DOCUMENTED", "REVIEWED"];

/// The states a language implementation may have (requirements §14.4).
pub const IMPLEMENTATION_STATUSES: [&str; 5] = [
    "IMPLEMENTED",
    "TESTED",
    "APPROVED",
    "DEPRECATED",
    "REVOKED",
];

/// The fields the manifest of an asset must carry (design §19.1, the same list as the catalog validator).
pub const MANIFEST_FIELDS: [&str; 13] = [
    "schema_version",
    "asset_id",
    "version",
    "kind",
    "name",
    "aliases",
    "category",
    "status",
    "intent",
    "applicability",
    "complexity",
    "implementations",
    "provenance",
];

/// One implementation of an asset (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplementationEntry {
    /// The id of the implementation.
    pub implementation_id: String,
    /// The language of the implementation.
    pub language: String,
    /// The version of the implementation.
    pub version: String,
    /// The state of the approval of the implementation (`APPROVED`, ...).
    pub status: String,
    /// The engine that renders the implementation.
    pub engine: String,
    /// Where the implementation lives.
    pub path: String,
    /// The digest of the package of the implementation.
    pub sha256: String,
    /// Where the schema of the parameters of the implementation lives.
    pub parameters_schema: String,
    /// Where the manifest of the tests of the implementation lives.
    pub tests_manifest: String,
}

impl ImplementationEntry {
    /// Whether the implementation is approved for its language.
    pub fn is_approved(&self) -> bool {
        self.status == "APPROVED"
    }

    /// Whether the implementation is written in this language (the language is compared without case).
    pub fn is_in_language(&self, language: &str) -> bool {
        self.language.trim().eq_ignore_ascii_case(language.trim())
    }
}

/// One asset of the catalog, as the registry holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    /// The id of the asset (`algorithm.graph.kruskal`).
    pub id: String,
    /// The version of the asset.
    pub version: String,
    /// The kind of the asset (`algorithm`, `pattern`, ...).
    pub kind: String,
    /// The category of the asset.
    pub category: String,
    /// The name of the asset in the two languages of the catalog (`zh-CN` and `en-US`).
    pub names: BTreeMap<String, String>,
    /// The other names of the asset (the search keys of requirements §14.3).
    pub aliases: Vec<String>,
    /// The state of the card (`DOCUMENTED` or `REVIEWED`).
    pub status: String,
    /// What the asset is for.
    pub intent: String,
    /// The conditions the asset needs, in the order of the manifest.
    pub constraints: Vec<(String, String)>,
    /// The cases the asset does not solve.
    pub counterexamples: Vec<String>,
    /// The assets that solve those cases instead.
    pub alternatives: Vec<String>,
    /// What the asset costs in time, when the manifest declares it.
    pub complexity_time: Option<String>,
    /// What the asset costs in memory, when the manifest declares it.
    pub complexity_space: Option<String>,
    /// The implementations of the asset.
    pub implementations: Vec<ImplementationEntry>,
    /// The license of the asset, when the manifest declares one.
    pub license: Option<String>,
    /// The references of the asset.
    pub references: Vec<String>,
    /// The content digest of the asset, computed by the loader (it is not stored in the manifest).
    pub digest: String,
    /// Where the knowledge card of the asset lives, relative to the catalog.
    pub card_path: Option<String>,
}

impl AssetEntry {
    /// The name of the asset in Chinese.
    pub fn name_zh(&self) -> Option<&str> {
        self.names.get("zh-CN").map(String::as_str)
    }

    /// The name of the asset in English.
    pub fn name_en(&self) -> Option<&str> {
        self.names.get("en-US").map(String::as_str)
    }

    /// Whether the asset has an implementation in this language (the language is compared without case).
    pub fn has_implementation_in(&self, language: &str) -> bool {
        self.implementations
            .iter()
            .any(|implementation| implementation.is_in_language(language))
    }

    /// The approved implementation of the asset in this language, when there is one.
    ///
    /// This is the only way an asset may be instantiated (requirements §14.4): an implementation that is
    /// approved for one language says nothing about another one.
    pub fn approved_implementation(&self, language: &str) -> Option<&ImplementationEntry> {
        self.implementations
            .iter()
            .find(|implementation| implementation.is_approved() && implementation.is_in_language(language))
    }
}

/// One line of the index of the catalog (`catalog.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRow {
    /// The id of the asset.
    pub id: String,
    /// The kind of the asset.
    pub kind: String,
    /// The category of the asset.
    pub category: String,
    /// The name of the asset in Chinese.
    pub name_zh: String,
    /// The name of the asset in English.
    pub name_en: String,
    /// The other names of the asset.
    pub aliases: Vec<String>,
    /// The state of the card.
    pub status: String,
    /// Where the manifest of the asset lives, relative to the catalog.
    pub manifest: String,
    /// Where the knowledge card of the asset lives, relative to the catalog.
    pub card: Option<String>,
}

/// The field of a JSON object; a missing field is an invalid argument.
fn field<'a>(value: &'a Json, key: &str) -> Result<&'a Json, TaseError> {
    value
        .get(key)
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} is missing")))
}

/// A string field that must not be empty.
fn req_str(value: &Json, key: &str) -> Result<String, TaseError> {
    let text = field(value, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be a string")))?;
    if text.trim().is_empty() {
        return Err(TaseError::invalid_argument(format!(
            "the field {key:?} must not be empty"
        )));
    }
    Ok(text)
}

/// A string field that may be empty (the field must be there).
fn opt_str(value: &Json, key: &str) -> Result<String, TaseError> {
    field(value, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be a string")))
}

/// A list of strings.
fn req_strings(value: &Json, key: &str) -> Result<Vec<String>, TaseError> {
    let items = field(value, key)?
        .as_array()
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be an array")))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| {
                    TaseError::invalid_argument(format!("the field {key:?} must only hold strings"))
                })?,
        );
    }
    Ok(out)
}

/// A table of strings, kept in the order it was written (the order is part of the digest of the manifest).
fn req_table(value: &Json, key: &str) -> Result<Vec<(String, String)>, TaseError> {
    let pairs = field(value, key)?
        .as_object()
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be an object")))?;
    let mut out = Vec::with_capacity(pairs.len());
    for (name, item) in pairs {
        let text = item.as_str().ok_or_else(|| {
            TaseError::invalid_argument(format!("the field {key:?} must only hold strings"))
        })?;
        out.push((name.clone(), text.to_string()));
    }
    Ok(out)
}

/// Parses one implementation of an asset (design §19.1).
pub fn parse_implementation(json: &Json) -> Result<ImplementationEntry, TaseError> {
    let implementation_id = req_str(json, "implementation_id")?;
    // the id ends up in a path and in the audit log: it is checked before it is used
    ImplementationId::parse(&implementation_id)?;
    let language = req_str(json, "language")?;
    let version = req_str(json, "version")?;
    let status = req_str(json, "status")?;
    if !IMPLEMENTATION_STATUSES.contains(&status.as_str()) {
        return Err(TaseError::invalid_argument(format!(
            "the implementation status {status:?} is not one of {}",
            IMPLEMENTATION_STATUSES.join(", ")
        )));
    }
    let engine = req_str(json, "engine")?;
    let path = req_str(json, "path")?;
    let sha256 = opt_str(json, "sha256")?;
    let parameters_schema = req_str(json, "parameters_schema")?;
    let tests_manifest = req_str(json, "tests_manifest")?;

    // design §19.1: an implementation that is not approved must carry the digest of its package, and a digest
    // that is there must be a real one - a placeholder digest is never published.
    if !sha256.trim().is_empty() && hex_of(sha256.trim()).is_none() {
        return Err(TaseError::invalid_argument(format!(
            "the implementation {implementation_id} carries a digest that is not a digest: {sha256:?}"
        )));
    }
    if status != "APPROVED" && sha256.trim().is_empty() {
        return Err(TaseError::invalid_argument(format!(
            "the implementation {implementation_id} is {status} and must carry the digest of its package"
        )));
    }

    Ok(ImplementationEntry {
        implementation_id,
        language,
        version,
        status,
        engine,
        path,
        sha256,
        parameters_schema,
        tests_manifest,
    })
}

/// Parses the manifest of an asset (design §19.1).
///
/// The digest is not filled in here: it is not stored in the manifest (design §3.2 keeps it in the external
/// metadata of the registry), so the loader of the registry computes it.
pub fn parse_manifest(json: &Json) -> Result<AssetEntry, TaseError> {
    let schema_version = req_str(json, "schema_version")?;
    if schema_version != tase_contract::ASSET_SCHEMA_VERSION {
        return Err(TaseError::invalid_argument(format!(
            "the schema_version {schema_version:?} is not {:?}",
            tase_contract::ASSET_SCHEMA_VERSION
        )));
    }

    let id = req_str(json, "asset_id")?;
    // the id names a directory and a file: it is checked before it is used
    AssetId::parse(&id)?;

    let version = req_str(json, "version")?;
    let kind = req_str(json, "kind")?;
    let category = req_str(json, "category")?;
    let status = req_str(json, "status")?;
    if !ASSET_STATUSES.contains(&status.as_str()) {
        return Err(TaseError::invalid_argument(format!(
            "the status {status:?} is not one of {}",
            ASSET_STATUSES.join(", ")
        )));
    }
    let intent = req_str(json, "intent")?;

    let name = field(json, "name")?;
    let mut names = BTreeMap::new();
    for key in ["zh-CN", "en-US"] {
        let value = req_str(name, key).map_err(|error| error.with_details(format!("name.{key}")))?;
        names.insert(key.to_string(), value);
    }

    let aliases = req_strings(json, "aliases")?;
    for alias in &aliases {
        if alias.trim().is_empty() {
            return Err(TaseError::invalid_argument(
                "the field \"aliases\" must not hold an empty name",
            ));
        }
    }

    let applicability = field(json, "applicability")?;
    let constraints = req_table(applicability, "constraints")
        .map_err(|error| error.with_details("applicability.constraints"))?;
    let counterexamples = req_strings(applicability, "counterexamples")
        .map_err(|error| error.with_details("applicability.counterexamples"))?;
    let alternatives = req_strings(applicability, "alternatives")
        .map_err(|error| error.with_details("applicability.alternatives"))?;

    let complexity = field(json, "complexity")?;
    let complexity_time = optional_str(complexity, "time").map_err(|error| error.with_details("complexity"))?;
    let complexity_space =
        optional_str(complexity, "extra_space").map_err(|error| error.with_details("complexity"))?;

    let implementations = field(json, "implementations")?
        .as_array()
        .ok_or_else(|| {
            TaseError::invalid_argument("the field \"implementations\" must be an array")
        })?
        .iter()
        .map(parse_implementation)
        .collect::<Result<Vec<_>, _>>()?;

    let provenance = field(json, "provenance")?;
    let references =
        req_strings(provenance, "references").map_err(|error| error.with_details("provenance"))?;
    let license = optional_str(provenance, "license").map_err(|error| error.with_details("provenance"))?;

    Ok(AssetEntry {
        id,
        version,
        kind,
        category,
        names,
        aliases,
        status,
        intent,
        constraints,
        counterexamples,
        alternatives,
        complexity_time,
        complexity_space,
        implementations,
        license,
        references,
        digest: String::new(),
        card_path: None,
    })
}

/// A string field that may be missing or empty.
fn optional_str(value: &Json, key: &str) -> Result<Option<String>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(text)) if text.trim().is_empty() => Ok(None),
        Some(Json::Str(text)) => Ok(Some(text.clone())),
        Some(_) => Err(TaseError::invalid_argument(format!(
            "the field {key:?} must be a string"
        ))),
    }
}

/// Parses the index of the catalog (`catalog.json`).
///
/// The value may be the whole document (an object with an `assets` array) or the `assets` array itself.
pub fn parse_catalog_index(json: &Json) -> Result<Vec<CatalogRow>, TaseError> {
    let assets = match json {
        Json::Arr(items) => items,
        Json::Obj(_) => field(json, "assets")?
            .as_array()
            .ok_or_else(|| TaseError::invalid_argument("the field \"assets\" must be an array"))?,
        _ => {
            return Err(TaseError::invalid_argument(
                "the catalog must be an object with an \"assets\" array",
            ));
        }
    };

    let mut rows = Vec::with_capacity(assets.len());
    for asset in assets {
        let id = req_str(asset, "id")?;
        AssetId::parse(&id)?;
        let kind = req_str(asset, "kind")?;
        let category = req_str(asset, "category")?;
        let name_zh = req_str(asset, "name_zh")?;
        let name_en = req_str(asset, "name_en")?;
        let aliases = req_strings(asset, "aliases")?;
        let status = req_str(asset, "status")?;
        if !ASSET_STATUSES.contains(&status.as_str()) {
            return Err(TaseError::invalid_argument(format!(
                "the status {status:?} of {id} is not one of {}",
                ASSET_STATUSES.join(", ")
            )));
        }
        let manifest = req_str(asset, "manifest")?;
        let card = match asset.get("card") {
            None | Some(Json::Null) => None,
            Some(Json::Str(text)) if text.trim().is_empty() => None,
            Some(Json::Str(text)) => Some(text.clone()),
            Some(_) => {
                return Err(TaseError::invalid_argument(
                    "the field \"card\" must be a string",
                ));
            }
        };
        rows.push(CatalogRow {
            id,
            kind,
            category,
            name_zh,
            name_en,
            aliases,
            status,
            manifest,
            card,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json::{Limits, parse};

    /// The manifest of an asset, as the catalog writes it.
    fn manifest_json() -> Json {
        parse(
            r#"{
              "schema_version": "tase.asset.v1",
              "asset_id": "algorithm.graph.kruskal",
              "version": "1.0.0",
              "kind": "algorithm",
              "name": {"zh-CN": "Kruskal 最小生成树", "en-US": "Kruskal's Minimum Spanning Tree"},
              "aliases": ["Kruskal", "最小生成树", "MST"],
              "category": "graph",
              "status": "DOCUMENTED",
              "intent": "按边权升序贪心加边。",
              "applicability": {
                "constraints": {"graph": "undirected_connected", "output": "spanning_tree_edges"},
                "counterexamples": ["有向图——最小生成树无定义"],
                "alternatives": ["algorithm.graph.prim"]
              },
              "complexity": {"time": "O(E log E)", "extra_space": "O(V + E)"},
              "implementations": [],
              "provenance": {"references": ["https://en.wikipedia.org/wiki/Kruskal%27s_algorithm"], "license": "MIT"}
            }"#,
            Limits::DEFAULT,
        )
        .expect("valid JSON")
    }

    #[test]
    fn a_manifest_is_parsed_field_by_field() {
        let entry = parse_manifest(&manifest_json()).expect("the manifest is valid");
        assert_eq!(entry.id, "algorithm.graph.kruskal");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.kind, "algorithm");
        assert_eq!(entry.category, "graph");
        assert_eq!(entry.status, "DOCUMENTED");
        assert_eq!(entry.name_zh(), Some("Kruskal 最小生成树"));
        assert_eq!(entry.name_en(), Some("Kruskal's Minimum Spanning Tree"));
        assert_eq!(entry.aliases, ["Kruskal", "最小生成树", "MST"]);
        assert_eq!(
            entry.constraints,
            [
                ("graph".to_string(), "undirected_connected".to_string()),
                ("output".to_string(), "spanning_tree_edges".to_string())
            ]
        );
        assert_eq!(entry.counterexamples.len(), 1);
        assert_eq!(entry.alternatives, ["algorithm.graph.prim"]);
        assert_eq!(entry.complexity_time.as_deref(), Some("O(E log E)"));
        assert_eq!(entry.complexity_space.as_deref(), Some("O(V + E)"));
        assert_eq!(entry.license.as_deref(), Some("MIT"));
        assert_eq!(entry.references.len(), 1);
        assert!(entry.implementations.is_empty());
        // the digest is not stored in the manifest: the loader computes it
        assert!(entry.digest.is_empty());
        assert!(entry.card_path.is_none());
    }

    #[test]
    fn a_field_that_is_missing_is_refused() {
        for field in MANIFEST_FIELDS {
            let mut value = manifest_json();
            if let Json::Obj(pairs) = &mut value {
                pairs.retain(|(name, _)| name != field);
            }
            let error = parse_manifest(&value).expect_err("the manifest is incomplete");
            assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
            assert!(
                error.message.contains(field),
                "the error must name the field {field}: {error}"
            );
        }
    }

    #[test]
    fn a_name_without_one_of_the_two_languages_is_refused() {
        let mut value = manifest_json();
        if let Json::Obj(pairs) = &mut value
            && let Some((_, name)) = pairs.iter_mut().find(|(name, _)| name == "name")
            && let Json::Obj(parts) = name
        {
            parts.retain(|(key, _)| key != "en-US");
        }
        let error = parse_manifest(&value).expect_err("the name is incomplete");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert_eq!(error.details.as_deref(), Some("name.en-US"));
    }

    #[test]
    fn a_state_that_the_design_book_does_not_know_is_refused() {
        let mut value = manifest_json();
        if let Json::Obj(pairs) = &mut value {
            for (name, item) in pairs.iter_mut() {
                if name == "status" {
                    *item = Json::str("APPROVED_CARD");
                }
            }
        }
        let error = parse_manifest(&value).expect_err("the state is not allowed");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("APPROVED_CARD"), "{error}");
    }

    #[test]
    fn an_implementation_state_that_the_design_book_does_not_know_is_refused() {
        let mut value = manifest_json();
        if let Json::Obj(pairs) = &mut value {
            for (name, item) in pairs.iter_mut() {
                if name == "implementations" {
                    *item = Json::Arr(vec![implementation_json("DONE")]);
                }
            }
        }
        let error = parse_manifest(&value).expect_err("the state is not allowed");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("DONE"), "{error}");
    }

    /// One implementation of an asset.
    fn implementation_json(status: &str) -> Json {
        parse(
            &format!(
                r#"{{
                  "implementation_id": "algorithm.graph.kruskal.rust.binary_heap",
                  "language": "rust",
                  "version": "1.0.0",
                  "status": "{status}",
                  "engine": "tase-template",
                  "path": "templates/rust/graph/kruskal",
                  "sha256": "{}",
                  "parameters_schema": "schemas/kruskal-params.json",
                  "tests_manifest": "tests/kruskal.yaml"
                }}"#,
                "a".repeat(64)
            ),
            Limits::DEFAULT,
        )
        .expect("valid JSON")
    }

    #[test]
    fn an_implementation_that_is_not_approved_must_carry_a_digest() {
        for status in ["IMPLEMENTED", "TESTED", "DEPRECATED", "REVOKED"] {
            let mut implementation = implementation_json(status);
            if let Json::Obj(pairs) = &mut implementation {
                for (name, item) in pairs.iter_mut() {
                    if name == "sha256" {
                        *item = Json::str("");
                    }
                }
            }
            let error = parse_implementation(&implementation).expect_err("the digest is missing");
            assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
            assert!(error.message.contains("digest"), "{error}");
        }
    }

    #[test]
    fn a_placeholder_digest_is_refused() {
        let mut implementation = implementation_json("IMPLEMENTED");
        if let Json::Obj(pairs) = &mut implementation {
            for (name, item) in pairs.iter_mut() {
                if name == "sha256" {
                    *item = Json::str("<release-sha256-placeholder>");
                }
            }
        }
        let error = parse_implementation(&implementation).expect_err("the digest is a placeholder");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("placeholder"), "{error}");
    }

    #[test]
    fn an_approved_implementation_may_carry_an_empty_digest() {
        let mut implementation = implementation_json("APPROVED");
        if let Json::Obj(pairs) = &mut implementation {
            for (name, item) in pairs.iter_mut() {
                if name == "sha256" {
                    *item = Json::str("");
                }
            }
        }
        let entry = parse_implementation(&implementation).expect("the implementation is valid");
        assert!(entry.is_approved());
        assert!(entry.is_in_language("RUST"));
        assert!(!entry.is_in_language("typescript"));
    }

    #[test]
    fn the_index_of_the_catalog_is_parsed() {
        let index = parse(
            r#"{
              "schema_version": "tase.asset.v1",
              "catalog_version": "1.0.0",
              "count": 1,
              "assets": [
                {
                  "id": "algorithm.graph.kruskal",
                  "kind": "algorithm",
                  "category": "graph",
                  "name_zh": "Kruskal 最小生成树",
                  "name_en": "Kruskal's Minimum Spanning Tree",
                  "aliases": ["Kruskal", "MST"],
                  "status": "DOCUMENTED",
                  "manifest": "assets/algorithm.graph.kruskal/manifest.json",
                  "card": "assets/algorithm.graph.kruskal/card.md"
                }
              ]
            }"#,
            Limits::DEFAULT,
        )
        .expect("valid JSON");
        let rows = parse_catalog_index(&index).expect("the index is valid");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "algorithm.graph.kruskal");
        assert_eq!(rows[0].kind, "algorithm");
        assert_eq!(rows[0].category, "graph");
        assert_eq!(rows[0].name_zh, "Kruskal 最小生成树");
        assert_eq!(rows[0].name_en, "Kruskal's Minimum Spanning Tree");
        assert_eq!(rows[0].aliases, ["Kruskal", "MST"]);
        assert_eq!(rows[0].status, "DOCUMENTED");
        assert_eq!(
            rows[0].manifest,
            "assets/algorithm.graph.kruskal/manifest.json"
        );
        assert_eq!(
            rows[0].card.as_deref(),
            Some("assets/algorithm.graph.kruskal/card.md")
        );
    }

    #[test]
    fn an_alias_that_is_empty_is_refused() {
        let mut value = manifest_json();
        if let Json::Obj(pairs) = &mut value {
            for (name, item) in pairs.iter_mut() {
                if name == "aliases" {
                    *item = Json::Arr(vec![Json::str("Kruskal"), Json::str("")]);
                }
            }
        }
        let error = parse_manifest(&value).expect_err("the alias is empty");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("aliases"), "{error}");
    }

    #[test]
    fn an_alias_that_is_not_a_string_is_refused() {
        let mut value = manifest_json();
        if let Json::Obj(pairs) = &mut value {
            for (name, item) in pairs.iter_mut() {
                if name == "aliases" {
                    *item = Json::Arr(vec![Json::int(7)]);
                }
            }
        }
        let error = parse_manifest(&value).expect_err("the alias is not a string");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("aliases"), "{error}");
    }

    #[test]
    fn an_index_line_without_an_id_is_refused() {
        let index = parse(r#"{"assets": [{"kind": "algorithm"}]}"#, Limits::DEFAULT).expect("valid JSON");
        let error = parse_catalog_index(&index).expect_err("the line is incomplete");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("id"), "{error}");
    }
}
