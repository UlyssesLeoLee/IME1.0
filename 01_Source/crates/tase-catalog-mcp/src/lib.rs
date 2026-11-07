//! TASE catalog MCP: the projection of the unified standard asset catalog into MCP tools
//! (detailed design §19.6, basic design §17.5, requirements §14.4).
//!
//! The crate turns the 93 knowledge cards of `assets/tase-catalog` into the four tools an AI client
//! calls:
//!
//! | the tool | what it does |
//! |---|---|
//! | `catalog.search` | finds the cards by keyword, language, layer, category and tag |
//! | `catalog.recommend` | ranks the assets that answer an intent, with the breakdown of the score |
//! | `catalog.instantiate` | drafts a plan out of an **approved** implementation of the target language |
//! | `catalog.describe` | gives the whole knowledge card of one asset |
//!
//! # The three modules
//!
//! * [`schema`] - the JSON Schema of the four tools, written by hand and exported as `&'static str`;
//! * [`tools`] - the declarations of the tools, the [`CatalogService`] trait and the dispatcher;
//! * [`adapter`] - the adapter of `tase-catalog` and the JSON-RPC shape of the answers.
//!
//! # The catalog is behind a trait
//!
//! The tools do not know how the answers are computed: they read the arguments, they call the
//! [`CatalogService`] and they put the answer on the wire. The adapter of [`CatalogAdapter`]
//! implements the trait over the [`tase_catalog::Catalog`], so that the ranking, the gate and the
//! governance are **not written twice** - they come from `tase-catalog`.
//!
//! ```
//! use tase_catalog_mcp::tools;
//!
//! let list = tools();
//! assert_eq!(list.len(), 4);
//! assert_eq!(list[0].name, "catalog.search");
//! assert_eq!(list[3].name, "catalog.describe");
//! ```
//!
//! # A knowledge card is not an implementation
//!
//! The catalog of this repository holds **93 knowledge cards and not one language implementation**.
//! The crate keeps that difference visible everywhere:
//!
//! * `catalog.instantiate` is refused with [`tase_contract::ErrorCode::PolicyRejected`] and says why - a card may
//!   be read and quoted, it may not be instantiated;
//! * every answer says whether the asset may be instantiated, and why;
//! * the counterexamples, the license and the risks of an asset travel with every answer, so that a
//!   model cannot pick an asset without seeing what it does not solve (requirements §14.4).
//!
//! # The shape of the answers
//!
//! The answer of a tool is a JSON object that starts with `{"ok": true, "tool": "<the name>"}` and
//! holds the fields of the answer. The adapter wraps it in the shape of `pime-agent`:
//! `{"content": [{"type": "text", "text": "..."}], "isError": false, "structuredContent": {...}}`.
//!
//! # The dependencies
//!
//! The crate has no third-party dependency: the strict JSON parser comes from `pime-core`, the error
//! codes from `tase-contract` and the catalog from `tase-catalog`.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod schema;
pub mod tools;

pub use adapter::{CatalogAdapter, METHOD_TOOLS_CALL, METHOD_TOOLS_LIST};
pub use schema::{
    DESCRIBE_SCHEMA, INSTANTIATE_SCHEMA, RECOMMEND_SCHEMA, SCHEMAS, SCHEMA_DIALECT, SEARCH_SCHEMA,
    schema_of,
};
pub use tools::{
    AssetDetail, AssetSummary, CatalogService, CatalogTools, DEFAULT_LIMIT, DESCRIBE, INSTRUCTIONS,
    INSTANTIATE, ImplementationSummary, InstantiateRequest, MAX_LIMIT, PlanDraft, RECOMMEND,
    RecommendQuery, RecommendedAsset, RefusedAsset, ScorePart, SEARCH, SearchQuery, TOOL_NAMES,
    ToolDefinition, tools,
};
