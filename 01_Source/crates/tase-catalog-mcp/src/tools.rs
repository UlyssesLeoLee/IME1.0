//! The four MCP tools of the catalog, and the service they call.
//!
//! # The four tools
//!
//! | the tool | the service call | what the answer holds |
//! |---|---|---|
//! | `catalog.search` | [`CatalogService::search`] | the cards that answer the query, with their counterexamples, their license and their risks |
//! | `catalog.recommend` | [`CatalogService::recommend`] | the ranked candidates, the breakdown of their score, their counterexamples and their risks |
//! | `catalog.instantiate` | [`CatalogService::instantiate`] | the draft of a plan - only when an APPROVED implementation of the target language exists |
//! | `catalog.describe` | [`CatalogService::describe`] | the whole knowledge card: the body of the `card.md`, the manifest and the governance |
//!
//! The order of the tools is fixed: the list the server publishes is read from one declaration, so
//! the server cannot drift away from it.
//!
//! # The seam
//!
//! [`CatalogService`] is the seam between the MCP surface and the catalog itself. The tools know
//! nothing about how the answers are computed: they read the arguments, they call the service and
//! they put the answer on the wire. The adapter of `tase-catalog` implements the trait, and a test
//! may implement it with a fake service.
//!
//! # The counterexamples and the risks are never dropped
//!
//! Requirements §14.4 says it in one line: "反例、许可和风险必须随机器结果返回，防止 Agent 为了减少
//! Token 无条件套用不适合的算法". Every answer of this crate therefore carries the counterexamples,
//! the license and the risks of the assets it names - a caller that drops them drops the safety of
//! the answer, so the tools never build an answer without them.
//!
//! # The errors
//!
//! * an unknown tool is refused with [`tase_contract::ErrorCode::NotFound`];
//! * an argument that is missing, that has the wrong type or that the schema does not name is
//!   refused with [`tase_contract::ErrorCode::InvalidArgument`];
//! * a capability that the caller does not have - the instantiation of an asset that has no
//!   APPROVED implementation in the target language - is refused with [`tase_contract::ErrorCode::PolicyRejected`].

use pime_core::json::Json;
use tase_catalog::{ComplexityBudget, Constraints, Layer};
use tase_contract::TaseError;

use crate::schema::{DESCRIBE_SCHEMA, INSTANTIATE_SCHEMA, RECOMMEND_SCHEMA, SEARCH_SCHEMA};

/// The number of the answers when the caller does not ask for a number.
pub const DEFAULT_LIMIT: usize = 10;

/// The largest number of the answers the tools give.
pub const MAX_LIMIT: usize = 50;

/// The name of the tool that finds the cards.
pub const SEARCH: &str = "catalog.search";
/// The name of the tool that ranks the candidates.
pub const RECOMMEND: &str = "catalog.recommend";
/// The name of the tool that drafts a plan.
pub const INSTANTIATE: &str = "catalog.instantiate";
/// The name of the tool that gives the whole card of an asset.
pub const DESCRIBE: &str = "catalog.describe";

/// The names of the four tools, in the order they are published.
pub const TOOL_NAMES: [&str; 4] = [SEARCH, RECOMMEND, INSTANTIATE, DESCRIBE];

/// The text that tells the model what this server is for.
///
/// It says what the tools do, when they are to be used, and **what has to be read carefully** in
/// the answers: the counterexamples, the license and the risks. It stays well below the 4096
/// characters an MCP host accepts.
pub const INSTRUCTIONS: &str = concat!(
    "This server is the unified standard asset catalog of TASE: 93 knowledge cards of design patterns and ",
    "algorithms, each with its manifest, its card, its counterexamples, its license and its risks.\n\n",
    "Use catalog.search to find the cards that answer a question (\"a stable sort that keeps the order of ",
    "the equal keys\"), catalog.recommend when you know what you want to build and want the ranked candidates ",
    "with the breakdown of their score, and catalog.describe for the whole card of one asset. Use ",
    "catalog.instantiate only when you are sure of the asset and of the version: it is refused with ",
    "POLICY_REJECTED unless the asset has an APPROVED implementation in the language you name, because a ",
    "knowledge card may be read and quoted but not instantiated.\n\n",
    "Read the counterexamples, the license and the risks of every candidate before you use it. A card that ",
    "declares that its order is not stable, that it keeps a global mutable state, that it is not thread safe ",
    "or that it may need negative inputs will break the code you write. A candidate that carries no ",
    "implementation is a knowledge card: read it, quote it, and do not pretend that it can be instantiated.\n\n",
    "Every tool is read-only. Nothing here writes a file: catalog.instantiate answers a plan draft, and ",
    "applying a plan is another tool."
);

/// The declaration of one MCP tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolDefinition {
    /// The name of the tool (`catalog.search`).
    pub name: &'static str,
    /// What the tool does and when it is to be used (the model reads it).
    pub description: &'static str,
    /// The JSON Schema of the arguments of the tool, as it is published.
    pub input_schema: &'static str,
}

/// The four tools of the catalog, in the order they are published.
pub fn tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: SEARCH,
            description: "Find the knowledge cards of the catalog by keyword, language, layer, category and tag. Use it when you need to know whether the catalog holds an algorithm or a pattern at all. It writes no file. Every hit carries its counterexamples, its license and its risks - read them before you rely on a hit.",
            input_schema: SEARCH_SCHEMA,
        },
        ToolDefinition {
            name: RECOMMEND,
            description: "Rank the assets that answer an intent in a target language, with the breakdown of the score of every candidate. Use it when you know what you want to build. The constraints you state refuse the assets that do not fit them, and the refusal says why. It writes no file. Every candidate carries its counterexamples, its license and its risks.",
            input_schema: RECOMMEND_SCHEMA,
        },
        ToolDefinition {
            name: INSTANTIATE,
            description: "Draft the plan of an instantiation of an asset in a target language. Use it only when you are sure of the asset and of the version, and only to see the plan: it writes no file and applies nothing. It is refused with POLICY_REJECTED unless the asset has an APPROVED implementation in the language you name.",
            input_schema: INSTANTIATE_SCHEMA,
        },
        ToolDefinition {
            name: DESCRIBE,
            description: "Give the whole knowledge card of one asset: the body of its card.md, its manifest, its state and the state of its implementations. Use it to read a candidate in full, or to check what an id or an alias stands for. It writes no file.",
            input_schema: DESCRIBE_SCHEMA,
        },
    ]
}

/// The query of a search of the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// The words to look for.
    pub query: String,
    /// The language of the implementation the caller cares about.
    pub language: String,
    /// Only the assets of this layer.
    pub layer: Option<String>,
    /// Only the assets of this category.
    pub category: Option<String>,
    /// Only the assets that carry this tag.
    pub tag: Option<String>,
    /// How many cards to answer with at most.
    pub limit: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            query: String::new(),
            language: String::new(),
            layer: None,
            category: None,
            tag: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// The summary of an asset, as the search answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetSummary {
    /// The id of the asset.
    pub asset_id: String,
    /// The version of the asset.
    pub version: String,
    /// The logical layer of the asset.
    pub layer: String,
    /// The category of the asset.
    pub category: String,
    /// The name of the asset in Chinese.
    pub name_zh: Option<String>,
    /// The name of the asset in English.
    pub name_en: Option<String>,
    /// The other names of the asset (the search keys).
    pub aliases: Vec<String>,
    /// The state of the knowledge card (`DOCUMENTED` or `REVIEWED`).
    pub status: String,
    /// What the asset is for.
    pub intent: String,
    /// The license of the asset, when the manifest declares one.
    pub license: Option<String>,
    /// The declared time cost of the asset.
    pub complexity_time: Option<String>,
    /// The declared extra space of the asset.
    pub complexity_space: Option<String>,
    /// Whether the asset may be instantiated in the language of the query.
    pub instantiable: bool,
    /// Why the asset may (or may not) be instantiated.
    pub instantiable_reasons: Vec<String>,
    /// The cases the card does not solve.
    pub counterexamples: Vec<String>,
    /// The risks the card declares.
    pub risks: Vec<String>,
    /// The assets the card names as its alternatives.
    pub alternatives: Vec<String>,
    /// The digest of the content of the asset.
    pub digest: String,
}

/// One part of the score of a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScorePart {
    /// The name of the part (`intent`, `constraints`, `complexity` or `implementation`).
    pub name: String,
    /// The points of the part.
    pub points: i32,
    /// The weight of the part.
    pub weight: i32,
}

/// A candidate of a recommendation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecommendedAsset {
    /// The id of the asset.
    pub asset_id: String,
    /// The version of the asset.
    pub version: String,
    /// The logical layer of the asset.
    pub layer: String,
    /// The score of the candidate.
    pub score: i32,
    /// The breakdown of the score.
    pub breakdown: Vec<ScorePart>,
    /// Where the confidence of the score comes from (`rule` or `verified_benchmark`).
    pub confidence_source: String,
    /// Whether the asset may be instantiated in the requested language.
    pub instantiable: bool,
    /// The id of the approved implementation, when there is one.
    pub implementation_id: Option<String>,
    /// The conditions the card declares.
    pub requirements: Vec<(String, String)>,
    /// Where the evidence of the implementation lives, when there is one.
    pub proof_evidence: Option<String>,
    /// Why the asset may (or may not) be instantiated.
    pub reasons: Vec<String>,
    /// The cases the card does not solve.
    pub counterexamples: Vec<String>,
    /// The risks the card declares.
    pub risks: Vec<String>,
    /// The explanation of the score.
    pub notes: Vec<String>,
    /// The license of the asset.
    pub license: Option<String>,
    /// The declared time cost of the asset.
    pub complexity_time: Option<String>,
    /// The declared extra space of the asset.
    pub complexity_space: Option<String>,
    /// The assets the card names as its alternatives.
    pub alternatives: Vec<String>,
}

/// A candidate that was not recommended, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedAsset {
    /// The id of the asset.
    pub asset_id: String,
    /// The reason of the refusal.
    pub reason: String,
    /// The refusal, in one line.
    pub detail: String,
}

/// The query of a recommendation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecommendQuery {
    /// What the caller wants to do, in words.
    pub intent: String,
    /// The language of the implementation the caller wants.
    pub language: String,
    /// The conditions the caller imposes.
    pub constraints: Constraints,
    /// Only the assets of this layer.
    pub layer: Option<String>,
    /// Only the assets of this category.
    pub category: Option<String>,
    /// How many candidates to answer with at most.
    pub limit: usize,
}

impl Default for RecommendQuery {
    fn default() -> Self {
        RecommendQuery {
            intent: String::new(),
            language: String::new(),
            constraints: Constraints::default(),
            layer: None,
            category: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// The state of one implementation of an asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplementationSummary {
    /// The id of the implementation.
    pub implementation_id: String,
    /// The language of the implementation.
    pub language: String,
    /// The state of the implementation (`PLANNED` ... `REVOKED`).
    pub status: String,
    /// The version of the implementation.
    pub version: String,
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

/// The whole knowledge card of an asset.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetDetail {
    /// The summary of the asset.
    pub summary: AssetSummary,
    /// The body of the `card.md` of the asset.
    pub card_markdown: String,
    /// The manifest of the asset.
    pub manifest: Json,
    /// The state of the knowledge card.
    pub card_status: String,
    /// Whether the asset may be recommended in the requested language.
    pub recommendable: bool,
    /// Whether the asset may be instantiated in the requested language.
    pub instantiable: bool,
    /// Why the asset may (or may not) be recommended and instantiated.
    pub governance_reasons: Vec<String>,
    /// The implementations of the asset.
    pub implementations: Vec<ImplementationSummary>,
    /// What the caller has to know about the answer itself (a card that could not be read, ...).
    pub notes: Vec<String>,
}

/// The request of an instantiation.
#[derive(Clone, Debug, PartialEq)]
pub struct InstantiateRequest {
    /// The id of the asset.
    pub asset_id: String,
    /// The version of the asset.
    pub version: String,
    /// The language of the implementation to instantiate.
    pub language: String,
    /// The parameters of the instantiation.
    pub parameters: Json,
    /// The licenses the caller accepts.
    pub allowed_licenses: Vec<String>,
}

/// The draft of a plan: what an instantiation would produce.
///
/// The draft is **not** a plan of the repository and it writes nothing: it is what a caller shows
/// before it asks for an approval.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanDraft {
    /// The id of the asset.
    pub asset_id: String,
    /// The version of the asset.
    pub version: String,
    /// The language of the implementation.
    pub language: String,
    /// The id of the implementation the draft is built out of.
    pub implementation_id: String,
    /// The parameters of the instantiation.
    pub parameters: Json,
    /// The steps of the draft.
    pub steps: Vec<String>,
    /// What the caller has to know before the draft is applied.
    pub warnings: Vec<String>,
    /// The version of the contract the draft speaks.
    pub contract_version: String,
}

/// The service the tools call: the seam between the MCP surface and the catalog.
///
/// The trait knows nothing about MCP and nothing about the catalog: it is the four questions the
/// tools ask. The adapter of `tase-catalog` implements it, and a test may implement it with a fake
/// service.
pub trait CatalogService: Send + Sync {
    /// The cards that answer the query.
    fn search(&self, q: &SearchQuery) -> Result<Vec<AssetSummary>, TaseError>;

    /// The candidates that answer the request, the best one first.
    fn recommend(&self, r: &RecommendQuery) -> Result<Vec<RecommendedAsset>, TaseError>;

    /// The whole knowledge card of one asset, by its id, its name or its alias.
    fn describe(&self, id: &str) -> Result<AssetDetail, TaseError>;

    /// The draft of the plan of an instantiation.
    ///
    /// The service refuses the request with [`tase_contract::ErrorCode::PolicyRejected`] when the asset has no
    /// APPROVED implementation in the requested language (requirements §14.4).
    fn instantiate(&self, req: &InstantiateRequest) -> Result<PlanDraft, TaseError>;

    /// The candidates that were refused, when the service keeps them.
    ///
    /// The answer of a recommendation of detailed design §19.2 holds the candidates that were not
    /// recommended with the reason of the refusal (`rejected=[{asset_id,reason}]`). A service that
    /// does not keep them answers an empty list.
    fn refused(&self, _query: &RecommendQuery) -> Vec<RefusedAsset> {
        Vec::new()
    }
}

/// The tools of the catalog, bound to the service they call.
pub struct CatalogTools {
    /// The service the tools call.
    pub catalog: Box<dyn CatalogService>,
}

impl CatalogTools {
    /// Calls a tool by its name.
    ///
    /// * an unknown tool is refused with [`tase_contract::ErrorCode::NotFound`];
    /// * an argument that is missing, that has the wrong type or that the schema of the tool does
    ///   not name is refused with [`tase_contract::ErrorCode::InvalidArgument`];
    /// * a failure of the service is passed on as it is.
    pub fn call(&self, name: &str, arguments: &Json) -> Result<Json, TaseError> {
        match name {
            SEARCH => self.search(arguments),
            RECOMMEND => self.recommend(arguments),
            INSTANTIATE => self.instantiate(arguments),
            DESCRIBE => self.describe(arguments),
            other => Err(TaseError::not_found(format!(
                "there is no tool named {other:?}: the tools are {TOOL_NAMES:?}"
            ))),
        }
    }

    /// The answer of `catalog.search`.
    fn search(&self, arguments: &Json) -> Result<Json, TaseError> {
        let fields = fields_of(arguments)?;
        refuse_unknown(
            fields,
            &["query", "language", "layer", "category", "tag", "limit"],
            SEARCH,
        )?;
        let query = SearchQuery {
            query: required_str(fields, "query")?,
            language: required_str(fields, "language")?,
            layer: layer_of(fields)?,
            category: optional_str(fields, "category")?,
            tag: optional_str(fields, "tag")?,
            limit: limit_of(fields)?,
        };
        let assets = self.catalog.search(&query)?;
        let count = assets.len();
        Ok(Json::obj(vec![
            ("ok", Json::Bool(true)),
            ("tool", Json::str(SEARCH)),
            ("query", Json::str(&query.query)),
            ("language", Json::str(&query.language)),
            ("count", Json::int(count as i64)),
            (
                "assets",
                Json::Arr(assets.iter().map(asset_summary_json).collect()),
            ),
        ]))
    }

    /// The answer of `catalog.recommend`.
    fn recommend(&self, arguments: &Json) -> Result<Json, TaseError> {
        let fields = fields_of(arguments)?;
        refuse_unknown(
            fields,
            &[
                "intent",
                "language",
                "layer",
                "category",
                "limit",
                "constraints",
            ],
            RECOMMEND,
        )?;
        let query = RecommendQuery {
            intent: required_str(fields, "intent")?,
            language: required_str(fields, "language")?,
            constraints: constraints_of(fields)?,
            layer: layer_of(fields)?,
            category: optional_str(fields, "category")?,
            limit: limit_of(fields)?,
        };
        let recommended = self.catalog.recommend(&query)?;
        let refused = self.catalog.refused(&query);
        let count = recommended.len();
        Ok(Json::obj(vec![
            ("ok", Json::Bool(true)),
            ("tool", Json::str(RECOMMEND)),
            ("intent", Json::str(&query.intent)),
            ("language", Json::str(&query.language)),
            ("count", Json::int(count as i64)),
            (
                "recommended",
                Json::Arr(recommended.iter().map(recommended_json).collect()),
            ),
            (
                "rejected",
                Json::Arr(refused.iter().map(refused_json).collect()),
            ),
        ]))
    }

    /// The answer of `catalog.instantiate`.
    fn instantiate(&self, arguments: &Json) -> Result<Json, TaseError> {
        let fields = fields_of(arguments)?;
        refuse_unknown(
            fields,
            &[
                "asset_id",
                "version",
                "language",
                "parameters",
                "allowed_licenses",
            ],
            INSTANTIATE,
        )?;
        let request = InstantiateRequest {
            asset_id: required_str(fields, "asset_id")?,
            version: required_str(fields, "version")?,
            language: required_str(fields, "language")?,
            parameters: parameters_of(fields)?,
            allowed_licenses: optional_strings(fields, "allowed_licenses")?,
        };
        let plan = self.catalog.instantiate(&request)?;
        Ok(Json::obj(vec![
            ("ok", Json::Bool(true)),
            ("tool", Json::str(INSTANTIATE)),
            ("plan", plan_json(&plan)),
        ]))
    }

    /// The answer of `catalog.describe`.
    fn describe(&self, arguments: &Json) -> Result<Json, TaseError> {
        let fields = fields_of(arguments)?;
        refuse_unknown(fields, &["asset_id"], DESCRIBE)?;
        let asset_id = required_str(fields, "asset_id")?;
        let detail = self.catalog.describe(&asset_id)?;
        Ok(Json::obj(vec![
            ("ok", Json::Bool(true)),
            ("tool", Json::str(DESCRIBE)),
            ("asset", asset_detail_json(&detail)),
        ]))
    }
}

/// The fields of the arguments of a call.
///
/// A missing object is an empty one - the tool then refuses the field it needs - and another shape
/// is refused right away.
fn fields_of(arguments: &Json) -> Result<&[(String, Json)], TaseError> {
    match arguments {
        Json::Obj(pairs) => Ok(pairs.as_slice()),
        Json::Null => Ok(&[]),
        _ => Err(TaseError::invalid_argument(
            "the arguments of a tool must be an object",
        )),
    }
}

/// Refuses the fields the schema of the tool does not name.
fn refuse_unknown(
    fields: &[(String, Json)],
    allowed: &[&str],
    tool: &str,
) -> Result<(), TaseError> {
    for (key, _) in fields {
        if !allowed.contains(&key.as_str()) {
            return Err(TaseError::invalid_argument(format!(
                "the tool {tool} has no parameter named {key:?}"
            )));
        }
    }
    Ok(())
}

/// A string field that must be there and must not be empty.
fn required_str(fields: &[(String, Json)], key: &str) -> Result<String, TaseError> {
    let value = fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
        .ok_or_else(|| {
            TaseError::invalid_argument(format!("the parameter {key:?} is required"))
        })?;
    let text = value.as_str().ok_or_else(|| {
        TaseError::invalid_argument(format!("the parameter {key:?} must be a string"))
    })?;
    if text.trim().is_empty() {
        return Err(TaseError::invalid_argument(format!(
            "the parameter {key:?} must not be empty"
        )));
    }
    Ok(text.to_string())
}

/// A string field that may be missing.
fn optional_str(fields: &[(String, Json)], key: &str) -> Result<Option<String>, TaseError> {
    match fields.iter().find(|(name, _)| name == key) {
        None | Some((_, Json::Null)) => Ok(None),
        Some((_, value)) => {
            let text = value.as_str().ok_or_else(|| {
                TaseError::invalid_argument(format!("the parameter {key:?} must be a string"))
            })?;
            Ok(Some(text.to_string()))
        }
    }
}

/// The layer of a call, when it is named.
///
/// The name is checked against the six layers of the catalog: an unknown layer is refused instead
/// of being silently ignored.
fn layer_of(fields: &[(String, Json)]) -> Result<Option<String>, TaseError> {
    let Some(text) = optional_str(fields, "layer")? else {
        return Ok(None);
    };
    let name = text.trim().to_uppercase();
    let layer = Layer::from_name(&name).ok_or_else(|| {
        TaseError::invalid_argument(format!("the layer {text:?} is not one of the six layers"))
    })?;
    Ok(Some(layer.as_str().to_string()))
}

/// The limit of a call: the number of the answers.
fn limit_of(fields: &[(String, Json)]) -> Result<usize, TaseError> {
    match fields.iter().find(|(name, _)| name == "limit") {
        None | Some((_, Json::Null)) => Ok(DEFAULT_LIMIT),
        Some((_, value)) => {
            let number = value.as_i64().ok_or_else(|| {
                TaseError::invalid_argument("the parameter \"limit\" must be an integer")
            })?;
            if number < 1 || number > MAX_LIMIT as i64 {
                return Err(TaseError::invalid_argument(format!(
                    "the limit {number} is out of the range 1..={MAX_LIMIT}"
                )));
            }
            Ok(number as usize)
        }
    }
}

/// A boolean field that may be missing.
fn optional_bool(
    fields: &[(String, Json)],
    key: &str,
    default: bool,
) -> Result<bool, TaseError> {
    match fields.iter().find(|(name, _)| name == key) {
        None | Some((_, Json::Null)) => Ok(default),
        Some((_, value)) => value.as_bool().ok_or_else(|| {
            TaseError::invalid_argument(format!("the parameter {key:?} must be a boolean"))
        }),
    }
}

/// A list of strings that may be missing.
fn optional_strings(fields: &[(String, Json)], key: &str) -> Result<Vec<String>, TaseError> {
    match fields.iter().find(|(name, _)| name == key) {
        None | Some((_, Json::Null)) => Ok(Vec::new()),
        Some((_, value)) => {
            let items = value.as_array().ok_or_else(|| {
                TaseError::invalid_argument(format!("the parameter {key:?} must be an array"))
            })?;
            items
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_string).ok_or_else(|| {
                        TaseError::invalid_argument(format!(
                            "the parameter {key:?} must only hold strings"
                        ))
                    })
                })
                .collect()
        }
    }
}

/// The conditions of a recommendation.
fn constraints_of(fields: &[(String, Json)]) -> Result<Constraints, TaseError> {
    let Some((_, value)) = fields.iter().find(|(name, _)| name == "constraints") else {
        return Ok(Constraints::default());
    };
    if value.is_null() {
        return Ok(Constraints::default());
    }
    let inner = value.as_object().ok_or_else(|| {
        TaseError::invalid_argument("the parameter \"constraints\" must be an object")
    })?;
    refuse_unknown(
        inner,
        &[
            "max_complexity",
            "allow_global_mutable_state",
            "require_thread_safety",
            "allow_negative_inputs",
            "require_stable_order",
            "require_instantiable",
            "allowed_licenses",
        ],
        RECOMMEND,
    )?;
    let max_complexity = match optional_str(inner, "max_complexity")? {
        None => None,
        Some(text) => Some(budget_of(&text)?),
    };
    Ok(Constraints {
        max_complexity,
        allow_global_mutable_state: optional_bool(inner, "allow_global_mutable_state", true)?,
        require_thread_safety: optional_bool(inner, "require_thread_safety", false)?,
        allow_negative_inputs: optional_bool(inner, "allow_negative_inputs", true)?,
        require_stable_order: optional_bool(inner, "require_stable_order", false)?,
        require_instantiable: optional_bool(inner, "require_instantiable", false)?,
        allowed_licenses: optional_strings(inner, "allowed_licenses")?,
    })
}

/// The budget of the caller, from its name on the wire.
fn budget_of(text: &str) -> Result<ComplexityBudget, TaseError> {
    match text.trim().to_uppercase().as_str() {
        "LOGARITHMIC" => Ok(ComplexityBudget::Logarithmic),
        "LINEAR" => Ok(ComplexityBudget::Linear),
        "LINEARITHMIC" => Ok(ComplexityBudget::Linearithmic),
        "QUADRATIC" => Ok(ComplexityBudget::Quadratic),
        "UNBOUNDED" => Ok(ComplexityBudget::Unbounded),
        other => Err(TaseError::invalid_argument(format!(
            "the complexity budget {other:?} is not one of the five budgets"
        ))),
    }
}

/// The parameters of an instantiation: an object, empty when it is not given.
fn parameters_of(fields: &[(String, Json)]) -> Result<Json, TaseError> {
    match fields.iter().find(|(name, _)| name == "parameters") {
        None | Some((_, Json::Null)) => Ok(Json::Obj(Vec::new())),
        Some((_, value)) => {
            if value.as_object().is_none() {
                return Err(TaseError::invalid_argument(
                    "the parameter \"parameters\" must be an object",
                ));
            }
            Ok(value.clone())
        }
    }
}

/// The JSON of a summary of an asset.
fn asset_summary_json(summary: &AssetSummary) -> Json {
    Json::obj(vec![
        ("asset_id", Json::str(&summary.asset_id)),
        ("version", Json::str(&summary.version)),
        ("layer", Json::str(&summary.layer)),
        ("category", Json::str(&summary.category)),
        ("name_zh", optional_json(summary.name_zh.as_deref())),
        ("name_en", optional_json(summary.name_en.as_deref())),
        ("aliases", Json::strs(&summary.aliases)),
        ("status", Json::str(&summary.status)),
        ("intent", Json::str(&summary.intent)),
        ("license", optional_json(summary.license.as_deref())),
        (
            "complexity",
            complexity_json(
                summary.complexity_time.as_deref(),
                summary.complexity_space.as_deref(),
            ),
        ),
        ("instantiable", Json::Bool(summary.instantiable)),
        (
            "instantiable_reasons",
            Json::strs(&summary.instantiable_reasons),
        ),
        ("counterexamples", Json::strs(&summary.counterexamples)),
        ("risks", Json::strs(&summary.risks)),
        ("alternatives", Json::strs(&summary.alternatives)),
        ("digest", Json::str(&summary.digest)),
    ])
}

/// The JSON of the cost of an asset.
fn complexity_json(time: Option<&str>, space: Option<&str>) -> Json {
    match (time, space) {
        (None, None) => Json::Null,
        (time, space) => Json::obj(vec![
            ("time", optional_json(time)),
            ("extra_space", optional_json(space)),
        ]),
    }
}

/// The JSON of one part of a score.
fn score_part_json(part: &ScorePart) -> Json {
    Json::obj(vec![
        ("name", Json::str(&part.name)),
        ("points", Json::int(part.points as i64)),
        ("weight", Json::int(part.weight as i64)),
    ])
}

/// The JSON of a candidate of a recommendation.
fn recommended_json(asset: &RecommendedAsset) -> Json {
    Json::obj(vec![
        ("asset_id", Json::str(&asset.asset_id)),
        ("version", Json::str(&asset.version)),
        ("layer", Json::str(&asset.layer)),
        ("score", Json::int(asset.score as i64)),
        (
            "breakdown",
            Json::Arr(asset.breakdown.iter().map(score_part_json).collect()),
        ),
        ("confidence_source", Json::str(&asset.confidence_source)),
        ("instantiable", Json::Bool(asset.instantiable)),
        (
            "implementation_id",
            optional_json(asset.implementation_id.as_deref()),
        ),
        (
            "requirements",
            Json::Arr(
                asset
                    .requirements
                    .iter()
                    .map(|(name, value)| {
                        Json::obj(vec![
                            ("name", Json::str(name)),
                            ("value", Json::str(value)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("proof_evidence", optional_json(asset.proof_evidence.as_deref())),
        ("reasons", Json::strs(&asset.reasons)),
        ("counterexamples", Json::strs(&asset.counterexamples)),
        ("risks", Json::strs(&asset.risks)),
        ("notes", Json::strs(&asset.notes)),
        ("license", optional_json(asset.license.as_deref())),
        (
            "complexity",
            complexity_json(
                asset.complexity_time.as_deref(),
                asset.complexity_space.as_deref(),
            ),
        ),
        ("alternatives", Json::strs(&asset.alternatives)),
    ])
}

/// The JSON of a refused candidate.
fn refused_json(refused: &RefusedAsset) -> Json {
    Json::obj(vec![
        ("asset_id", Json::str(&refused.asset_id)),
        ("reason", Json::str(&refused.reason)),
        ("detail", Json::str(&refused.detail)),
    ])
}

/// The JSON of the state of one implementation.
fn implementation_json(implementation: &ImplementationSummary) -> Json {
    Json::obj(vec![
        (
            "implementation_id",
            Json::str(&implementation.implementation_id),
        ),
        ("language", Json::str(&implementation.language)),
        ("status", Json::str(&implementation.status)),
        ("version", Json::str(&implementation.version)),
        ("engine", Json::str(&implementation.engine)),
        ("path", optional_json(implementation.path.as_deref())),
        ("sha256", optional_json(implementation.sha256.as_deref())),
        (
            "parameters_schema",
            optional_json(implementation.parameters_schema.as_deref()),
        ),
        (
            "tests_manifest",
            optional_json(implementation.tests_manifest.as_deref()),
        ),
    ])
}

/// The JSON of the whole knowledge card of an asset.
fn asset_detail_json(detail: &AssetDetail) -> Json {
    Json::obj(vec![
        ("summary", asset_summary_json(&detail.summary)),
        ("card_markdown", Json::str(&detail.card_markdown)),
        ("manifest", detail.manifest.clone()),
        ("card_status", Json::str(&detail.card_status)),
        ("recommendable", Json::Bool(detail.recommendable)),
        ("instantiable", Json::Bool(detail.instantiable)),
        (
            "governance_reasons",
            Json::strs(&detail.governance_reasons),
        ),
        (
            "implementations",
            Json::Arr(
                detail
                    .implementations
                    .iter()
                    .map(implementation_json)
                    .collect(),
            ),
        ),
        ("notes", Json::strs(&detail.notes)),
    ])
}

/// The JSON of a draft of a plan.
fn plan_json(plan: &PlanDraft) -> Json {
    Json::obj(vec![
        ("asset_id", Json::str(&plan.asset_id)),
        ("version", Json::str(&plan.version)),
        ("language", Json::str(&plan.language)),
        (
            "implementation_id",
            Json::str(&plan.implementation_id),
        ),
        ("parameters", plan.parameters.clone()),
        ("steps", Json::strs(&plan.steps)),
        ("warnings", Json::strs(&plan.warnings)),
        ("contract_version", Json::str(&plan.contract_version)),
    ])
}

/// A string that may be missing, as the JSON of it.
fn optional_json(text: Option<&str>) -> Json {
    match text {
        Some(text) => Json::str(text),
        None => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::schema_of;
    use std::sync::{Arc, Mutex};
    use tase_contract::ErrorCode;

    /// What the fake service was asked.
    #[derive(Default)]
    struct Recorded {
        search: Option<SearchQuery>,
        recommend: Option<RecommendQuery>,
        instantiate: Option<InstantiateRequest>,
    }

    /// The service of the tests: it answers what the test puts into it and it records what it was
    /// asked.
    struct FakeService {
        assets: Vec<AssetSummary>,
        candidates: Vec<RecommendedAsset>,
        refusals: Vec<RefusedAsset>,
        detail: Option<AssetDetail>,
        detail_error: Option<TaseError>,
        plan: Option<PlanDraft>,
        recorded: Arc<Mutex<Recorded>>,
    }

    impl FakeService {
        /// A service that answers nothing.
        fn empty() -> FakeService {
            FakeService {
                assets: Vec::new(),
                candidates: Vec::new(),
                refusals: Vec::new(),
                detail: None,
                detail_error: None,
                plan: None,
                recorded: Arc::new(Mutex::new(Recorded::default())),
            }
        }

        /// The handle of the record of the service.
        fn recorder(&self) -> Arc<Mutex<Recorded>> {
            Arc::clone(&self.recorded)
        }
    }

    impl CatalogService for FakeService {
        fn search(&self, q: &SearchQuery) -> Result<Vec<AssetSummary>, TaseError> {
            self.recorded.lock().expect("the recorder is not poisoned").search = Some(q.clone());
            Ok(self.assets.clone())
        }

        fn recommend(&self, r: &RecommendQuery) -> Result<Vec<RecommendedAsset>, TaseError> {
            self.recorded.lock().expect("the recorder is not poisoned").recommend = Some(r.clone());
            Ok(self.candidates.clone())
        }

        fn describe(&self, _id: &str) -> Result<AssetDetail, TaseError> {
            match (&self.detail, &self.detail_error) {
                (_, Some(error)) => Err(error.clone()),
                (Some(detail), None) => Ok(detail.clone()),
                (None, None) => Err(TaseError::not_found("there is no asset")),
            }
        }

        fn instantiate(&self, req: &InstantiateRequest) -> Result<PlanDraft, TaseError> {
            self.recorded.lock().expect("the recorder is not poisoned").instantiate = Some(req.clone());
            match &self.plan {
                Some(plan) => Ok(plan.clone()),
                None => Err(TaseError::policy_rejected(
                    "the asset has no approved implementation in the requested language",
                )),
            }
        }

        fn refused(&self, _query: &RecommendQuery) -> Vec<RefusedAsset> {
            self.refusals.clone()
        }
    }

    /// The tools of a fake service.
    fn tools_of(service: FakeService) -> CatalogTools {
        CatalogTools {
            catalog: Box::new(service),
        }
    }

    /// The arguments of a call.
    fn args(pairs: Vec<(&str, Json)>) -> Json {
        Json::obj(pairs)
    }

    /// The summary of an asset.
    fn summary(id: &str) -> AssetSummary {
        AssetSummary {
            asset_id: id.to_string(),
            version: "1.0.0".to_string(),
            layer: "ALGORITHM".to_string(),
            category: "graph".to_string(),
            name_zh: Some("最小生成树".to_string()),
            name_en: Some("Minimum Spanning Tree".to_string()),
            aliases: vec!["MST".to_string()],
            status: "DOCUMENTED".to_string(),
            intent: "求出无向带权图的最小生成树".to_string(),
            license: Some("MIT".to_string()),
            complexity_time: Some("O(E log E)".to_string()),
            complexity_space: Some("O(V + E)".to_string()),
            instantiable: false,
            instantiable_reasons: vec!["the asset is a knowledge card".to_string()],
            counterexamples: vec!["有向图——最小生成树无定义".to_string()],
            risks: vec!["the card declares no counterexample".to_string()],
            alternatives: vec!["algorithm.graph.prim".to_string()],
            digest: "sha256:abc".to_string(),
        }
    }

    /// A candidate that carries a counterexample and a risk.
    fn candidate() -> RecommendedAsset {
        RecommendedAsset {
            asset_id: "pattern.gof.creational.singleton".to_string(),
            version: "1.0.0".to_string(),
            layer: "PATTERN".to_string(),
            score: 72,
            breakdown: vec![ScorePart {
                name: "intent".to_string(),
                points: 60,
                weight: 60,
            }],
            confidence_source: "rule".to_string(),
            instantiable: false,
            implementation_id: None,
            requirements: vec![("state".to_string(), "mutable_global_requires_locks".to_string())],
            proof_evidence: None,
            reasons: vec!["the asset has no implementation in rust".to_string()],
            counterexamples: vec!["多进程下无法保证唯一实例".to_string()],
            risks: vec!["the card declares that the asset keeps a global mutable state".to_string()],
            notes: vec!["intent: 60 of 60".to_string()],
            license: Some("MIT".to_string()),
            complexity_time: None,
            complexity_space: None,
            alternatives: vec!["pattern.arch.dependency_injection".to_string()],
        }
    }

    /// The whole card of an asset.
    fn detail() -> AssetDetail {
        AssetDetail {
            summary: summary("algorithm.graph.kruskal"),
            card_markdown: "# Kruskal\n\n贪心加边。\n".to_string(),
            manifest: args(vec![("asset_id", Json::str("algorithm.graph.kruskal"))]),
            card_status: "DOCUMENTED".to_string(),
            recommendable: true,
            instantiable: false,
            governance_reasons: vec!["the asset is a knowledge card".to_string()],
            implementations: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn the_tools_are_four_and_their_order_is_fixed() {
        let list = tools();
        assert_eq!(list.len(), 4);
        let names: Vec<&str> = list.iter().map(|tool| tool.name).collect();
        assert_eq!(names, ["catalog.search", "catalog.recommend", "catalog.instantiate", "catalog.describe"]);
        assert_eq!(names, TOOL_NAMES);
    }

    #[test]
    fn every_tool_carries_a_description_and_the_schema_of_its_name() {
        for tool in tools() {
            assert!(!tool.description.is_empty(), "{}", tool.name);
            assert!(tool.description.len() < 1024, "{}", tool.name);
            assert_eq!(
                tool.input_schema,
                schema_of(tool.name).unwrap_or_else(|| panic!("{} must have a schema", tool.name))
            );
            assert!(tool.input_schema.contains("2020-12"), "{}", tool.name);
        }
    }

    #[test]
    fn an_unknown_tool_is_refused_with_not_found() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call("catalog.nothing", &Json::Obj(Vec::new()))
            .expect_err("there is no such tool");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("catalog.nothing"), "{}", error.message);
    }

    #[test]
    fn an_empty_tool_name_is_refused_with_not_found() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call("", &Json::Obj(Vec::new()))
            .expect_err("there is no such tool");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_missing_parameter_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(SEARCH, &args(vec![("language", Json::str("rust"))]))
            .expect_err("the query is required");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("query"), "{}", error.message);
    }

    #[test]
    fn a_parameter_of_the_wrong_type_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                SEARCH,
                &args(vec![("query", Json::int(3)), ("language", Json::str("rust"))]),
            )
            .expect_err("the query must be a string");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("string"), "{}", error.message);
    }

    #[test]
    fn an_empty_parameter_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                SEARCH,
                &args(vec![("query", Json::str("  ")), ("language", Json::str("rust"))]),
            )
            .expect_err("the query must not be empty");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_parameter_the_schema_does_not_name_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                    ("sort", Json::str("by name")),
                ]),
            )
            .expect_err("the search has no parameter named sort");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("sort"), "{}", error.message);
    }

    #[test]
    fn a_limit_out_of_the_range_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        for limit in [0, 51, -1] {
            let error = tools
                .call(
                    SEARCH,
                    &args(vec![
                        ("query", Json::str("MST")),
                        ("language", Json::str("rust")),
                        ("limit", Json::int(limit)),
                    ]),
                )
                .expect_err("the limit is out of the range");
            assert_eq!(error.code, ErrorCode::InvalidArgument);
        }
    }

    #[test]
    fn a_limit_that_is_not_an_integer_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                    ("limit", Json::str("ten")),
                ]),
            )
            .expect_err("the limit must be an integer");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_layer_that_is_not_one_of_the_six_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                    ("layer", Json::str("RECIPE")),
                ]),
            )
            .expect_err("the layer is unknown");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("RECIPE"), "{}", error.message);
    }

    #[test]
    fn arguments_that_are_not_an_object_are_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        for arguments in [Json::Arr(Vec::new()), Json::str("MST"), Json::int(1)] {
            let error = tools
                .call(SEARCH, &arguments)
                .expect_err("the arguments must be an object");
            assert_eq!(error.code, ErrorCode::InvalidArgument);
        }
    }

    #[test]
    fn arguments_that_are_missing_are_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(SEARCH, &Json::Null)
            .expect_err("the query is required");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_search_answers_the_cards_of_the_service() {
        let mut service = FakeService::empty();
        service.assets = vec![summary("algorithm.graph.kruskal")];
        let tools = tools_of(service);
        let answer = tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        assert_eq!(answer.get("ok").and_then(Json::as_bool), Some(true));
        assert_eq!(answer.get("tool").and_then(Json::as_str), Some(SEARCH));
        assert_eq!(answer.get("count").and_then(Json::as_i64), Some(1));
        let assets = answer.get("assets").and_then(Json::as_array).expect("the assets");
        assert_eq!(assets.len(), 1);
        let first = &assets[0];
        assert_eq!(
            first.get("asset_id").and_then(Json::as_str),
            Some("algorithm.graph.kruskal")
        );
        assert_eq!(first.get("license").and_then(Json::as_str), Some("MIT"));
    }

    #[test]
    fn the_search_carries_the_counterexamples_and_the_risks_of_every_card() {
        let mut service = FakeService::empty();
        service.assets = vec![summary("algorithm.graph.kruskal")];
        let tools = tools_of(service);
        let answer = tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let text = answer.to_json_string();
        assert!(text.contains("有向图——最小生成树无定义"), "{text}");
        assert!(text.contains("the card declares no counterexample"), "{text}");
    }

    #[test]
    fn the_filters_of_the_search_reach_the_service() {
        let service = FakeService::empty();
        let recorded = service.recorder();
        let tools = tools_of(service);
        tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                    ("layer", Json::str("algorithm")),
                    ("category", Json::str("graph")),
                    ("tag", Json::str("stability=stable")),
                    ("limit", Json::int(7)),
                ]),
            )
            .expect("the answer");
        let query = recorded.lock().expect("the recorder is not poisoned").search.clone().expect("the query");
        assert_eq!(query.query, "MST");
        assert_eq!(query.language, "rust");
        assert_eq!(query.layer.as_deref(), Some("ALGORITHM"));
        assert_eq!(query.category.as_deref(), Some("graph"));
        assert_eq!(query.tag.as_deref(), Some("stability=stable"));
        assert_eq!(query.limit, 7);
    }

    #[test]
    fn the_search_without_a_limit_asks_for_the_default_number_of_cards() {
        let service = FakeService::empty();
        let recorded = service.recorder();
        let tools = tools_of(service);
        tools
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let query = recorded.lock().expect("the recorder is not poisoned").search.clone().expect("the query");
        assert_eq!(query.limit, DEFAULT_LIMIT);
    }

    #[test]
    fn the_recommendation_answers_the_candidates_of_the_service() {
        let mut service = FakeService::empty();
        service.candidates = vec![candidate()];
        let tools = tools_of(service);
        let answer = tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        assert_eq!(answer.get("tool").and_then(Json::as_str), Some(RECOMMEND));
        let recommended = answer
            .get("recommended")
            .and_then(Json::as_array)
            .expect("the candidates");
        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].get("score").and_then(Json::as_i64), Some(72));
    }

    #[test]
    fn the_answer_of_a_recommendation_carries_the_counterexamples_and_the_risks() {
        let mut service = FakeService::empty();
        service.candidates = vec![candidate()];
        let tools = tools_of(service);
        let answer = tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let text = answer.to_json_string();
        assert!(text.contains("多进程下无法保证唯一实例"), "{text}");
        assert!(
            text.contains("the card declares that the asset keeps a global mutable state"),
            "{text}"
        );
        assert!(text.contains("MIT"), "{text}");
    }

    #[test]
    fn the_answer_of_a_recommendation_carries_the_refusals() {
        let mut service = FakeService::empty();
        service.refusals = vec![RefusedAsset {
            asset_id: "algorithm.graph.bellman_ford".to_string(),
            reason: "NEGATIVE_INPUTS_NOT_ALLOWED".to_string(),
            detail: "the card declares that the asset may need negative inputs".to_string(),
        }];
        let tools = tools_of(service);
        let answer = tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("最短路")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let rejected = answer
            .get("rejected")
            .and_then(Json::as_array)
            .expect("the refusals");
        assert_eq!(rejected.len(), 1);
        assert_eq!(
            rejected[0].get("reason").and_then(Json::as_str),
            Some("NEGATIVE_INPUTS_NOT_ALLOWED")
        );
    }

    #[test]
    fn the_constraints_of_a_recommendation_reach_the_service() {
        let service = FakeService::empty();
        let recorded = service.recorder();
        let tools = tools_of(service);
        tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                    (
                        "constraints",
                        args(vec![
                            ("max_complexity", Json::str("LINEAR")),
                            ("allow_global_mutable_state", Json::Bool(false)),
                            ("require_thread_safety", Json::Bool(true)),
                            ("allow_negative_inputs", Json::Bool(false)),
                            ("require_stable_order", Json::Bool(true)),
                            ("require_instantiable", Json::Bool(true)),
                            ("allowed_licenses", Json::strs(&["MIT"])),
                        ]),
                    ),
                ]),
            )
            .expect("the answer");
        let query = recorded.lock().expect("the recorder is not poisoned").recommend.clone().expect("the query");
        assert_eq!(query.intent, "单例");
        assert_eq!(query.language, "rust");
        assert_eq!(
            query.constraints.max_complexity,
            Some(ComplexityBudget::Linear)
        );
        assert!(!query.constraints.allow_global_mutable_state);
        assert!(query.constraints.require_thread_safety);
        assert!(!query.constraints.allow_negative_inputs);
        assert!(query.constraints.require_stable_order);
        assert!(query.constraints.require_instantiable);
        assert_eq!(query.constraints.allowed_licenses, ["MIT"]);
    }

    #[test]
    fn a_recommendation_without_constraints_uses_the_defaults() {
        let service = FakeService::empty();
        let recorded = service.recorder();
        let tools = tools_of(service);
        tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let query = recorded.lock().expect("the recorder is not poisoned").recommend.clone().expect("the query");
        assert_eq!(query.constraints, Constraints::default());
    }

    #[test]
    fn a_constraint_that_is_not_a_boolean_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                    (
                        "constraints",
                        args(vec![("require_thread_safety", Json::str("yes"))]),
                    ),
                ]),
            )
            .expect_err("the constraint must be a boolean");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_complexity_budget_that_is_unknown_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                    (
                        "constraints",
                        args(vec![("max_complexity", Json::str("QUADRATICAL"))]),
                    ),
                ]),
            )
            .expect_err("the budget is unknown");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_constraint_the_schema_does_not_name_is_refused_with_invalid_argument() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                    (
                        "constraints",
                        args(vec![("prefer_standard_library", Json::Bool(true))]),
                    ),
                ]),
            )
            .expect_err("the constraint is unknown");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_instantiation_without_an_approved_implementation_is_refused_with_policy_rejected() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                INSTANTIATE,
                &args(vec![
                    ("asset_id", Json::str("algorithm.graph.kruskal")),
                    ("version", Json::str("1.0.0")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect_err("there is no approved implementation");
        assert_eq!(error.code, ErrorCode::PolicyRejected);
        assert!(error.message.contains("approved"), "{}", error.message);
    }

    #[test]
    fn the_instantiation_answers_the_draft_of_the_service() {
        let mut service = FakeService::empty();
        service.plan = Some(PlanDraft {
            asset_id: "algorithm.graph.kruskal".to_string(),
            version: "1.0.0".to_string(),
            language: "rust".to_string(),
            implementation_id: "algorithm.graph.kruskal.rust".to_string(),
            parameters: args(vec![("n", Json::int(4))]),
            steps: vec!["render the plan".to_string()],
            warnings: vec!["the draft is a plan".to_string()],
            contract_version: "1.1.0".to_string(),
        });
        let tools = tools_of(service);
        let answer = tools
            .call(
                INSTANTIATE,
                &args(vec![
                    ("asset_id", Json::str("algorithm.graph.kruskal")),
                    ("version", Json::str("1.0.0")),
                    ("language", Json::str("rust")),
                    ("parameters", args(vec![("n", Json::int(4))])),
                ]),
            )
            .expect("the answer");
        let plan = answer.get("plan").expect("the plan");
        assert_eq!(
            plan.get("implementation_id").and_then(Json::as_str),
            Some("algorithm.graph.kruskal.rust")
        );
        assert_eq!(plan.get("contract_version").and_then(Json::as_str), Some("1.1.0"));
    }

    #[test]
    fn the_parameters_of_an_instantiation_must_be_an_object() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(
                INSTANTIATE,
                &args(vec![
                    ("asset_id", Json::str("algorithm.graph.kruskal")),
                    ("version", Json::str("1.0.0")),
                    ("language", Json::str("rust")),
                    ("parameters", Json::str("n=4")),
                ]),
            )
            .expect_err("the parameters must be an object");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_describe_answers_the_whole_card_of_the_service() {
        let mut service = FakeService::empty();
        service.detail = Some(detail());
        let tools = tools_of(service);
        let answer = tools
            .call(DESCRIBE, &args(vec![("asset_id", Json::str("algorithm.graph.kruskal"))]))
            .expect("the answer");
        let asset = answer.get("asset").expect("the asset");
        assert!(asset.get("card_markdown").is_some());
        assert!(asset.get("manifest").is_some());
        assert_eq!(asset.get("card_status").and_then(Json::as_str), Some("DOCUMENTED"));
    }

    #[test]
    fn the_describe_of_an_asset_that_does_not_exist_is_refused_with_not_found() {
        let mut service = FakeService::empty();
        service.detail_error = Some(TaseError::not_found("there is no asset"));
        let tools = tools_of(service);
        let error = tools
            .call(DESCRIBE, &args(vec![("asset_id", Json::str("nothing"))]))
            .expect_err("there is no such asset");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn the_describe_needs_the_id_of_an_asset() {
        let tools = tools_of(FakeService::empty());
        let error = tools
            .call(DESCRIBE, &Json::Obj(Vec::new()))
            .expect_err("the id is required");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_instructions_say_what_the_tools_are_for_and_what_to_read_carefully() {
        assert!(INSTRUCTIONS.len() < 4096, "the instructions are too long");
        assert!(INSTRUCTIONS.contains("catalog.search"));
        assert!(INSTRUCTIONS.contains("catalog.recommend"));
        assert!(INSTRUCTIONS.contains("catalog.instantiate"));
        assert!(INSTRUCTIONS.contains("catalog.describe"));
        assert!(INSTRUCTIONS.contains("POLICY_REJECTED"));
        assert!(INSTRUCTIONS.contains("counterexamples"));
        assert!(INSTRUCTIONS.contains("risks"));
        assert!(INSTRUCTIONS.contains("read-only"));
    }
}