//! The adapter between an MCP client and the catalog.
//!
//! # What the adapter is
//!
//! The adapter holds a [`Catalog`] of `tase-catalog` and answers the two shapes an MCP client
//! speaks:
//!
//! * [`CatalogAdapter::as_mcp_tool_list`] - the answer of `tools/list`: the four tools, their
//!   description and their JSON Schema;
//! * [`CatalogAdapter::call_as_mcp`] - the answer of `tools/call`: the tool runs, and the answer
//!   comes back in the shape of `pime-agent` (`content` + `isError` + `structuredContent`).
//!
//! The adapter is also the [`CatalogService`] of the tools: the four questions the tools ask are
//! answered out of the catalog, and the ranking, the gate and the governance are **not** written
//! twice - they come from `tase-catalog`.
//!
//! # The shape of the answer
//!
//! The shape is the one of `pime-agent` (`crates/pime-agent/src/mcp.rs`), so that the MCP server of
//! the agent may hand the answer of the catalog to its client without reshaping it:
//!
//! ```json
//! {
//!   "content": [{ "type": "text", "text": "<the answer as a JSON string>" }],
//!   "isError": false,
//!   "structuredContent": { "ok": true, "tool": "catalog.search", "...": "..." }
//! }
//! ```
//!
//! A failure is the same shape with `"isError": true`, and the `structuredContent` carries the
//! business code of the failure (`{"ok": false, "error": "POLICY_REJECTED", "message": "..."}`),
//! the way the agent spells it.
//!
//! # The knowledge card is not an implementation
//!
//! The catalog of this repository holds 93 knowledge cards and not one language implementation.
//! The adapter keeps that difference visible: `catalog.instantiate` is refused with
//! [`tase_contract::ErrorCode::PolicyRejected`] and says why, and every answer says whether the
//! asset may be instantiated at all.

use std::fs;
use std::path::{Path, PathBuf};

use pime_core::json::{Json, Limits, parse};
use tase_catalog::{AssetView, Catalog, CatalogQuery, Layer, RecommendRequest};
use tase_contract::TaseError;

use crate::tools::{
    AssetDetail, AssetSummary, CatalogService, CatalogTools, ImplementationSummary, InstantiateRequest,
    PlanDraft, RecommendQuery, RecommendedAsset, RefusedAsset, ScorePart, SearchQuery, DEFAULT_LIMIT,
    INSTRUCTIONS,
};

/// The name of the method that lists the tools.
pub const METHOD_TOOLS_LIST: &str = "tools/list";

/// The name of the method that calls a tool.
pub const METHOD_TOOLS_CALL: &str = "tools/call";

/// The adapter of a catalog: the service of the tools and the MCP shape of their answers.
#[derive(Clone, Debug)]
pub struct CatalogAdapter {
    catalog: Catalog,
    root: PathBuf,
}

impl CatalogAdapter {
    /// The adapter of a catalog that is already loaded.
    ///
    /// The `root` is the directory the catalog was read from: the knowledge cards and the manifests
    /// are read from it when an asset is described.
    pub fn new(catalog: Catalog, root: impl Into<PathBuf>) -> CatalogAdapter {
        CatalogAdapter {
            catalog,
            root: root.into(),
        }
    }

    /// The adapter of a catalog that is on the disk.
    pub fn load(dir: &Path) -> Result<CatalogAdapter, TaseError> {
        let catalog = Catalog::load(dir)?;
        Ok(CatalogAdapter::new(catalog, dir))
    }

    /// The catalog behind the adapter.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The tools of the adapter, bound to the adapter itself.
    pub fn tools(&self) -> CatalogTools {
        CatalogTools {
            catalog: Box::new(self.clone()),
        }
    }

    /// The list of the tools, as `tools/list` answers it.
    ///
    /// The answer is `{"tools": [{"name": ..., "description": ..., "inputSchema": {...}}]}`.
    pub fn as_mcp_tool_list(&self) -> Json {
        let tools: Vec<Json> = crate::tools::tools()
            .iter()
            .map(|tool| {
                Json::obj(vec![
                    ("name", Json::str(tool.name)),
                    ("description", Json::str(tool.description)),
                    ("inputSchema", schema_json(tool.input_schema)),
                ])
            })
            .collect();
        Json::obj(vec![("tools", Json::Arr(tools))])
    }

    /// The answer of a JSON-RPC method of the catalog.
    ///
    /// * `tools/list` answers the list of the tools;
    /// * `tools/call` runs the tool named in `params.name` with the arguments of `params.arguments`;
    /// * any other method answers a failure that says there is no such method.
    ///
    /// The answer is the **result** of the JSON-RPC call - the object the host puts into the
    /// `result` field of its envelope. The host keeps the `id` and the `jsonrpc` field, the
    /// adapter answers the body.
    pub fn call_as_mcp(&self, method: &str, params: &Json) -> Json {
        match method {
            METHOD_TOOLS_LIST => self.as_mcp_tool_list(),
            METHOD_TOOLS_CALL => self.tools_call(params),
            other => tool_result(
                &error_json(&format!("there is no method named {other:?}")),
                true,
            ),
        }
    }

    /// The answer of `tools/call`.
    fn tools_call(&self, params: &Json) -> Json {
        let name = params.get("name").and_then(Json::as_str).unwrap_or("");
        let arguments = match params.get("arguments") {
            None | Some(Json::Null) => Json::Obj(Vec::new()),
            Some(value) => value.clone(),
        };
        match self.tools().call(name, &arguments) {
            Ok(answer) => tool_result(&answer, false),
            Err(error) => tool_result(&error_json_of(&error), true),
        }
    }

    /// The instructions of the server: what the tools are for and what has to be read carefully.
    pub fn instructions(&self) -> &'static str {
        INSTRUCTIONS
    }

    /// The summary of an asset, for a language.
    ///
    /// An empty language asks for the governance of the asset itself, without a language: that is
    /// what `catalog.describe` needs, because it names no language.
    fn summary_of(&self, view: &AssetView, language: &str) -> AssetSummary {
        let entry = &view.entry;
        let (instantiable, reasons) = if language.is_empty() {
            self.governance_without_language(view)
        } else {
            match self.catalog.governance_of(view.id(), language) {
                Some(state) => (state.instantiable, state.reasons.clone()),
                None => (
                    false,
                    vec![format!(
                        "the asset {} is not in the catalog of this server",
                        view.id()
                    )],
                ),
            }
        };
        AssetSummary {
            asset_id: entry.id.clone(),
            version: entry.version.clone(),
            layer: view.layer.as_str().to_string(),
            category: entry.category.clone(),
            name_zh: entry.name_zh().map(str::to_string),
            name_en: entry.name_en().map(str::to_string),
            aliases: view.aliases.clone(),
            status: entry.status.clone(),
            intent: entry.intent.clone(),
            license: entry.license.clone(),
            complexity_time: entry.complexity_time.clone(),
            complexity_space: entry.complexity_space.clone(),
            instantiable,
            instantiable_reasons: reasons,
            counterexamples: entry.counterexamples.clone(),
            risks: self.risks_of(view, language),
            alternatives: entry.alternatives.clone(),
            digest: entry.digest.clone(),
        }
    }

    /// The governance of an asset, without a language: the states of its implementations.
    fn governance_without_language(&self, view: &AssetView) -> (bool, Vec<String>) {
        let implementations = self.catalog.implementations().for_asset(view.id());
        let mut reasons: Vec<String> = implementations
            .iter()
            .map(|implementation| {
                format!(
                    "the implementation {} is {} in {}",
                    implementation.id,
                    implementation.status.as_str(),
                    implementation.language
                )
            })
            .collect();
        if implementations.is_empty() {
            reasons.push(format!(
                "the asset {} is a knowledge card: it has no implementation in any language",
                view.id()
            ));
        }
        let instantiable = implementations
            .iter()
            .any(|implementation| implementation.is_instantiable());
        (instantiable, reasons)
    }

    /// The risks the card declares, read from the conditions of its manifest.
    ///
    /// A card that says nothing about a property is not punished for it: the risk is reported, the
    /// property is not invented (requirements §14.4).
    fn risks_of(&self, view: &AssetView, language: &str) -> Vec<String> {
        let traits = view.traits();
        let mut risks: Vec<String> = Vec::new();
        if traits.global_mutable_state {
            risks.push("the card declares that the asset keeps a global mutable state".to_string());
        }
        if traits.not_thread_safe {
            risks.push("the card declares that the asset is not thread safe".to_string());
        }
        if !traits.declares_thread_safe {
            risks.push("the card does not declare whether the asset is thread safe".to_string());
        }
        if traits.may_need_negative_inputs {
            risks.push("the card declares that the asset may need negative inputs".to_string());
        }
        if traits.stable_order == Some(false) {
            risks.push("the card declares that the order it produces is not stable".to_string());
        }
        if view.entry.license.is_none() {
            risks.push("the card declares no license".to_string());
        }
        let implementations = self.catalog.implementations().for_asset(view.id());
        if language.is_empty() {
            if implementations.is_empty() {
                risks.push("there is no implementation of the asset in any language".to_string());
            }
        } else {
            let in_language: Vec<&tase_catalog::Implementation> = implementations
                .iter()
                .copied()
                .filter(|implementation| implementation.is_in_language(language))
                .collect();
            if in_language.is_empty() {
                risks.push(format!(
                    "there is no implementation of the asset in {language}"
                ));
            } else if !in_language
                .iter()
                .any(|implementation| implementation.is_instantiable())
            {
                risks.push(format!(
                    "the implementation of the asset in {language} is not APPROVED"
                ));
            }
        }
        if view.entry.counterexamples.is_empty() {
            risks.push("the card declares no counterexample".to_string());
        }
        risks
    }

    /// The request of the catalog, out of the query of the tools.
    fn request_of(&self, query: &RecommendQuery) -> Result<RecommendRequest, TaseError> {
        let layer = match &query.layer {
            None => None,
            Some(name) => {
                let canonical = name.trim().to_uppercase();
                Some(Layer::from_name(&canonical).ok_or_else(|| {
                    TaseError::invalid_argument(format!(
                        "the layer {name:?} is not one of the six layers of the catalog"
                    ))
                })?)
            }
        };
        Ok(RecommendRequest {
            intent: query.intent.clone(),
            language: query.language.clone(),
            constraints: query.constraints.clone(),
            layer,
            category: query.category.clone(),
            limit: query.limit,
        })
    }
}

impl CatalogService for CatalogAdapter {
    /// The cards that answer the query.
    fn search(&self, q: &SearchQuery) -> Result<Vec<AssetSummary>, TaseError> {
        let layer = match &q.layer {
            None => None,
            Some(name) => {
                let canonical = name.trim().to_uppercase();
                Some(Layer::from_name(&canonical).ok_or_else(|| {
                    TaseError::invalid_argument(format!(
                        "the layer {name:?} is not one of the six layers of the catalog"
                    ))
                })?)
            }
        };
        let query = CatalogQuery {
            text: if q.query.trim().is_empty() {
                None
            } else {
                Some(q.query.clone())
            },
            layer,
            category: q.category.clone(),
            tag: q.tag.clone(),
            language: Some(q.language.clone()),
        };
        let limit = if q.limit == 0 { DEFAULT_LIMIT } else { q.limit };
        let hits = self.catalog.index().search(&query);
        Ok(hits
            .iter()
            .take(limit)
            .map(|view| self.summary_of(view, &q.language))
            .collect())
    }

    /// The candidates that answer the request, the best one first.
    fn recommend(&self, r: &RecommendQuery) -> Result<Vec<RecommendedAsset>, TaseError> {
        let result = self.catalog.recommend(&self.request_of(r)?)?;
        Ok(result.recommended.iter().map(recommended_of).collect())
    }

    /// The candidates that were refused, with the reason of the refusal.
    fn refused(&self, query: &RecommendQuery) -> Vec<RefusedAsset> {
        let Ok(request) = self.request_of(query) else {
            return Vec::new();
        };
        match self.catalog.recommend(&request) {
            Ok(result) => result
                .rejected
                .iter()
                .map(|rejected| RefusedAsset {
                    asset_id: rejected.asset_id.clone(),
                    reason: rejected.reason.as_str().to_string(),
                    detail: rejected.detail.clone(),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The whole knowledge card of one asset, by its id, its name or its alias.
    fn describe(&self, id: &str) -> Result<AssetDetail, TaseError> {
        let key = id.trim();
        if key.is_empty() {
            return Err(TaseError::invalid_argument(
                "the id of an asset must not be empty",
            ));
        }
        let view = self.catalog.inspect(key).ok_or_else(|| {
            TaseError::not_found(format!("there is no asset named {key:?} in the catalog"))
        })?;
        let entry = &view.entry;

        let mut notes: Vec<String> = Vec::new();
        let card_markdown = match &entry.card_path {
            None => {
                notes.push(format!("the asset {} carries no knowledge card", entry.id));
                String::new()
            }
            Some(relative) => {
                let path = self.root.join(relative);
                match fs::read_to_string(&path) {
                    Ok(body) => body,
                    Err(error) => {
                        notes.push(format!(
                            "the knowledge card of {} could not be read ({}): {}",
                            entry.id,
                            path.display(),
                            error
                        ));
                        String::new()
                    }
                }
            }
        };

        let manifest = match self.manifest_of(view) {
            Some(manifest) => manifest,
            None => {
                notes.push(format!(
                    "the manifest of {} could not be read: the structured view is given instead",
                    entry.id
                ));
                manifest_json(view)
            }
        };

        let implementations: Vec<ImplementationSummary> = self
            .catalog
            .implementations()
            .for_asset(view.id())
            .iter()
            .map(|implementation| ImplementationSummary {
                implementation_id: implementation.id.clone(),
                language: implementation.language.clone(),
                status: implementation.status.as_str().to_string(),
                version: implementation.version.clone(),
                engine: implementation.engine.clone(),
                path: implementation.path.clone(),
                sha256: implementation.sha256.clone(),
                parameters_schema: implementation.parameters_schema.clone(),
                tests_manifest: implementation.tests_manifest.clone(),
            })
            .collect();

        let (instantiable, governance_reasons) = self.governance_without_language(view);
        let revoked = implementations
            .iter()
            .any(|implementation| implementation.status == "REVOKED");

        Ok(AssetDetail {
            summary: self.summary_of(view, ""),
            card_markdown,
            manifest,
            card_status: entry.status.clone(),
            recommendable: !revoked,
            instantiable,
            governance_reasons,
            implementations,
            notes,
        })
    }

    /// The draft of the plan of an instantiation.
    ///
    /// The draft is refused with [`tase_contract::ErrorCode::PolicyRejected`] when the asset has no
    /// APPROVED implementation in the requested language: a knowledge card may not be instantiated
    /// (requirements §14.4).
    fn instantiate(&self, req: &InstantiateRequest) -> Result<PlanDraft, TaseError> {
        let key = req.asset_id.trim();
        let view = self.catalog.inspect(key).ok_or_else(|| {
            TaseError::not_found(format!("there is no asset named {key:?} in the catalog"))
        })?;
        if view.entry.version != req.version.trim() {
            return Err(TaseError::invalid_argument(format!(
                "the asset {} is in the version {}, not in {}",
                view.id(),
                view.entry.version,
                req.version
            )));
        }
        let language = req.language.trim();
        let approved = self
            .catalog
            .implementations()
            .approved_for(view.id(), language);
        let Some(implementation) = approved else {
            let in_language = self
                .catalog
                .implementations()
                .for_asset_and_language(view.id(), language);
            let reason = if in_language.is_empty() {
                format!(
                    "the asset {} has no implementation in {language}: it is a knowledge card and a knowledge card may not be instantiated",
                    view.id()
                )
            } else {
                let states: Vec<&str> = in_language
                    .iter()
                    .map(|implementation| implementation.status.as_str())
                    .collect();
                format!(
                    "the implementation of {} in {language} is {} and not APPROVED: only an APPROVED implementation may be instantiated",
                    view.id(),
                    states.join(", ")
                )
            };
            return Err(TaseError::policy_rejected(reason));
        };
        self.catalog.policy().may_instantiate(
            implementation,
            view.entry.license.as_deref(),
            &req.allowed_licenses,
        )?;

        let mut warnings: Vec<String> = Vec::new();
        if let Some(license) = view.entry.license.as_deref() {
            warnings.push(format!(
                "the license of the asset is {license}: it must travel with the code that uses it"
            ));
        }
        for counterexample in &view.entry.counterexamples {
            warnings.push(format!("the card does not solve: {counterexample}"));
        }
        warnings.push(
            "the draft is a plan: nothing is written and nothing is applied by this tool".to_string(),
        );

        Ok(PlanDraft {
            asset_id: view.entry.id.clone(),
            version: view.entry.version.clone(),
            language: implementation.language.clone(),
            implementation_id: implementation.id.clone(),
            parameters: req.parameters.clone(),
            steps: vec![
                format!(
                    "take the implementation {} of {} in {}",
                    implementation.id, view.id(), implementation.language
                ),
                "check the parameters against the schema of the implementation".to_string(),
                "render the plan and show it to the user".to_string(),
                "apply the plan only after the user has approved it".to_string(),
            ],
            warnings,
            contract_version: tase_contract::CONTRACT_VERSION.to_string(),
        })
    }
}

impl CatalogAdapter {
    /// The manifest of an asset, read from the disk of the catalog.
    fn manifest_of(&self, view: &AssetView) -> Option<Json> {
        let path = self
            .root
            .join("assets")
            .join(view.id())
            .join("manifest.json");
        let bytes = fs::read_to_string(path).ok()?;
        parse(&bytes, Limits::DEFAULT).ok()
    }
}

/// A candidate of the catalog, as the tool answers it.
fn recommended_of(recommendation: &tase_catalog::Recommendation) -> RecommendedAsset {
    RecommendedAsset {
        asset_id: recommendation.asset_id.clone(),
        version: recommendation.version.clone(),
        layer: recommendation.layer.as_str().to_string(),
        score: recommendation.score,
        breakdown: recommendation
            .breakdown
            .components()
            .iter()
            .map(|component| ScorePart {
                name: component.name.to_string(),
                points: component.points,
                weight: component.weight,
            })
            .collect(),
        confidence_source: recommendation.confidence_source.as_str().to_string(),
        instantiable: recommendation.instantiable,
        implementation_id: recommendation.implementation_id.clone(),
        requirements: recommendation.requirements.clone(),
        proof_evidence: recommendation.proof_evidence.clone(),
        reasons: recommendation.reasons.clone(),
        counterexamples: recommendation.counterexamples.clone(),
        risks: recommendation.risks.clone(),
        notes: recommendation.notes.clone(),
        license: recommendation.license.clone(),
        complexity_time: recommendation
            .complexity
            .as_ref()
            .map(|complexity| complexity.time.clone()),
        complexity_space: recommendation
            .complexity
            .as_ref()
            .map(|complexity| complexity.extra_space.clone()),
        alternatives: recommendation.alternatives.clone(),
    }
}

/// The manifest of an asset, built out of the entry of the registry.
fn manifest_json(view: &AssetView) -> Json {
    let entry = &view.entry;
    let names: Vec<(String, Json)> = entry
        .names
        .iter()
        .map(|(language, name)| (language.clone(), Json::str(name)))
        .collect();
    let constraints: Vec<Json> = entry
        .constraints
        .iter()
        .map(|(name, value)| {
            Json::obj(vec![
                ("name", Json::str(name)),
                ("value", Json::str(value)),
            ])
        })
        .collect();
    let implementations: Vec<Json> = entry
        .implementations
        .iter()
        .map(|implementation| {
            Json::obj(vec![
                (
                    "implementation_id",
                    Json::str(&implementation.implementation_id),
                ),
                ("language", Json::str(&implementation.language)),
                ("version", Json::str(&implementation.version)),
                ("status", Json::str(&implementation.status)),
                ("engine", Json::str(&implementation.engine)),
                ("path", Json::str(&implementation.path)),
                ("sha256", Json::str(&implementation.sha256)),
                (
                    "parameters_schema",
                    Json::str(&implementation.parameters_schema),
                ),
                (
                    "tests_manifest",
                    Json::str(&implementation.tests_manifest),
                ),
            ])
        })
        .collect();
    Json::obj(vec![
        ("schema_version", Json::str("tase.asset.v1")),
        ("asset_id", Json::str(&entry.id)),
        ("version", Json::str(&entry.version)),
        ("kind", Json::str(&entry.kind)),
        ("name", Json::Obj(names)),
        ("aliases", Json::strs(&entry.aliases)),
        ("category", Json::str(&entry.category)),
        ("status", Json::str(&entry.status)),
        ("intent", Json::str(&entry.intent)),
        (
            "applicability",
            Json::obj(vec![
                ("constraints", Json::Arr(constraints)),
                ("counterexamples", Json::strs(&entry.counterexamples)),
                ("alternatives", Json::strs(&entry.alternatives)),
            ]),
        ),
        (
            "complexity",
            match (&entry.complexity_time, &entry.complexity_space) {
                (None, None) => Json::Null,
                (time, space) => Json::obj(vec![
                    ("time", optional(time.as_deref())),
                    ("extra_space", optional(space.as_deref())),
                ]),
            },
        ),
        ("implementations", Json::Arr(implementations)),
        (
            "provenance",
            Json::obj(vec![
                ("references", Json::strs(&entry.references)),
                ("license", optional(entry.license.as_deref())),
            ]),
        ),
    ])
}

/// A string that may be missing, as the JSON of it.
fn optional(text: Option<&str>) -> Json {
    match text {
        Some(text) => Json::str(text),
        None => Json::Null,
    }
}

/// The schema of a tool, parsed. An empty object when the text is not a JSON.
fn schema_json(text: &str) -> Json {
    parse(text, Limits::DEFAULT).unwrap_or_else(|_| Json::Obj(Vec::new()))
}

/// The answer of a tool: the compact JSON answer is the text, the answer itself is the structure.
///
/// This is the shape of `pime-agent` (`crates/pime-agent/src/mcp.rs`).
fn tool_result(answer: &Json, failed: bool) -> Json {
    Json::obj(vec![
        (
            "content",
            Json::Arr(vec![Json::obj(vec![
                ("type", Json::str("text")),
                ("text", Json::str(&answer.to_json_string())),
            ])]),
        ),
        ("isError", Json::Bool(failed)),
        ("structuredContent", answer.clone()),
    ])
}

/// The JSON of a failure of the business codes.
fn error_json_of(error: &TaseError) -> Json {
    let mut pairs = vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str(error.code.as_str())),
        ("message", Json::str(&error.message)),
    ];
    if let Some(details) = &error.details {
        pairs.push(("details", Json::str(details)));
    }
    Json::obj(pairs)
}

/// The JSON of a failure of the transport.
fn error_json(message: &str) -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str("INVALID_ARGUMENT")),
        ("message", Json::str(message)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{CatalogTools, SEARCH};
    use tase_contract::ErrorCode;

    /// The catalog of the repository.
    fn real_catalog() -> CatalogAdapter {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        CatalogAdapter::load(&dir).expect("the catalog of the repository is valid")
    }

    /// The arguments of a call.
    fn args(pairs: Vec<(&str, Json)>) -> Json {
        Json::obj(pairs)
    }

    /// The JSON of the parameters of a `tools/call`.
    fn call_params(name: &str, arguments: Json) -> Json {
        Json::obj(vec![("name", Json::str(name)), ("arguments", arguments)])
    }

    #[test]
    fn the_tool_list_holds_the_four_tools_with_their_schema() {
        let answer = real_catalog().as_mcp_tool_list();
        let tools = answer.get("tools").and_then(Json::as_array).expect("the tools");
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Json::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "catalog.search",
                "catalog.recommend",
                "catalog.instantiate",
                "catalog.describe"
            ]
        );
        for tool in tools {
            assert!(tool.get("description").and_then(Json::as_str).is_some());
            let schema = tool.get("inputSchema").expect("the schema");
            assert_eq!(
                schema.get("$schema").and_then(Json::as_str),
                Some("https://json-schema.org/draft/2020-12/schema")
            );
            assert!(schema.get("properties").is_some());
        }
    }

    #[test]
    fn the_tool_list_is_the_same_as_the_one_of_the_declaration() {
        let answer = real_catalog().as_mcp_tool_list();
        let tools = answer.get("tools").and_then(Json::as_array).expect("the tools");
        for (tool, declaration) in tools.iter().zip(crate::tools::tools()) {
            assert_eq!(
                tool.get("name").and_then(Json::as_str),
                Some(declaration.name)
            );
            assert_eq!(
                tool.get("description").and_then(Json::as_str),
                Some(declaration.description)
            );
        }
    }

    #[test]
    fn a_call_that_succeeds_answers_the_shape_of_the_agent() {
        let adapter = real_catalog();
        let answer = adapter.call_as_mcp(
            METHOD_TOOLS_CALL,
            &call_params(
                SEARCH,
                args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                ]),
            ),
        );
        assert_eq!(answer.get("isError").and_then(Json::as_bool), Some(false));
        let content = answer.get("content").and_then(Json::as_array).expect("the content");
        assert_eq!(content.len(), 1);
        assert_eq!(
            content[0].get("type").and_then(Json::as_str),
            Some("text")
        );
        let text = content[0].get("text").and_then(Json::as_str).expect("the text");
        assert!(text.contains("catalog.search"), "{text}");
        // the structure of the answer is there too
        let structured = answer
            .get("structuredContent")
            .expect("the structure of the answer");
        assert_eq!(structured.get("ok").and_then(Json::as_bool), Some(true));
    }

    #[test]
    fn a_call_that_fails_answers_is_error_and_a_readable_message() {
        let adapter = real_catalog();
        let answer = adapter.call_as_mcp(
            METHOD_TOOLS_CALL,
            &call_params("catalog.nothing", Json::Obj(Vec::new())),
        );
        assert_eq!(answer.get("isError").and_then(Json::as_bool), Some(true));
        let content = answer.get("content").and_then(Json::as_array).expect("the content");
        let text = content[0].get("text").and_then(Json::as_str).expect("the text");
        assert!(text.contains("NOT_FOUND"), "{text}");
        assert!(text.contains("catalog.nothing"), "{text}");
        let structured = answer.get("structuredContent").expect("the structure");
        assert_eq!(structured.get("ok").and_then(Json::as_bool), Some(false));
        assert_eq!(
            structured.get("error").and_then(Json::as_str),
            Some("NOT_FOUND")
        );
    }

    #[test]
    fn a_call_with_arguments_that_are_not_an_object_answers_is_error() {
        let adapter = real_catalog();
        let answer = adapter.call_as_mcp(
            METHOD_TOOLS_CALL,
            &call_params(SEARCH, Json::str("MST")),
        );
        assert_eq!(answer.get("isError").and_then(Json::as_bool), Some(true));
    }

    #[test]
    fn a_call_without_arguments_answers_is_error() {
        let adapter = real_catalog();
        let answer = adapter.call_as_mcp(
            METHOD_TOOLS_CALL,
            &Json::obj(vec![("name", Json::str(SEARCH))]),
        );
        assert_eq!(answer.get("isError").and_then(Json::as_bool), Some(true));
    }

    #[test]
    fn a_method_that_is_not_known_answers_is_error() {
        let adapter = real_catalog();
        let answer = adapter.call_as_mcp("tools/nothing", &Json::Obj(Vec::new()));
        assert_eq!(answer.get("isError").and_then(Json::as_bool), Some(true));
        let content = answer.get("content").and_then(Json::as_array).expect("the content");
        let text = content[0].get("text").and_then(Json::as_str).expect("the text");
        assert!(text.contains("tools/nothing"), "{text}");
    }

    #[test]
    fn the_method_of_the_tool_list_answers_the_list() {
        let adapter = real_catalog();
        let answer = adapter.call_as_mcp(METHOD_TOOLS_LIST, &Json::Obj(Vec::new()));
        assert!(answer.get("isError").is_none());
        assert_eq!(
            answer
                .get("tools")
                .and_then(Json::as_array)
                .map(Vec::len),
            Some(4)
        );
    }

    #[test]
    fn the_instructions_are_the_ones_of_the_declaration() {
        let adapter = real_catalog();
        assert_eq!(adapter.instructions(), crate::tools::INSTRUCTIONS);
        assert!(adapter.instructions().contains("catalog.search"));
    }

    #[test]
    fn the_search_of_the_real_catalog_finds_the_kruskal_card() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("MST")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let assets = answer.get("assets").and_then(Json::as_array).expect("the assets");
        assert!(!assets.is_empty());
        let ids: Vec<&str> = assets
            .iter()
            .filter_map(|asset| asset.get("asset_id").and_then(Json::as_str))
            .collect();
        assert!(ids.contains(&"algorithm.graph.kruskal"), "{ids:?}");
    }

    #[test]
    fn the_answer_of_the_search_carries_the_counterexamples_the_license_and_the_risks() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
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
        assert!(text.contains("MIT"), "{text}");
        assert!(
            text.contains("the card does not declare whether the asset is thread safe"),
            "{text}"
        );
        // The algorithm carries its implementation, but it is TESTED and not APPROVED: the answer says so.
        assert!(
            text.contains("the implementation of the asset in rust is not APPROVED"),
            "{text}"
        );
    }

    #[test]
    fn the_search_of_the_real_catalog_finds_the_cards_by_their_alias() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                SEARCH,
                &args(vec![
                    ("query", Json::str("单例")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let assets = answer.get("assets").and_then(Json::as_array).expect("the assets");
        let ids: Vec<&str> = assets
            .iter()
            .filter_map(|asset| asset.get("asset_id").and_then(Json::as_str))
            .collect();
        assert!(ids.contains(&"pattern.gof.creational.singleton"), "{ids:?}");
    }

    #[test]
    fn the_recommendation_of_the_real_catalog_ranks_the_singleton_first() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                crate::tools::RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let recommended = answer
            .get("recommended")
            .and_then(Json::as_array)
            .expect("the candidates");
        assert!(!recommended.is_empty());
        assert_eq!(
            recommended[0].get("asset_id").and_then(Json::as_str),
            Some("pattern.gof.creational.singleton")
        );
        // the candidate is a knowledge card: it may not be instantiated
        assert_eq!(
            recommended[0].get("instantiable").and_then(Json::as_bool),
            Some(false)
        );
        // the score is broken down
        let breakdown = recommended[0]
            .get("breakdown")
            .and_then(Json::as_array)
            .expect("the breakdown");
        assert_eq!(breakdown.len(), 4);
        assert_eq!(
            breakdown[0].get("name").and_then(Json::as_str),
            Some("intent")
        );
    }

    #[test]
    fn the_recommendation_of_the_real_catalog_carries_the_risks() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                crate::tools::RECOMMEND,
                &args(vec![
                    ("intent", Json::str("单例")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect("the answer");
        let text = answer.to_json_string();
        assert!(
            text.contains("the card declares that the asset keeps a global mutable state"),
            "{text}"
        );
        assert!(text.contains("counterexamples"), "{text}");
    }

    #[test]
    fn the_describe_of_the_real_catalog_gives_the_card_and_the_manifest() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                crate::tools::DESCRIBE,
                &args(vec![("asset_id", Json::str("algorithm.graph.kruskal"))]),
            )
            .expect("the answer");
        let asset = answer.get("asset").expect("the asset");
        let card = asset
            .get("card_markdown")
            .and_then(Json::as_str)
            .expect("the card");
        assert!(card.contains("最小生成树"), "{card}");
        let manifest = asset.get("manifest").expect("the manifest");
        assert_eq!(
            manifest.get("asset_id").and_then(Json::as_str),
            Some("algorithm.graph.kruskal")
        );
        assert_eq!(
            manifest.get("schema_version").and_then(Json::as_str),
            Some("tase.asset.v1")
        );
    }

    #[test]
    fn the_describe_of_the_real_catalog_finds_an_asset_by_its_alias() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                crate::tools::DESCRIBE,
                &args(vec![("asset_id", Json::str("MST"))]),
            )
            .expect("the answer");
        let asset = answer.get("asset").expect("the asset");
        assert_eq!(
            asset
                .get("summary")
                .and_then(|summary| summary.get("asset_id"))
                .and_then(Json::as_str),
            Some("algorithm.graph.kruskal")
        );
    }

    #[test]
    fn the_describe_of_an_asset_that_does_not_exist_is_refused_with_not_found() {
        let adapter = real_catalog();
        let error = adapter
            .tools()
            .call(
                crate::tools::DESCRIBE,
                &args(vec![("asset_id", Json::str("nothing.at.all"))]),
            )
            .expect_err("there is no such asset");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn the_describe_names_the_governance_without_a_dangling_language() {
        let adapter = real_catalog();
        let answer = adapter
            .tools()
            .call(
                crate::tools::DESCRIBE,
                &args(vec![("asset_id", Json::str("algorithm.graph.kruskal"))]),
            )
            .expect("the answer");
        let asset = answer.get("asset").expect("the asset");
        let reasons = asset
            .get("governance_reasons")
            .and_then(Json::as_array)
            .expect("the reasons");
        let reasons: Vec<&str> = reasons.iter().filter_map(Json::as_str).collect();
        // The governance reasons name the state of the implementation of the card and the language of it: the
        // algorithm carries its implementation (the crate `tase-impls`), in the state TESTED, in Rust. The language
        // is named with it - never left dangling.
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("is TESTED in rust")),
            "{reasons:?}"
        );
        for reason in &reasons {
            assert!(!reason.ends_with(" in "), "{reason}");
        }
        let risks = asset
            .get("summary")
            .and_then(|summary| summary.get("risks"))
            .and_then(Json::as_array)
            .expect("the risks");
        let risks: Vec<&str> = risks.iter().filter_map(Json::as_str).collect();
        assert!(
            risks
                .iter()
                .any(|risk| risk.contains("does not declare whether the asset is thread safe")),
            "{risks:?}"
        );
        for risk in &risks {
            assert!(!risk.ends_with(" in "), "{risk}");
        }
    }

    #[test]
    fn the_instantiation_of_a_card_of_the_real_catalog_is_refused_with_policy_rejected() {
        let adapter = real_catalog();
        let error = adapter
            .tools()
            .call(
                crate::tools::INSTANTIATE,
                &args(vec![
                    ("asset_id", Json::str("algorithm.graph.kruskal")),
                    ("version", Json::str("1.0.0")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect_err("an implementation that is not approved may not be instantiated");
        assert_eq!(error.code, ErrorCode::PolicyRejected);
        // The implementation exists, but it is TESTED and not APPROVED: only an approved one may be instantiated.
        assert!(
            error.message.contains("not APPROVED"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_instantiation_of_a_version_that_is_not_the_one_of_the_asset_is_refused() {
        let adapter = real_catalog();
        let error = adapter
            .tools()
            .call(
                crate::tools::INSTANTIATE,
                &args(vec![
                    ("asset_id", Json::str("algorithm.graph.kruskal")),
                    ("version", Json::str("9.9.9")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect_err("the version is not the one of the asset");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_instantiation_of_an_asset_that_does_not_exist_is_refused_with_not_found() {
        let adapter = real_catalog();
        let error = adapter
            .tools()
            .call(
                crate::tools::INSTANTIATE,
                &args(vec![
                    ("asset_id", Json::str("nothing.at.all")),
                    ("version", Json::str("1.0.0")),
                    ("language", Json::str("rust")),
                ]),
            )
            .expect_err("there is no such asset");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn the_tools_of_the_adapter_are_the_tools_of_the_declaration() {
        let adapter = real_catalog();
        let tools: CatalogTools = adapter.tools();
        assert!(
            tools
                .call(
                    SEARCH,
                    &args(vec![
                        ("query", Json::str("MST")),
                        ("language", Json::str("rust")),
                    ]),
                )
                .is_ok()
        );
    }
}
