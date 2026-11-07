//! The index of the catalog: the way to find an asset without touching the disk again (detailed design §19.3,
//! basic design §17.3).
//!
//! The index is built **once**, when the catalog is loaded, out of the entries the registry has already read. A
//! query never goes back to the disk: the index holds the views, the layers, the categories, the tags, the aliases
//! and the languages of every asset, and every answer is a lookup in a map.
//!
//! # The alias resolves to the canonical id
//!
//! An alias is a search key (requirements §14.3): `MST` is the key of `algorithm.graph.kruskal`, and it is **not**
//! an asset of its own. [`CatalogIndex::resolve_alias`] therefore answers with the canonical id of the asset, and
//! the number of the assets of the catalog stays the number of the manifests.
//!
//! # The language rule
//!
//! The rule is the one of the registry (requirements §14.4):
//!
//! * an asset with **no** implementation at all is language neutral: it matches every language, because a knowledge
//!   card is written for every language;
//! * an asset that has implementations but not one in the requested language does **not** match: an implementation
//!   that is approved for Rust says nothing about TypeScript.

use std::collections::BTreeMap;

use tase_contract::TaseError;
use tase_registry::Registry;

use crate::assets::{AssetView, Layer};

/// The query of the index: every filter that is given must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogQuery {
    /// Only the assets whose name, alias, id or intent holds this text.
    pub text: Option<String>,
    /// Only the assets of this layer.
    pub layer: Option<Layer>,
    /// Only the assets of this category.
    pub category: Option<String>,
    /// Only the assets that carry this tag.
    pub tag: Option<String>,
    /// Only the assets that answer for this language.
    pub language: Option<String>,
}

/// The index of the catalog.
#[derive(Clone, Debug)]
pub struct CatalogIndex {
    views: Vec<AssetView>,
    by_id: BTreeMap<String, usize>,
    by_alias: BTreeMap<String, usize>,
    by_name: BTreeMap<String, usize>,
    by_layer: BTreeMap<Layer, Vec<usize>>,
    by_category: BTreeMap<String, Vec<usize>>,
    by_tag: BTreeMap<String, Vec<usize>>,
    by_language: BTreeMap<String, Vec<usize>>,
    by_alternative: BTreeMap<String, Vec<usize>>,
}

impl CatalogIndex {
    /// Builds the index out of the entries of the registry.
    ///
    /// The index is a pure function of the registry: the same registry gives the same index, every time.
    pub fn build(registry: &Registry) -> Result<CatalogIndex, TaseError> {
        let mut views: Vec<AssetView> = Vec::with_capacity(registry.len());
        for entry in registry.entries() {
            views.push(AssetView::new(entry.clone())?);
        }

        let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_alias: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_layer: BTreeMap<Layer, Vec<usize>> = BTreeMap::new();
        let mut by_category: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_tag: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_language: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_alternative: BTreeMap<String, Vec<usize>> = BTreeMap::new();

        for (index, view) in views.iter().enumerate() {
            by_id.insert(view.entry.id.clone(), index);

            for alias in &view.aliases {
                let key = alias.to_lowercase();
                by_alias.entry(key).or_insert(index);
            }
            for name in view.entry.names.values() {
                let key = name.to_lowercase();
                by_name.entry(key).or_insert(index);
            }

            by_layer.entry(view.layer).or_default().push(index);

            let category = view.entry.category.to_lowercase();
            by_category.entry(category).or_default().push(index);

            for tag in &view.tags {
                by_tag.entry(tag.clone()).or_default().push(index);
            }

            for implementation in &view.entry.implementations {
                let language = implementation.language.to_lowercase();
                let bucket = by_language.entry(language).or_default();
                if !bucket.contains(&index) {
                    bucket.push(index);
                }
            }

            for alternative in &view.entry.alternatives {
                by_alternative
                    .entry(alternative.to_lowercase())
                    .or_default()
                    .push(index);
            }
        }

        Ok(CatalogIndex {
            views,
            by_id,
            by_alias,
            by_name,
            by_layer,
            by_category,
            by_tag,
            by_language,
            by_alternative,
        })
    }

    /// The number of the assets of the catalog.
    pub fn len(&self) -> usize {
        self.views.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// The views of the catalog, in the order of the index of the catalog.
    pub fn views(&self) -> &[AssetView] {
        &self.views
    }

    /// The asset with this id.
    pub fn get(&self, id: &str) -> Option<&AssetView> {
        self.by_id.get(id).map(|index| &self.views[*index])
    }

    /// The canonical id of an alias, of a name or of an id.
    ///
    /// The alias is resolved first, then the name (in Chinese or in English), then the id itself. An unknown name
    /// is `None`: the index never invents an asset.
    pub fn resolve_alias(&self, name: &str) -> Option<&str> {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return None;
        }
        if let Some(index) = self.by_alias.get(&key) {
            return Some(self.views[*index].id());
        }
        if let Some(index) = self.by_name.get(&key) {
            return Some(self.views[*index].id());
        }
        if let Some(index) = self.by_id.get(&key) {
            return Some(self.views[*index].id());
        }
        None
    }

    /// The asset that owns this alias.
    pub fn by_alias(&self, alias: &str) -> Option<&AssetView> {
        let key = alias.trim().to_lowercase();
        self.by_alias.get(&key).map(|index| &self.views[*index])
    }

    /// The asset that carries this name (in Chinese or in English).
    pub fn by_name(&self, name: &str) -> Option<&AssetView> {
        let key = name.trim().to_lowercase();
        self.by_name.get(&key).map(|index| &self.views[*index])
    }

    /// The assets of this layer.
    pub fn in_layer(&self, layer: Layer) -> Vec<&AssetView> {
        self.by_layer
            .get(&layer)
            .map(|indexes| indexes.iter().map(|index| &self.views[*index]).collect())
            .unwrap_or_default()
    }

    /// The assets of this category.
    pub fn in_category(&self, category: &str) -> Vec<&AssetView> {
        let key = category.trim().to_lowercase();
        self.by_category
            .get(&key)
            .map(|indexes| indexes.iter().map(|index| &self.views[*index]).collect())
            .unwrap_or_default()
    }

    /// The assets that carry this tag.
    pub fn with_tag(&self, tag: &str) -> Vec<&AssetView> {
        let key = tag.trim().to_lowercase();
        self.by_tag
            .get(&key)
            .map(|indexes| indexes.iter().map(|index| &self.views[*index]).collect())
            .unwrap_or_default()
    }

    /// The assets that answer for this language.
    ///
    /// An asset with no implementation at all is language neutral and matches; an asset that has implementations
    /// but not one in this language does not match.
    pub fn in_language(&self, language: &str) -> Vec<&AssetView> {
        let key = language.trim().to_lowercase();
        let mut out: Vec<&AssetView> = Vec::new();
        for view in &self.views {
            if view.entry.implementations.is_empty() {
                out.push(view);
                continue;
            }
            let answers = view
                .entry
                .implementations
                .iter()
                .any(|implementation| implementation.language.trim().eq_ignore_ascii_case(&key));
            if answers {
                out.push(view);
            }
        }
        out
    }

    /// The languages the catalog holds an implementation for, sorted.
    ///
    /// The catalog of this repository holds none: every asset is a knowledge card.
    pub fn languages(&self) -> Vec<&str> {
        self.by_language.keys().map(String::as_str).collect()
    }

    /// The assets that this asset names as its alternatives.
    pub fn alternatives_of(&self, asset_id: &str) -> Vec<&AssetView> {
        let Some(view) = self.get(asset_id) else {
            return Vec::new();
        };
        view.entry
            .alternatives
            .iter()
            .filter_map(|alternative| self.get(alternative))
            .collect()
    }

    /// The assets that name this asset as one of their alternatives.
    pub fn offering(&self, asset_id: &str) -> Vec<&AssetView> {
        self.by_alternative
            .get(&asset_id.trim().to_lowercase())
            .map(|indexes| indexes.iter().map(|index| &self.views[*index]).collect())
            .unwrap_or_default()
    }

    /// The assets that answer the query.
    ///
    /// The filters are combined: an asset must answer every filter that is given. The order of the answer is the
    /// order of the index of the catalog.
    pub fn search(&self, query: &CatalogQuery) -> Vec<&AssetView> {
        self.views
            .iter()
            .filter(|view| self.matches(view, query))
            .collect()
    }

    /// Whether the asset answers the query.
    fn matches(&self, view: &AssetView, query: &CatalogQuery) -> bool {
        if let Some(layer) = query.layer
            && view.layer != layer
        {
            return false;
        }
        if let Some(category) = &query.category {
            let category = category.trim();
            if !category.is_empty() && !view.entry.category.eq_ignore_ascii_case(category) {
                return false;
            }
        }
        if let Some(tag) = &query.tag {
            let tag = tag.trim().to_lowercase();
            if !tag.is_empty() && !view.tags.iter().any(|candidate| candidate == &tag) {
                return false;
            }
        }
        if let Some(language) = &query.language {
            let language = language.trim();
            if !language.is_empty() && !view.answers_for(language) {
                return false;
            }
        }
        if let Some(text) = &query.text {
            let text = text.trim().to_lowercase();
            if !text.is_empty() && !self.holds_text(view, &text) {
                return false;
            }
        }
        true
    }

    /// Whether the asset holds this text in its name, in its alias, in its id or in its intent.
    fn holds_text(&self, view: &AssetView, text: &str) -> bool {
        view.entry
            .names
            .values()
            .any(|name| name.to_lowercase().contains(text))
            || view
                .aliases
                .iter()
                .any(|alias| alias.to_lowercase().contains(text))
            || view.entry.id.to_lowercase().contains(text)
            || view.entry.intent.to_lowercase().contains(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The catalog of the repository.
    fn real_index() -> CatalogIndex {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = Registry::load(&dir).expect("the catalog of the repository is valid");
        CatalogIndex::build(&registry).expect("the index is built")
    }

    #[test]
    fn the_index_holds_the_assets_of_the_registry() {
        let index = real_index();
        assert_eq!(index.len(), 93);
        assert!(!index.is_empty());
        assert_eq!(index.views().len(), 93);
        // the index is a pure function of the registry
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = Registry::load(&dir).expect("the catalog of the repository is valid");
        let again = CatalogIndex::build(&registry).expect("the index is built");
        assert_eq!(again.len(), index.len());
        assert_eq!(again.views(), index.views());
    }

    #[test]
    fn an_asset_is_found_by_its_id() {
        let index = real_index();
        let asset = index.get("algorithm.graph.kruskal").expect("the asset is there");
        assert_eq!(asset.entry.id, "algorithm.graph.kruskal");
        assert_eq!(asset.layer, Layer::Algorithm);
        assert!(index.get("algorithm.graph.nothing").is_none());
    }

    #[test]
    fn an_alias_resolves_to_the_canonical_id() {
        let index = real_index();
        assert_eq!(
            index.resolve_alias("MST"),
            Some("algorithm.graph.kruskal")
        );
        // the alias is compared without case
        assert_eq!(
            index.resolve_alias("mst"),
            Some("algorithm.graph.kruskal")
        );
        // the name of the asset resolves as well
        assert_eq!(
            index.resolve_alias("Kruskal 最小生成树"),
            Some("algorithm.graph.kruskal")
        );
        assert_eq!(
            index.resolve_alias("Kruskal’s Minimum Spanning Tree"),
            Some("algorithm.graph.kruskal")
        );
        // and the id resolves to itself
        assert_eq!(
            index.resolve_alias("algorithm.graph.kruskal"),
            Some("algorithm.graph.kruskal")
        );
    }

    #[test]
    fn an_unknown_name_resolves_to_nothing() {
        let index = real_index();
        assert_eq!(index.resolve_alias("nothing at all"), None);
        assert_eq!(index.resolve_alias(""), None);
        assert_eq!(index.resolve_alias("   "), None);
        assert!(index.by_alias("nothing at all").is_none());
        assert!(index.by_name("nothing at all").is_none());
    }

    #[test]
    fn the_assets_of_a_layer_are_found() {
        let index = real_index();
        assert_eq!(index.in_layer(Layer::Pattern).len(), 23);
        assert_eq!(index.in_layer(Layer::Algorithm).len(), 50);
        assert_eq!(index.in_layer(Layer::Architecture).len(), 7);
        assert_eq!(index.in_layer(Layer::Integration).len(), 6);
        assert_eq!(index.in_layer(Layer::Resilience).len(), 4);
        assert_eq!(index.in_layer(Layer::Concurrency).len(), 3);
        for view in index.in_layer(Layer::Algorithm) {
            assert!(view.id().starts_with("algorithm."), "{}", view.id());
        }
    }

    #[test]
    fn the_assets_of_a_category_are_found() {
        let index = real_index();
        let graph = index.in_category("graph");
        assert_eq!(graph.len(), 9);
        for view in graph {
            assert_eq!(view.entry.category, "graph");
        }
        // the category is compared without case
        assert_eq!(index.in_category("GRAPH").len(), 9);
        // a category that does not exist has no asset
        assert!(index.in_category("nothing").is_empty());
    }

    #[test]
    fn the_assets_with_a_tag_are_found() {
        let index = real_index();
        let stable = index.with_tag("stability=stable");
        assert_eq!(stable.len(), 3);
        for view in stable {
            assert!(view.tags.contains(&"stability=stable".to_string()), "{}", view.id());
        }
        // the tag is compared without case
        assert_eq!(index.with_tag("STABILITY=STABLE").len(), 3);
        // a tag that no asset carries has no asset
        assert!(index.with_tag("nothing").is_empty());
    }

    #[test]
    fn the_assets_of_a_language_are_found() {
        let index = real_index();
        // every asset of the repository is a knowledge card: it is language neutral and it matches
        assert_eq!(index.in_language("rust").len(), 93);
        assert_eq!(index.in_language("typescript").len(), 93);
        assert_eq!(index.in_language("").len(), 93);
    }

    #[test]
    fn the_alternatives_of_an_asset_are_found() {
        let index = real_index();
        let singleton = index.get("pattern.gof.creational.singleton").expect("the asset is there");
        let alternatives = index.alternatives_of(singleton.id());
        assert_eq!(alternatives.len(), 2);
        let ids: Vec<&str> = alternatives.iter().map(|view| view.id()).collect();
        assert!(ids.contains(&"pattern.arch.dependency_injection"), "{ids:?}");
        assert!(ids.contains(&"pattern.gof.structural.proxy"), "{ids:?}");
        // the other way round
        let offering = index.offering("pattern.arch.dependency_injection");
        assert!(offering.iter().any(|view| view.id() == "pattern.gof.creational.singleton"));
        // an asset that is not there has no alternative
        assert!(index.alternatives_of("algorithm.graph.nothing").is_empty());
    }

    #[test]
    fn the_filters_are_combined() {
        let index = real_index();
        let query = CatalogQuery {
            layer: Some(Layer::Algorithm),
            category: Some("graph".to_string()),
            ..CatalogQuery::default()
        };
        let hits = index.search(&query);
        assert_eq!(hits.len(), 9);
        for view in hits {
            assert_eq!(view.layer, Layer::Algorithm);
            assert_eq!(view.entry.category, "graph");
        }
        // a filter that does not fit the other one has no hit
        let query = CatalogQuery {
            text: Some("单例".to_string()),
            ..query
        };
        assert!(index.search(&query).is_empty());
        // the text is found in the name, in the alias and in the intent
        let query = CatalogQuery {
            text: Some("最小生成树".to_string()),
            ..CatalogQuery::default()
        };
        // two cards carry the name: the one of Kruskal and the one of Prim
        let hits = index.search(&query);
        assert_eq!(hits.len(), 2);
        let ids: Vec<&str> = hits.iter().map(|view| view.id()).collect();
        assert!(ids.contains(&"algorithm.graph.kruskal"), "{ids:?}");
        assert!(ids.contains(&"algorithm.graph.prim"), "{ids:?}");
        let query = CatalogQuery {
            text: Some("MST".to_string()),
            ..CatalogQuery::default()
        };
        assert_eq!(index.search(&query).len(), 1);
    }

    #[test]
    fn an_alias_does_not_add_an_asset_to_the_index() {
        let index = real_index();
        // the index holds the 93 manifests, and the aliases are only keys
        assert_eq!(index.len(), 93);
        assert_eq!(index.by_alias("MST").map(|view| view.id()), Some("algorithm.graph.kruskal"));
        // the search by an alias finds the asset, and only it
        let query = CatalogQuery {
            text: Some("MST".to_string()),
            ..CatalogQuery::default()
        };
        assert_eq!(index.search(&query).len(), 1);
    }
}
