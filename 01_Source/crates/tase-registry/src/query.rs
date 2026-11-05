//! The search of the catalog (design §2.2 and §19.3).
//!
//! The search of the registry is a read-only operation: it answers "which assets may help here?" and it never
//! says that a knowledge card is an executable template. Every hit carries the real state of the asset in
//! `approval_status` (see [`crate::policy::Approval`]).
//!
//! The filters are:
//!
//! * `language` - the asset has an implementation in that language. An asset that has **no** implementation at
//!   all is language neutral: it matches, because a knowledge card is written for every language, but it stays
//!   a knowledge card. An asset that has implementations but not one in the requested language does **not**
//!   match: an implementation that is approved for Rust says nothing about TypeScript (requirements §14.4).
//! * `tag` - one of the aliases of the asset (the search keys of requirements §14.3). The category and the kind
//!   have their own filters.
//! * `category` - the category of the asset.
//! * `keyword` - a text that is found in the name of the asset (Chinese or English), in its intent or in one of
//!   its aliases.
//!
//! The answer is a page: `limit` hits (20 by default, 100 at most) and the cursor of the next page. The cursor
//! is opaque: it carries the place in the result and a fingerprint of the filters, so a cursor that is used
//! with another query is refused instead of paging the wrong list.

use pime_core::base64;
use pime_core::sha256::{sha256, to_hex};
use tase_contract::api::{TemplateSearchResult, TemplateSummary};
use tase_contract::{TemplateId, TaseError};

use crate::digest;
use crate::loader::Registry;
use crate::manifest::AssetEntry;
use crate::policy::Policy;

/// The number of hits a search returns when the caller does not ask for a number.
pub const DEFAULT_LIMIT: usize = 20;

/// The largest number of hits a search returns.
pub const MAX_LIMIT: usize = 100;

/// The language of an asset that has no implementation: a knowledge card is not written in a language.
pub const LANGUAGE_NEUTRAL: &str = "language-neutral";

/// The query of the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Only the assets that have an implementation in this language.
    pub language: Option<String>,
    /// Only the assets that carry this tag (one of their aliases).
    pub tag: Option<String>,
    /// Only the assets of this category.
    pub category: Option<String>,
    /// Only the assets whose name, intent or alias holds this text.
    pub keyword: Option<String>,
    /// How many hits to return at most (0 means the default).
    pub limit: usize,
    /// The cursor of the page to return.
    pub cursor: Option<String>,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            language: None,
            tag: None,
            category: None,
            keyword: None,
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

/// The fingerprint of the filters of a query: the cursor of a page belongs to the query that made it.
pub fn query_fingerprint(query: &Query) -> String {
    let canonical = format!(
        "{}|{}|{}|{}",
        normalized(query.language.as_deref()),
        normalized(query.tag.as_deref()),
        normalized(query.category.as_deref()),
        normalized(query.keyword.as_deref()),
    );
    let hex = to_hex(&sha256(canonical.as_bytes()));
    hex[..16].to_string()
}

/// The text of a filter, without case and without the spaces around it.
fn normalized(text: Option<&str>) -> String {
    text.unwrap_or_default().trim().to_lowercase()
}

/// The searcher of the registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct Searcher;

impl Searcher {
    /// Searches the catalog.
    ///
    /// The answer holds the hits of the page and the cursor of the next page. The `approval_status` of every
    /// hit is the real state of the asset: a knowledge card is never presented as an executable template.
    pub fn search(
        &self,
        registry: &Registry,
        query: &Query,
        policy: &Policy,
    ) -> Result<TemplateSearchResult, TaseError> {
        let limit = if query.limit == 0 {
            DEFAULT_LIMIT
        } else {
            query.limit
        };
        if limit > MAX_LIMIT {
            return Err(TaseError::invalid_argument(format!(
                "the limit {limit} is larger than the largest page {MAX_LIMIT}"
            )));
        }

        let fingerprint = query_fingerprint(query);
        let offset = match &query.cursor {
            Some(cursor) => decode_cursor(cursor, &fingerprint)?,
            None => 0,
        };

        let hits: Vec<&AssetEntry> = registry
            .entries()
            .iter()
            .filter(|entry| matches(entry, query))
            .collect();
        if offset > hits.len() {
            return Err(TaseError::invalid_argument(format!(
                "the cursor points at the hit {offset} of a search that has {} hits",
                hits.len()
            )));
        }

        let end = offset.saturating_add(limit).min(hits.len());
        let mut templates = Vec::with_capacity(end - offset);
        for entry in &hits[offset..end] {
            templates.push(summary_of(entry, query, policy)?);
        }
        let next_cursor = if end < hits.len() {
            Some(encode_cursor(end, &fingerprint))
        } else {
            None
        };

        Ok(TemplateSearchResult {
            templates,
            next_cursor,
        })
    }
}

/// Searches the catalog: the short way to call [`Searcher::search`].
pub fn search(
    registry: &Registry,
    query: &Query,
    policy: &Policy,
) -> Result<TemplateSearchResult, TaseError> {
    Searcher.search(registry, query, policy)
}

/// Whether the asset answers the query.
fn matches(entry: &AssetEntry, query: &Query) -> bool {
    if let Some(language) = &query.language {
        let language = language.trim();
        if !language.is_empty()
            && !entry.implementations.is_empty()
            && !entry.has_implementation_in(language)
        {
            return false;
        }
    }
    if let Some(tag) = &query.tag {
        let tag = tag.trim();
        if !tag.is_empty()
            && !entry
                .aliases
                .iter()
                .any(|alias| alias.trim().eq_ignore_ascii_case(tag))
        {
            return false;
        }
    }
    if let Some(category) = &query.category {
        let category = category.trim();
        if !category.is_empty() && !entry.category.eq_ignore_ascii_case(category) {
            return false;
        }
    }
    if let Some(keyword) = &query.keyword {
        let keyword = keyword.trim().to_lowercase();
        if !keyword.is_empty() {
            let hit = entry.names.values().any(|name| name.to_lowercase().contains(&keyword))
                || entry.intent.to_lowercase().contains(&keyword)
                || entry
                    .aliases
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&keyword));
            if !hit {
                return false;
            }
        }
    }
    true
}

/// The summary of a hit.
fn summary_of(
    entry: &AssetEntry,
    query: &Query,
    policy: &Policy,
) -> Result<TemplateSummary, TaseError> {
    let digest = digest::hex_of(&entry.digest).ok_or_else(|| {
        TaseError::internal(format!("the asset {} has no digest", entry.id))
    })?;
    Ok(TemplateSummary {
        id: TemplateId::parse(&entry.id)?,
        version: entry.version.clone(),
        digest: digest.to_string(),
        language: language_of(entry, query),
        tags: entry.aliases.clone(),
        params_schema_ref: format!(
            "artifact://templates/{}/{}/params.schema.json",
            entry.id, entry.version
        ),
        approval_status: policy.approval_of(entry).as_str().to_string(),
        license: entry.license.clone().unwrap_or_default(),
    })
}

/// The language of the hit: the language of the implementation that answers the query, or the language of the
/// first implementation, or the language of a knowledge card.
fn language_of(entry: &AssetEntry, query: &Query) -> String {
    if let Some(language) = &query.language
        && let Some(implementation) = entry
            .implementations
            .iter()
            .find(|implementation| implementation.is_in_language(language))
    {
        return implementation.language.clone();
    }
    match entry.implementations.first() {
        Some(implementation) => implementation.language.clone(),
        None => LANGUAGE_NEUTRAL.to_string(),
    }
}

/// The cursor of a page: the place in the result and the fingerprint of the filters, hidden from the caller.
fn encode_cursor(offset: usize, fingerprint: &str) -> String {
    base64::encode(format!("tase1:{offset}:{fingerprint}").as_bytes())
}

/// The place of the page a cursor points at.
fn decode_cursor(cursor: &str, fingerprint: &str) -> Result<usize, TaseError> {
    let invalid = || {
        TaseError::invalid_argument("the cursor is not a cursor of this search".to_string())
    };
    let bytes = base64::decode(cursor).ok_or_else(invalid)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let mut parts = text.splitn(3, ':');
    let tag = parts.next().unwrap_or_default();
    let offset = parts.next().unwrap_or_default();
    let cursor_fingerprint = parts.next().unwrap_or_default();
    if tag != "tase1" {
        return Err(invalid());
    }
    if cursor_fingerprint != fingerprint {
        return Err(TaseError::invalid_argument(
            "the cursor belongs to another search".to_string(),
        ));
    }
    offset.parse::<usize>().map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Registry;
    use std::path::Path;

    /// The catalog of the repository.
    fn registry() -> Registry {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        Registry::load(&dir).expect("the catalog of the repository is valid")
    }

    /// A query without a filter.
    fn query() -> Query {
        Query::default()
    }

    #[test]
    fn a_search_without_a_filter_returns_the_first_page() {
        let registry = registry();
        let answer = search(&registry, &query(), &Policy::default()).expect("the search works");
        assert_eq!(answer.templates.len(), DEFAULT_LIMIT);
        assert!(answer.next_cursor.is_some());
        // the order of the index is kept
        assert_eq!(
            answer.templates[0].id.as_str(),
            "pattern.gof.creational.abstract_factory"
        );
    }

    #[test]
    fn every_hit_of_the_repository_is_a_knowledge_card() {
        let registry = registry();
        let answer = search(&registry, &query(), &Policy::default()).expect("the search works");
        for template in &answer.templates {
            assert_eq!(template.approval_status, "KNOWLEDGE_ONLY");
            assert_eq!(template.language, LANGUAGE_NEUTRAL);
            // the digest is a real one
            assert_eq!(template.digest.len(), 64);
            assert!(template.digest.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(template.digest.bytes().all(|b| !b.is_ascii_uppercase()));
            // the schema of the parameters is referenced, not invented
            assert_eq!(
                template.params_schema_ref,
                format!(
                    "artifact://templates/{}/{}/params.schema.json",
                    template.id.as_str(),
                    template.version
                )
            );
        }
    }

    #[test]
    fn the_category_filters_the_hits() {
        let registry = registry();
        let mut query = query();
        query.category = Some("creational".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(!answer.templates.is_empty());
        // every hit of the page is a pattern of the category
        for template in &answer.templates {
            assert!(template.id.as_str().contains("pattern.gof.creational."));
        }
        // a category that does not exist has no hit
        query.category = Some("nothing".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(answer.templates.is_empty());
        assert!(answer.next_cursor.is_none());
    }

    #[test]
    fn the_tag_filters_the_hits() {
        let registry = registry();
        let mut query = query();
        query.tag = Some("MST".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(answer.templates.len(), 1);
        assert_eq!(answer.templates[0].id.as_str(), "algorithm.graph.kruskal");
        // the tag is compared without case
        query.tag = Some("mst".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(answer.templates.len(), 1);
        // a tag that no asset carries has no hit
        query.tag = Some("nothing".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(answer.templates.is_empty());
    }

    #[test]
    fn the_keyword_is_found_in_the_name_the_intent_and_the_aliases() {
        let registry = registry();
        let mut query = query();
        // the English name
        query.keyword = Some("Abstract Factory".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(!answer.templates.is_empty());
        // the Chinese name
        query.keyword = Some("抽象工厂".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(answer.templates.len(), 1);
        assert_eq!(
            answer.templates[0].id.as_str(),
            "pattern.gof.creational.abstract_factory"
        );
        // an alias
        query.keyword = Some("kruskal".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(!answer.templates.is_empty());
        assert!(answer.templates.iter().any(|template| template
            .id
            .as_str()
            .contains("kruskal")));
        // a keyword that no asset holds has no hit
        query.keyword = Some("nothing at all".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(answer.templates.is_empty());
    }

    #[test]
    fn the_language_of_a_knowledge_card_is_language_neutral() {
        let registry = registry();
        let mut query = query();
        query.language = Some("rust".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        // every asset of the repository is a knowledge card: it is language neutral and it matches
        assert_eq!(answer.templates.len(), DEFAULT_LIMIT);
        for template in &answer.templates {
            assert_eq!(template.approval_status, "KNOWLEDGE_ONLY");
        }
    }

    #[test]
    fn the_cursor_continues_the_previous_page() {
        let registry = registry();
        let mut query = query();
        query.limit = 10;
        let first = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(first.templates.len(), 10);
        let cursor = first.next_cursor.clone().expect("there is a next page");
        query.cursor = Some(cursor);
        let second = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(second.templates.len(), 10);
        // the second page continues the first one
        assert_ne!(first.templates[0].id, second.templates[0].id);
        let first_ids: Vec<&str> = first.templates.iter().map(|t| t.id.as_str()).collect();
        let second_ids: Vec<&str> = second.templates.iter().map(|t| t.id.as_str()).collect();
        assert!(first_ids.iter().all(|id| !second_ids.contains(id)));
        // the last page has no cursor
        let mut cursor = first.next_cursor.clone();
        let mut pages = 1;
        while let Some(next) = cursor {
            query.cursor = Some(next);
            let answer = search(&registry, &query, &Policy::default()).expect("the search works");
            pages += 1;
            cursor = answer.next_cursor.clone();
            assert!(pages < 20, "the pages must end");
        }
        // 93 hits by pages of 10
        assert_eq!(pages, 10);
    }

    #[test]
    fn a_limit_that_is_too_large_is_refused() {
        let registry = registry();
        let mut query = query();
        query.limit = MAX_LIMIT + 1;
        let error = search(&registry, &query, &Policy::default()).expect_err("the limit is too large");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        // the largest page is accepted
        query.limit = MAX_LIMIT;
        assert!(search(&registry, &query, &Policy::default()).is_ok());
        // a limit of zero means the default
        query.limit = 0;
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(answer.templates.len(), DEFAULT_LIMIT);
    }

    #[test]
    fn a_cursor_that_is_out_of_range_is_refused() {
        let registry = registry();
        let mut query = query();
        let fingerprint = query_fingerprint(&query);
        query.cursor = Some(base64::encode(
            format!("tase1:9999:{fingerprint}").as_bytes(),
        ));
        let error = search(&registry, &query, &Policy::default()).expect_err("the cursor is out of range");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);

        // a cursor that is not a cursor at all
        query.cursor = Some("not a cursor".to_string());
        let error = search(&registry, &query, &Policy::default()).expect_err("the cursor is not one");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_cursor_of_another_search_is_refused() {
        let registry = registry();
        let mut query = query();
        query.limit = 10;
        let first = search(&registry, &query, &Policy::default()).expect("the search works");
        let cursor = first.next_cursor.clone().expect("there is a next page");
        // the same cursor with another filter pages the wrong list
        query.keyword = Some("kruskal".to_string());
        query.cursor = Some(cursor);
        let error = search(&registry, &query, &Policy::default()).expect_err("the cursor is of another search");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        assert!(error.message.contains("another search"), "{error}");
    }

    #[test]
    fn the_filters_are_combined() {
        let registry = registry();
        let mut query = query();
        query.category = Some("creational".to_string());
        query.keyword = Some("抽象工厂".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert_eq!(answer.templates.len(), 1);
        // a filter that does not fit the other one has no hit
        query.keyword = Some("kruskal".to_string());
        let answer = search(&registry, &query, &Policy::default()).expect("the search works");
        assert!(answer.templates.is_empty());
    }
}
