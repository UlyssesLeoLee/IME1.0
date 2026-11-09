//! The view of an asset of the catalog: the layer, the tags and the aliases that the catalog derives from the
//! manifest of the registry (basic design §17.1 and §17.2).
//!
//! The catalog of this repository holds **93 knowledge cards** and **not one language implementation**. A view is
//! therefore always a *card*: it may be read, searched and quoted, it may not be instantiated. The view keeps that
//! difference visible: it carries no implementation of its own, and it never pretends that an alias is a second
//! asset.
//!
//! # The layer
//!
//! The layer of an asset is not written in the manifest: it is the namespace of its id (detailed design §19.4):
//!
//! | the id starts with | the layer is |
//! |---|---|
//! | `pattern.gof.` | [`Layer::Pattern`] |
//! | `pattern.arch.` | [`Layer::Architecture`] |
//! | `pattern.integration.` | [`Layer::Integration`] |
//! | `pattern.resilience.` | [`Layer::Resilience`] |
//! | `pattern.concurrent.` | [`Layer::Concurrency`] |
//! | `algorithm.` | [`Layer::Algorithm`] |
//!
//! An id that names no known namespace is refused: a card that does not say which layer it belongs to does not
//! belong to the catalog.
//!
//! # The tags
//!
//! The tags are the keys the manifest itself declares - the catalog never invents a tag:
//!
//! * one tag per condition of `applicability.constraints`: the name of the condition (`stability`) and the
//!   condition with its value (`stability=stable`);
//! * one tag per alternative of `applicability.alternatives`: `alternative=algorithm.graph.prim`.
//!
//! The tags are sorted and free of duplicates, so two runs of the catalog over the same manifest give the same tags.
//!
//! # The aliases
//!
//! The aliases are the other names of the asset (`Singleton`, `单例模式`, `单例`). They are the **search keys** of
//! requirements §14.3 and nothing else: an alias never becomes a second asset, and the number of the assets of the
//! catalog is the number of the manifests, never the number of the names.

use tase_contract::TaseError;
use tase_registry::AssetEntry;

/// The logical layer of an asset (basic design §17.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// The patterns of GoF (`pattern.gof.*`).
    Pattern,
    /// The architecture patterns (`pattern.arch.*`).
    Architecture,
    /// The integration patterns (`pattern.integration.*`).
    Integration,
    /// The resilience patterns (`pattern.resilience.*`).
    Resilience,
    /// The concurrency patterns (`pattern.concurrent.*`).
    Concurrency,
    /// The algorithms (`algorithm.*`).
    Algorithm,
}

impl Layer {
    /// The layer of an asset, from the namespace of its id.
    ///
    /// `None` when the id names no known namespace.
    pub fn of_id(id: &str) -> Option<Layer> {
        if id.starts_with("pattern.gof.") {
            Some(Layer::Pattern)
        } else if id.starts_with("pattern.arch.") {
            Some(Layer::Architecture)
        } else if id.starts_with("pattern.integration.") {
            Some(Layer::Integration)
        } else if id.starts_with("pattern.resilience.") {
            Some(Layer::Resilience)
        } else if id.starts_with("pattern.concurrent.") {
            Some(Layer::Concurrency)
        } else if id.starts_with("algorithm.") {
            Some(Layer::Algorithm)
        } else {
            None
        }
    }

    /// The name of the layer on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Pattern => "PATTERN",
            Layer::Architecture => "ARCHITECTURE",
            Layer::Integration => "INTEGRATION",
            Layer::Resilience => "RESILIENCE",
            Layer::Concurrency => "CONCURRENCY",
            Layer::Algorithm => "ALGORITHM",
        }
    }

    /// The namespace of the ids of the layer.
    pub fn namespace(self) -> &'static str {
        match self {
            Layer::Pattern => "pattern.gof.",
            Layer::Architecture => "pattern.arch.",
            Layer::Integration => "pattern.integration.",
            Layer::Resilience => "pattern.resilience.",
            Layer::Concurrency => "pattern.concurrent.",
            Layer::Algorithm => "algorithm.",
        }
    }

    /// The layer of a name; `None` when the name is not a layer.
    pub fn from_name(name: &str) -> Option<Layer> {
        match name {
            "PATTERN" => Some(Layer::Pattern),
            "ARCHITECTURE" => Some(Layer::Architecture),
            "INTEGRATION" => Some(Layer::Integration),
            "RESILIENCE" => Some(Layer::Resilience),
            "CONCURRENCY" => Some(Layer::Concurrency),
            "ALGORITHM" => Some(Layer::Algorithm),
            _ => None,
        }
    }
}

/// What the manifest of an asset declares about its own properties.
///
/// The traits are read from the conditions of `applicability.constraints` and from nothing else. A card that says
/// nothing about a property is neither punished nor rewarded on it: the recommendation reports it as a risk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traits {
    /// The card declares that the asset keeps a global mutable state.
    pub global_mutable_state: bool,
    /// The card declares that the asset is not thread safe.
    pub not_thread_safe: bool,
    /// The card declares that the asset is thread safe.
    pub declares_thread_safe: bool,
    /// The card declares that the asset may need negative inputs (negative weights or negative edges).
    pub may_need_negative_inputs: bool,
    /// The card declares that the asset does not need negative inputs.
    pub declares_nonnegative_inputs: bool,
    /// The card declares the stability of the order it produces: `None` when it says nothing about it.
    pub stable_order: Option<bool>,
}

/// The tags of an asset: the keys that the manifest itself declares.
///
/// * one tag per condition of `applicability.constraints`: the name of the condition and the condition with its
///   value (`stability`, `stability=stable`);
/// * one tag per alternative of `applicability.alternatives`: `alternative=algorithm.graph.prim`.
///
/// The list is sorted and free of duplicates.
pub fn tags_of(entry: &AssetEntry) -> Vec<String> {
    let mut tags: Vec<String> = Vec::with_capacity(entry.constraints.len() * 2 + entry.alternatives.len());
    for (name, value) in &entry.constraints {
        let name = name.trim().to_lowercase();
        let value = value.trim().to_lowercase();
        if name.is_empty() {
            continue;
        }
        tags.push(name.clone());
        tags.push(format!("{name}={value}"));
    }
    for alternative in &entry.alternatives {
        let alternative = alternative.trim().to_lowercase();
        if alternative.is_empty() {
            continue;
        }
        tags.push(format!("alternative={alternative}"));
    }
    tags.sort();
    tags.dedup();
    tags
}

/// The traits of an asset, read from the conditions of its manifest.
pub fn traits_of(entry: &AssetEntry) -> Traits {
    let mut traits = Traits::default();
    for (name, value) in &entry.constraints {
        let value = value.to_lowercase();
        if value.contains("mutable_global")
            || value.contains("global_mutable")
            || value.contains("global_state")
            || value.contains("全局可变")
        {
            traits.global_mutable_state = true;
        }
        if value.contains("requires_lock")
            || value.contains("not_thread_safe")
            || value.contains("shared_mutable")
            || value.contains("需要锁")
            || value.contains("非线程安全")
        {
            traits.not_thread_safe = true;
        }
        if value.contains("thread_safe") || value.contains("线程安全") {
            traits.declares_thread_safe = true;
        }
        // "nonnegative" contains "negative": the word that forbids the negative inputs comes first.
        if value.contains("negative") && !value.contains("nonnegative") && !value.contains("non-negative") {
            traits.may_need_negative_inputs = true;
        }
        if value.contains("nonnegative") || value.contains("non-negative") {
            traits.declares_nonnegative_inputs = true;
        }
        if name.trim() == "stability" {
            traits.stable_order = Some(value.trim() == "stable");
        }
    }
    if traits.global_mutable_state {
        // a global mutable state is a shared state: it is not thread safe without an external lock.
        traits.not_thread_safe = true;
    }
    traits
}

/// The view of an asset: the entry of the registry and what the catalog derives from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetView {
    /// The entry of the registry this view was derived from.
    pub entry: AssetEntry,
    /// The logical layer of the asset.
    pub layer: Layer,
    /// The keys the manifest declares.
    pub tags: Vec<String>,
    /// The other names of the asset (the search keys of requirements §14.3).
    pub aliases: Vec<String>,
}

impl AssetView {
    /// Builds the view of an asset.
    ///
    /// The layer, the tags and the aliases are derived from the entry; an id that names no known layer is refused
    /// with [`ErrorCode::InvalidArgument`].
    pub fn new(entry: AssetEntry) -> Result<AssetView, TaseError> {
        let layer = Layer::of_id(&entry.id).ok_or_else(|| {
            TaseError::invalid_argument(format!(
                "the id {} names no known layer of the catalog",
                entry.id
            ))
        })?;
        let tags = tags_of(&entry);
        let mut aliases: Vec<String> = Vec::with_capacity(entry.aliases.len());
        for alias in &entry.aliases {
            let alias = alias.trim().to_string();
            if alias.is_empty() {
                continue;
            }
            if !aliases.contains(&alias) {
                aliases.push(alias);
            }
        }
        Ok(AssetView {
            entry,
            layer,
            tags,
            aliases,
        })
    }

    /// The id of the asset.
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    /// The traits the manifest of the asset declares.
    pub fn traits(&self) -> Traits {
        traits_of(&self.entry)
    }

    /// Whether the asset carries an implementation in this language.
    ///
    /// An asset with **no** implementation at all is language neutral: a knowledge card is written for every
    /// language. An asset that has implementations but not one in this language does not answer for it (an
    /// implementation that is approved for Rust says nothing about TypeScript, requirements §14.4).
    pub fn answers_for(&self, language: &str) -> bool {
        self.entry.implementations.is_empty() || self.entry.has_implementation_in(language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    /// The catalog of the repository: 93 knowledge cards and no implementation.
    fn real_catalog() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog")
    }

    /// The views of the catalog of the repository.
    fn real_views() -> Vec<AssetView> {
        let registry = tase_registry::Registry::load(&real_catalog()).expect("the catalog is valid");
        registry
            .entries()
            .iter()
            .cloned()
            .map(AssetView::new)
            .collect::<Result<Vec<_>, _>>()
            .expect("every asset of the repository names a layer")
    }

    #[test]
    fn the_layer_comes_from_the_namespace_of_the_id() {
        assert_eq!(Layer::of_id("pattern.gof.creational.singleton"), Some(Layer::Pattern));
        assert_eq!(
            Layer::of_id("pattern.arch.dependency_injection"),
            Some(Layer::Architecture)
        );
        assert_eq!(
            Layer::of_id("pattern.integration.saga"),
            Some(Layer::Integration)
        );
        assert_eq!(
            Layer::of_id("pattern.resilience.circuit_breaker"),
            Some(Layer::Resilience)
        );
        assert_eq!(
            Layer::of_id("pattern.concurrent.actor"),
            Some(Layer::Concurrency)
        );
        assert_eq!(Layer::of_id("algorithm.graph.dijkstra"), Some(Layer::Algorithm));
        // an id that names no known namespace has no layer
        assert_eq!(Layer::of_id("recipe.cooking.risotto"), None);
        assert_eq!(Layer::of_id(""), None);
    }

    #[test]
    fn every_asset_of_the_repository_has_the_layer_of_its_namespace() {
        let views = real_views();
        assert_eq!(views.len(), 93);
        let mut counts: BTreeMap<Layer, usize> = BTreeMap::new();
        for view in &views {
            assert_eq!(Layer::of_id(view.id()), Some(view.layer));
            assert!(view.id().starts_with(view.layer.namespace()), "{}", view.id());
            *counts.entry(view.layer).or_insert(0) += 1;
        }
        assert_eq!(counts.get(&Layer::Pattern), Some(&23));
        assert_eq!(counts.get(&Layer::Architecture), Some(&7));
        assert_eq!(counts.get(&Layer::Integration), Some(&6));
        assert_eq!(counts.get(&Layer::Resilience), Some(&4));
        assert_eq!(counts.get(&Layer::Concurrency), Some(&3));
        assert_eq!(counts.get(&Layer::Algorithm), Some(&50));
    }

    #[test]
    fn the_layer_travels_by_name() {
        for layer in [
            Layer::Pattern,
            Layer::Architecture,
            Layer::Integration,
            Layer::Resilience,
            Layer::Concurrency,
            Layer::Algorithm,
        ] {
            assert_eq!(Layer::from_name(layer.as_str()), Some(layer));
        }
        assert_eq!(Layer::from_name("RECIPE"), None);
    }

    #[test]
    fn the_tags_come_from_the_conditions_and_the_alternatives() {
        let views = real_views();
        let singleton = views
            .iter()
            .find(|view| view.id() == "pattern.gof.creational.singleton")
            .expect("the singleton is there");
        // the conditions of the manifest become the tags
        assert!(singleton.tags.contains(&"state".to_string()));
        assert!(singleton.tags.contains(&"state=mutable_global_requires_locks".to_string()));
        assert!(singleton.tags.contains(&"instance_count".to_string()));
        // the alternatives become the tags
        assert!(
            singleton
                .tags
                .contains(&"alternative=pattern.arch.dependency_injection".to_string())
        );
        // no tag is invented: every tag of the card comes from its manifest
        for view in &views {
            for tag in &view.tags {
                let from_condition = view
                    .entry
                    .constraints
                    .iter()
                    .any(|(name, value)| {
                        *tag == name.trim().to_lowercase()
                            || *tag == format!("{}={}", name.trim().to_lowercase(), value.trim().to_lowercase())
                    });
                let from_alternative = view
                    .entry
                    .alternatives
                    .iter()
                    .any(|alternative| *tag == format!("alternative={}", alternative.trim().to_lowercase()));
                assert!(from_condition || from_alternative, "{tag} of {}", view.id());
            }
        }
    }

    #[test]
    fn the_tags_are_sorted_and_free_of_duplicates() {
        for view in real_views() {
            let mut sorted = view.tags.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted, view.tags, "{}", view.id());
        }
    }

    #[test]
    fn the_aliases_are_the_search_keys_of_the_manifest() {
        let views = real_views();
        let kruskal = views
            .iter()
            .find(|view| view.id() == "algorithm.graph.kruskal")
            .expect("the asset is there");
        assert_eq!(
            kruskal.aliases,
            ["Kruskal", "最小生成树", "MST", "克鲁斯卡尔算法"]
        );
        // the aliases of the view are the aliases of the manifest, in the order of the manifest
        for view in &views {
            assert_eq!(view.aliases.len(), view.entry.aliases.len());
        }
    }

    #[test]
    fn an_alias_does_not_inflate_the_number_of_the_assets() {
        let views = real_views();
        // 93 assets, and the aliases are keys: they do not add an asset
        assert_eq!(views.len(), 93);
        let alias_count: usize = views.iter().map(|view| view.aliases.len()).sum();
        assert_eq!(alias_count, 332);
        // no alias is owned by two assets
        let mut owners: BTreeMap<String, String> = BTreeMap::new();
        for view in &views {
            for alias in &view.aliases {
                let key = alias.to_lowercase();
                match owners.get(&key) {
                    Some(owner) => panic!("the alias {alias} is owned by {owner} and by {}", view.id()),
                    None => {
                        owners.insert(key, view.id().to_string());
                    }
                }
            }
        }
        assert_eq!(owners.len(), alias_count);
        // and no alias is the id of another asset
        for (alias, owner) in &owners {
            assert!(!views.iter().any(|view| view.id() == alias), "{alias}");
            assert!(views.iter().any(|view| view.id() == owner));
        }
    }

    #[test]
    fn the_singleton_declares_the_global_mutable_state() {
        let views = real_views();
        let singleton = views
            .iter()
            .find(|view| view.id() == "pattern.gof.creational.singleton")
            .expect("the asset is there");
        let traits = singleton.traits();
        assert!(traits.global_mutable_state);
        assert!(traits.not_thread_safe);
        // the other cards of the repository do not declare it
        for view in &views {
            if view.id() == "pattern.gof.creational.singleton" {
                continue;
            }
            assert!(!view.traits().global_mutable_state, "{}", view.id());
        }
    }

    #[test]
    fn the_traits_of_the_algorithms_read_the_negative_inputs_and_the_stability() {
        let views = real_views();
        let dijkstra = views
            .iter()
            .find(|view| view.id() == "algorithm.graph.dijkstra")
            .expect("the asset is there");
        let traits = dijkstra.traits();
        assert!(!traits.may_need_negative_inputs);
        assert!(traits.declares_nonnegative_inputs);

        let bellman_ford = views
            .iter()
            .find(|view| view.id() == "algorithm.graph.bellman_ford")
            .expect("the asset is there");
        assert!(bellman_ford.traits().may_need_negative_inputs);
        assert!(!bellman_ford.traits().declares_nonnegative_inputs);

        let merge_sort = views
            .iter()
            .find(|view| view.id() == "algorithm.sorting.merge_sort")
            .expect("the asset is there");
        assert_eq!(merge_sort.traits().stable_order, Some(true));
        let heap_sort = views
            .iter()
            .find(|view| view.id() == "algorithm.sorting.heap_sort")
            .expect("the asset is there");
        assert_eq!(heap_sort.traits().stable_order, Some(false));
        // a card that says nothing about the stability says nothing
        let kruskal = views
            .iter()
            .find(|view| view.id() == "algorithm.graph.kruskal")
            .expect("the asset is there");
        assert_eq!(kruskal.traits().stable_order, None);
    }

    #[test]
    fn a_card_without_an_implementation_answers_for_every_language() {
        let views = real_views();
        for view in &views {
            // The patterns carry no implementation: they answer in every language. The fifty algorithms carry their
            // implementation (the crate `tase-impls`), so they answer in the language of that implementation.
            if view.id().starts_with("pattern.") {
                assert!(view.entry.implementations.is_empty(), "{}", view.id());
                assert!(view.answers_for("rust"));
                assert!(view.answers_for("typescript"));
            } else {
                assert_eq!(view.entry.implementations.len(), 1, "{}", view.id());
                assert!(view.answers_for("rust"), "{}", view.id());
                assert!(
                    !view.answers_for("typescript"),
                    "{} has no implementation in typescript",
                    view.id()
                );
            }
        }
    }

    #[test]
    fn an_id_that_names_no_layer_is_refused() {
        let mut entry = tase_registry::AssetEntry {
            id: "recipe.cooking.risotto".to_string(),
            version: "1.0.0".to_string(),
            kind: "recipe".to_string(),
            category: "cooking".to_string(),
            names: BTreeMap::new(),
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
        let error = AssetView::new(entry.clone()).expect_err("there is no layer");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        // an id of a known namespace is accepted
        entry.id = "algorithm.graph.dijkstra".to_string();
        assert!(AssetView::new(entry).is_ok());
    }
}
