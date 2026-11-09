//! The loader of the registry: it reads a catalog from the disk.
//!
//! A catalog is a directory that holds:
//!
//! ```text
//! <catalog>/
//! ├── catalog.json                    the index of the catalog
//! └── assets/<asset_id>/
//!     ├── manifest.json                the manifest of the asset (design §19.1)
//!     └── card.md                      the knowledge card of the asset
//! ```
//!
//! The loader does not trust the index: it reads every manifest the index points to, it compares the id of the
//! manifest with the id of the directory and with the id of the line of the index, and it compares the number
//! of the asset directories with the number of the lines of the index. A catalog whose index and whose assets
//! do not agree is refused with [`ErrorCode::InvalidArgument`] - a catalog that lies is worse than no catalog.
//!
//! The loader also checks the two rules of requirements §14.3 that belong to the registry and not to one
//! manifest: an alias may not be the id of another asset (an alias may not inflate the number of assets) and an
//! alternative must be an asset of the catalog.
//!
//! The digest of an asset is computed over the bytes of its manifest and of its knowledge card, framed with a
//! zero byte between the two:
//!
//! ```text
//! body = manifest_bytes || 0x00 || card_bytes
//! digest = asset_digest(asset_id, version, body)
//! ```
//!
//! The digest is not stored in the manifest (design §3.2), so the loader writes it into the entry it returns.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use pime_core::json::{Json, Limits, parse};
use tase_contract::TaseError;

use crate::digest::asset_digest;
use crate::manifest::{AssetEntry, CatalogRow, parse_catalog_index, parse_manifest};

/// The name of the index of a catalog.
pub const CATALOG_FILE: &str = "catalog.json";

/// The name of the directory that holds the assets of a catalog.
pub const ASSETS_DIR: &str = "assets";

/// The name of the manifest of an asset.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The name of the knowledge card of an asset.
pub const CARD_FILE: &str = "card.md";

/// The catalog of the registry: the assets it holds and the way to find them.
#[derive(Clone, Debug)]
pub struct Registry {
    entries: Vec<AssetEntry>,
    by_id: BTreeMap<String, usize>,
    dir: PathBuf,
    catalog_version: String,
}

impl Registry {
    /// Loads a catalog from the disk.
    ///
    /// The index is read first, then every manifest the index points to. The loader fails when the index and
    /// the assets do not agree, when a manifest carries another id than its directory or than the index, or
    /// when a manifest is not the shape of design §19.1.
    pub fn load(dir: &Path) -> Result<Registry, TaseError> {
        let catalog_path = dir.join(CATALOG_FILE);
        let bytes = read_file(&catalog_path)?;
        let json = parse_json(&bytes, &catalog_path)?;
        let rows = parse_catalog_index(&json)?;

        // the count of the index must be the number of its lines
        if let Some(count) = json.get("count").and_then(Json::as_i64)
            && count != rows.len() as i64
        {
            return Err(TaseError::invalid_argument(format!(
                "the catalog says {count} assets but its index holds {} lines",
                rows.len()
            )));
        }

        // the number of the asset directories must be the number of the lines of the index
        let directories = asset_directories(dir)?;
        if directories.len() != rows.len() {
            return Err(TaseError::invalid_argument(format!(
                "the catalog holds {} asset directories but its index holds {} lines",
                directories.len(),
                rows.len()
            )));
        }

        let mut entries: Vec<AssetEntry> = Vec::with_capacity(rows.len());
        for row in &rows {
            entries.push(load_asset(dir, row)?);
        }

        let mut by_id = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            if by_id.insert(entry.id.clone(), index).is_some() {
                return Err(TaseError::invalid_argument(format!(
                    "the catalog holds the asset {} twice",
                    entry.id
                )));
            }
        }

        check_aliases(&entries, &by_id)?;
        check_alternatives(&entries, &by_id)?;

        let catalog_version = json
            .get("catalog_version")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(Registry {
            entries,
            by_id,
            dir: dir.to_path_buf(),
            catalog_version,
        })
    }

    /// The asset with this id.
    pub fn get(&self, id: &str) -> Option<&AssetEntry> {
        self.by_id.get(id).map(|index| &self.entries[*index])
    }

    /// The number of the assets of the catalog.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The assets of the catalog, in the order of the index.
    pub fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }

    /// The directory the catalog was loaded from.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The version of the catalog (`catalog_version` of `catalog.json`).
    pub fn catalog_version(&self) -> &str {
        &self.catalog_version
    }
}

/// Reads the file; a file that is not there is `NOT_FOUND`, a file that cannot be read is an internal failure.
fn read_file(path: &Path) -> Result<Vec<u8>, TaseError> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(TaseError::not_found(format!(
            "the file {} does not exist",
            path.display()
        ))),
        Err(error) => Err(TaseError::internal(format!(
            "the file {} cannot be read: {error}",
            path.display()
        ))),
    }
}

/// Reads the file as the strict JSON of the core.
fn parse_json(bytes: &[u8], path: &Path) -> Result<Json, TaseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        TaseError::invalid_argument(format!("the file {} is not UTF-8", path.display()))
    })?;
    parse(text, Limits::DEFAULT).map_err(|error| {
        TaseError::invalid_argument(format!(
            "the file {} is not the JSON the registry expects: {error}",
            path.display()
        ))
    })
}

/// The directories of the assets of the catalog, sorted by name.
fn asset_directories(dir: &Path) -> Result<Vec<String>, TaseError> {
    let assets_dir = dir.join(ASSETS_DIR);
    let entries = fs::read_dir(&assets_dir).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TaseError::not_found(format!(
                "the directory {} does not exist",
                assets_dir.display()
            ))
        } else {
            TaseError::internal(format!(
                "the directory {} cannot be read: {error}",
                assets_dir.display()
            ))
        }
    })?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            TaseError::internal(format!(
                "the directory {} cannot be read: {error}",
                assets_dir.display()
            ))
        })?;
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Checks that a path of the index is `assets/<asset_id>/<name>`: the index may not point out of the catalog.
fn check_asset_path(text: &str, asset_id: &str, name: &str) -> Result<(), TaseError> {
    let path = Path::new(text);
    let parts: Vec<Component<'_>> = path.components().collect();
    let is_shape = parts.len() == 3
        && matches!(parts[0], Component::Normal(value) if value == ASSETS_DIR)
        && matches!(parts[1], Component::Normal(value) if value == asset_id)
        && matches!(parts[2], Component::Normal(value) if value == name);
    if !is_shape {
        return Err(TaseError::invalid_argument(format!(
            "the path {text:?} of the asset {asset_id} must be {ASSETS_DIR}/{asset_id}/{name}"
        )));
    }
    Ok(())
}

/// Loads one asset: its manifest, its knowledge card and its digest.
fn load_asset(dir: &Path, row: &CatalogRow) -> Result<AssetEntry, TaseError> {
    check_asset_path(&row.manifest, &row.id, MANIFEST_FILE)?;
    if let Some(card) = &row.card {
        check_asset_path(card, &row.id, CARD_FILE)?;
    }

    let manifest_path = dir.join(ASSETS_DIR).join(&row.id).join(MANIFEST_FILE);
    let manifest_bytes = read_file(&manifest_path)?;
    let manifest_json = parse_json(&manifest_bytes, &manifest_path)?;
    let mut entry = parse_manifest(&manifest_json)?;

    // the id of the manifest must be the id of the directory and the id of the line of the index
    if entry.id != row.id {
        return Err(TaseError::invalid_argument(format!(
            "the manifest of {} carries the id {}",
            row.id, entry.id
        )));
    }
    // the manifest and the index must tell the same story
    if entry.kind != row.kind {
        return Err(TaseError::invalid_argument(format!(
            "the kind of {} is {} in the manifest and {} in the index",
            row.id, entry.kind, row.kind
        )));
    }
    if entry.category != row.category {
        return Err(TaseError::invalid_argument(format!(
            "the category of {} is {} in the manifest and {} in the index",
            row.id, entry.category, row.category
        )));
    }
    if entry.status != row.status {
        return Err(TaseError::invalid_argument(format!(
            "the status of {} is {} in the manifest and {} in the index",
            row.id, entry.status, row.status
        )));
    }
    if entry.name_zh() != Some(row.name_zh.as_str()) {
        return Err(TaseError::invalid_argument(format!(
            "the Chinese name of {} differs between the manifest and the index",
            row.id
        )));
    }
    if entry.name_en() != Some(row.name_en.as_str()) {
        return Err(TaseError::invalid_argument(format!(
            "the English name of {} differs between the manifest and the index",
            row.id
        )));
    }

    // the knowledge card is part of the content of the asset
    let mut body = manifest_bytes;
    body.push(0);
    if let Some(card) = &row.card {
        let card_path = dir.join(ASSETS_DIR).join(&row.id).join(CARD_FILE);
        let card_bytes = read_file(&card_path)?;
        body.extend_from_slice(&card_bytes);
        entry.card_path = Some(card.clone());
    }

    entry.digest = asset_digest(&entry.id, &entry.version, &body);
    Ok(entry)
}

/// An alias may not be the id of another asset and may not point at two assets (requirements §14.3).
fn check_aliases(entries: &[AssetEntry], by_id: &BTreeMap<String, usize>) -> Result<(), TaseError> {
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        for alias in &entry.aliases {
            if by_id.contains_key(alias) && alias != &entry.id {
                return Err(TaseError::invalid_argument(format!(
                    "the alias {alias:?} of {} is the id of another asset: an alias may not inflate the number of assets",
                    entry.id
                )));
            }
            let key = alias.to_lowercase();
            match owners.get(&key) {
                Some(owner) if owner != &entry.id => {
                    return Err(TaseError::invalid_argument(format!(
                        "the alias {alias:?} points at {} and at {}: one alias points at one asset",
                        owner, entry.id
                    )));
                }
                Some(_) => {}
                None => {
                    owners.insert(key, entry.id.clone());
                }
            }
        }
    }
    Ok(())
}

/// An alternative must be an asset of the catalog.
fn check_alternatives(entries: &[AssetEntry], by_id: &BTreeMap<String, usize>) -> Result<(), TaseError> {
    for entry in entries {
        for alternative in &entry.alternatives {
            if !by_id.contains_key(alternative) {
                return Err(TaseError::invalid_argument(format!(
                    "the asset {} names the alternative {alternative:?}, which is not in the catalog",
                    entry.id
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::ErrorCode;

    /// The catalog of the repository: 93 knowledge cards and no implementation.
    fn real_catalog() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog")
    }

    /// A catalog with the given assets, written into a fresh directory.
    fn write_catalog(name: &str, assets: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tase-registry-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("the directory is created");
        for (id, body) in assets {
            let asset_dir = dir.join(ASSETS_DIR).join(id);
            fs::create_dir_all(&asset_dir).expect("the asset directory is created");
            fs::write(asset_dir.join(MANIFEST_FILE), body).expect("the manifest is written");
            fs::write(asset_dir.join(CARD_FILE), "# card\n").expect("the card is written");
        }
        dir
    }

    /// The index of a catalog with the given assets.
    fn index_json(assets: &[&str]) -> String {
        let lines: Vec<String> = assets
            .iter()
            .map(|id| {
                format!(
                    r#"{{"id":"{id}","kind":"algorithm","category":"graph","name_zh":"名称","name_en":"Name","aliases":[],"status":"DOCUMENTED","manifest":"assets/{id}/manifest.json","card":"assets/{id}/card.md"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"schema_version":"tase.asset.v1","catalog_version":"1.0.0","count":{},"assets":[{}]}}"#,
            assets.len(),
            lines.join(",")
        )
    }

    /// A manifest of an asset.
    fn manifest_of(id: &str) -> String {
        format!(
            r#"{{
              "schema_version": "tase.asset.v1",
              "asset_id": "{id}",
              "version": "1.0.0",
              "kind": "algorithm",
              "name": {{"zh-CN": "名称", "en-US": "Name"}},
              "aliases": [],
              "category": "graph",
              "status": "DOCUMENTED",
              "intent": "做什么。",
              "applicability": {{"constraints": {{}}, "counterexamples": ["反例"], "alternatives": []}},
              "complexity": {{"time": "O(n)", "extra_space": "O(1)"}},
              "implementations": [],
              "provenance": {{"references": [], "license": "MIT"}}
            }}"#
        )
    }

    #[test]
    fn the_catalog_of_the_repository_is_loaded() {
        let registry = Registry::load(&real_catalog()).expect("the catalog of the repository is valid");
        assert_eq!(registry.len(), 93);
        assert!(!registry.is_empty());
        assert_eq!(registry.catalog_version(), "1.0.0");
        // Every asset is a knowledge card. The fifty algorithms of the catalogue carry an implementation (the
        // crate `tase-impls`), the forty-three patterns do not: a pattern is a shape, it is not one algorithm.
        let mut with_implementation = 0;
        for entry in registry.entries() {
            assert_eq!(entry.status, "DOCUMENTED", "{}", entry.id);
            assert!(!entry.digest.is_empty(), "{} has no digest", entry.id);
            assert!(entry.digest.starts_with("sha256:"), "{}", entry.id);
            assert_eq!(entry.digest.len(), "sha256:".len() + 64, "{}", entry.id);
            assert!(entry.card_path.is_some(), "{} has no card", entry.id);
            assert_eq!(entry.license.as_deref(), Some("MIT"), "{}", entry.id);
            if entry.id.starts_with("pattern.") {
                assert!(
                    entry.implementations.is_empty(),
                    "{} is a pattern and has no implementation",
                    entry.id
                );
                continue;
            }
            assert_eq!(
                entry.implementations.len(),
                1,
                "{} carries exactly one implementation",
                entry.id
            );
            let implementation = &entry.implementations[0];
            assert_eq!(implementation.language, "rust", "{}", entry.id);
            assert_eq!(implementation.status, "TESTED", "{}", entry.id);
            assert_eq!(implementation.engine, "rust", "{}", entry.id);
            assert!(
                implementation.path.starts_with("01_Source/crates/tase-impls/"),
                "{} names the file of the implementation: {}",
                entry.id,
                implementation.path
            );
            assert!(
                implementation.sha256.len() == 64
                    && implementation
                        .sha256
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{} carries the real digest of its file",
                entry.id
            );
            assert!(
                implementation.tests_manifest.starts_with("01_Source/assets/tase-catalog/implementations/"),
                "{} names the tests that stand behind it",
                entry.id
            );
            with_implementation += 1;
        }
        assert_eq!(with_implementation, 50, "the fifty algorithms of the catalogue");
        // the first asset of the index is the first asset of the registry
        assert_eq!(
            registry.entries()[0].id,
            "pattern.gof.creational.abstract_factory"
        );
        // an asset is found by its id
        let kruskal = registry.get("algorithm.graph.kruskal").expect("the asset is there");
        assert_eq!(kruskal.category, "graph");
        assert_eq!(kruskal.name_zh(), Some("Kruskal 最小生成树"));
        assert!(registry.get("algorithm.graph.nothing").is_none());
    }

    #[test]
    fn the_digest_of_an_asset_is_the_digest_of_its_content() {
        let registry = Registry::load(&real_catalog()).expect("the catalog of the repository is valid");
        let entry = registry.get("algorithm.graph.kruskal").expect("the asset is there");
        let manifest = fs::read(
            real_catalog()
                .join(ASSETS_DIR)
                .join(&entry.id)
                .join(MANIFEST_FILE),
        )
        .expect("the manifest is readable");
        let card = fs::read(
            real_catalog()
                .join(ASSETS_DIR)
                .join(&entry.id)
                .join(CARD_FILE),
        )
        .expect("the card is readable");
        let mut body = manifest;
        body.push(0);
        body.extend_from_slice(&card);
        assert_eq!(entry.digest, asset_digest(&entry.id, &entry.version, &body));
    }

    #[test]
    fn an_index_that_is_smaller_than_the_assets_is_refused() {
        let id = "algorithm.graph.kruskal";
        let dir = write_catalog("short-index", &[(id, &manifest_of(id))]);
        // the index holds one line, the catalog holds one directory: it is cut down to zero lines
        fs::write(dir.join(CATALOG_FILE), index_json(&[])).expect("the index is written");
        let error = Registry::load(&dir).expect_err("the index and the assets do not agree");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("index"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_index_that_is_bigger_than_the_assets_is_refused() {
        let id = "algorithm.graph.kruskal";
        let dir = write_catalog("long-index", &[(id, &manifest_of(id))]);
        fs::write(dir.join(CATALOG_FILE), index_json(&[id, "algorithm.graph.prim"]))
            .expect("the index is written");
        let error = Registry::load(&dir).expect_err("the index and the assets do not agree");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manifest_with_another_id_than_its_directory_is_refused() {
        let id = "algorithm.graph.kruskal";
        let dir = write_catalog("wrong-id", &[(id, &manifest_of("algorithm.graph.prim"))]);
        fs::write(dir.join(CATALOG_FILE), index_json(&[id])).expect("the index is written");
        let error = Registry::load(&dir).expect_err("the manifest carries another id");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("algorithm.graph.prim"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_catalog_that_is_not_there_is_not_found() {
        let error = Registry::load(Path::new("does/not/exist")).expect_err("there is no catalog");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn an_alias_may_not_be_the_id_of_another_asset() {
        let first = "algorithm.graph.kruskal";
        let second = "algorithm.graph.prim";
        let dir = write_catalog(
            "alias",
            &[
                (first, &manifest_of(first)),
                (second, &manifest_of(second)),
            ],
        );
        // the manifest of the second asset claims the id of the first one as an alias
        let body = manifest_of(second).replace(
            r#""aliases": []"#,
            r#""aliases": ["algorithm.graph.kruskal"]"#,
        );
        fs::write(dir.join(ASSETS_DIR).join(second).join(MANIFEST_FILE), body)
            .expect("the manifest is written");
        fs::write(dir.join(CATALOG_FILE), index_json(&[first, second])).expect("the index is written");
        let error = Registry::load(&dir).expect_err("the alias is the id of another asset");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("alias"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }
}
