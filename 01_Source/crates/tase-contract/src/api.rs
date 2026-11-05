//! The DTOs of the TASE contract (design §2).
//!
//! Every type of this module is a plain data holder: the JSON examples of the design book are the only thing that
//! decides which field a type has and what the field is called. The JSON is written by hand with the strict parser
//! of `pime-core`, so a payload that is not ours is refused instead of half-understood, and no third-party serializer
//! decides the shape of the wire.
//!
//! The rules that hold for every type:
//! * the identifiers are the types of [`crate::ids`] and are checked when they are read;
//! * the times are strings (RFC 3339, UTC) and the hashes are strings;
//! * the counts are `u32`, the token counters are `u64`, the costs are `f64`;
//! * a field that may be absent is an `Option<T>` and is written as `null` when it is absent - the design book writes
//!   `"next_cursor": null` and `"approval_token": null` as well;
//! * a table of parameters is an ordered list of pairs, not a map: the order of the parameters is part of the digest
//!   of a plan, and the crate has no third-party map type to depend on.

use crate::error::TaseError;
use crate::ids::{
    AgentId, AssetId, CommitId, ImplementationId, JobId, OperationId, PlanId, PreviewId,
    RepositoryId, RequestId, SessionId, TemplateId, TenantId, TraceId, WorkspaceId,
};
use pime_core::json::Json;

/// The field of a JSON object; a missing field is an invalid argument.
fn field<'a>(value: &'a Json, key: &str) -> Result<&'a Json, TaseError> {
    value
        .get(key)
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} is missing")))
}

/// A string field.
fn req_str(value: &Json, key: &str) -> Result<String, TaseError> {
    field(value, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be a string")))
}

/// An optional string field: a missing field and a `null` are both "absent".
fn opt_str(value: &Json, key: &str) -> Result<Option<String>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(TaseError::invalid_argument(format!(
            "the field {key:?} must be a string"
        ))),
    }
}

/// A boolean field.
fn req_bool(value: &Json, key: &str) -> Result<bool, TaseError> {
    field(value, key)?
        .as_bool()
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be a boolean")))
}

/// An optional boolean field.
fn opt_bool(value: &Json, key: &str) -> Result<Option<bool>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(TaseError::invalid_argument(format!(
            "the field {key:?} must be a boolean"
        ))),
    }
}

/// A counter field.
fn req_u32(value: &Json, key: &str) -> Result<u32, TaseError> {
    let n = field(value, key)?.as_i64().ok_or_else(|| {
        TaseError::invalid_argument(format!("the field {key:?} must be a whole number"))
    })?;
    u32::try_from(n)
        .map_err(|_| TaseError::invalid_argument(format!("the field {key:?} must not be negative")))
}

/// A counter field.
fn req_u64(value: &Json, key: &str) -> Result<u64, TaseError> {
    let n = field(value, key)?.as_i64().ok_or_else(|| {
        TaseError::invalid_argument(format!("the field {key:?} must be a whole number"))
    })?;
    u64::try_from(n)
        .map_err(|_| TaseError::invalid_argument(format!("the field {key:?} must not be negative")))
}

/// An optional counter field.
fn opt_u64(value: &Json, key: &str) -> Result<Option<u64>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Num(n)) => {
            if n.fract() != 0.0 {
                return Err(TaseError::invalid_argument(format!(
                    "the field {key:?} must be a whole number"
                )));
            }
            let n = *n as i64;
            Ok(Some(u64::try_from(n).map_err(|_| {
                TaseError::invalid_argument(format!("the field {key:?} must not be negative"))
            })?))
        }
        Some(_) => Err(TaseError::invalid_argument(format!(
            "the field {key:?} must be a whole number"
        ))),
    }
}

/// An optional cost field.
fn opt_f64(value: &Json, key: &str) -> Result<Option<f64>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Num(n)) => Ok(Some(*n)),
        Some(_) => Err(TaseError::invalid_argument(format!(
            "the field {key:?} must be a number"
        ))),
    }
}

/// A list field.
fn req_array<'a>(value: &'a Json, key: &str) -> Result<&'a Vec<Json>, TaseError> {
    field(value, key)?
        .as_array()
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be an array")))
}

/// A list of strings.
fn req_strings(value: &Json, key: &str) -> Result<Vec<String>, TaseError> {
    let items = req_array(value, key)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(item.as_str().map(str::to_string).ok_or_else(|| {
            TaseError::invalid_argument(format!("the field {key:?} must only hold strings"))
        })?);
    }
    Ok(out)
}

/// A table of parameters: an object of strings, kept in the order it was written.
fn params_of(value: &Json, key: &str) -> Result<Vec<(String, String)>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Obj(pairs)) => {
            let mut out = Vec::with_capacity(pairs.len());
            for (name, item) in pairs {
                let text = item.as_str().ok_or_else(|| {
                    TaseError::invalid_argument(format!("the field {key:?} must only hold strings"))
                })?;
                out.push((name.clone(), text.to_string()));
            }
            Ok(out)
        }
        Some(_) => Err(TaseError::invalid_argument(format!(
            "the field {key:?} must be an object"
        ))),
    }
}

/// The little that the DTOs need from an identifier type.
trait Identifier {
    /// Reads an identifier.
    fn parse_id(s: &str) -> Result<Self, TaseError>
    where
        Self: Sized;
}

/// Implements [`Identifier`] for the identifier types.
macro_rules! impl_identifier {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Identifier for $ty {
                fn parse_id(s: &str) -> Result<Self, TaseError> {
                    <$ty>::parse(s)
                }
            }
        )*
    };
}

impl_identifier!(
    AgentId,
    AssetId,
    CommitId,
    ImplementationId,
    JobId,
    OperationId,
    PlanId,
    PreviewId,
    RepositoryId,
    RequestId,
    SessionId,
    TenantId,
    TemplateId,
    TraceId,
    WorkspaceId,
);

/// An identifier field.
fn req_id<T: Identifier>(value: &Json, key: &str) -> Result<T, TaseError> {
    let raw = req_str(value, key)?;
    T::parse_id(&raw).map_err(|e| {
        TaseError::invalid_argument(format!(
            "the field {key:?} is not a valid identifier: {}",
            e.message
        ))
    })
}

/// What an operation of a plan does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperationKind {
    /// Instantiate a template.
    InstantiateTemplate,
    /// Insert a symbol next to another one.
    InsertSibling,
    /// Replace the body of a symbol.
    ReplaceSymbolBody,
    /// Delete a symbol.
    DeleteSymbol,
    /// Update the imports of a file.
    UpdateImports,
    /// Format the file.
    Format,
}

impl OperationKind {
    /// Every kind of operation.
    pub const ALL: [OperationKind; 6] = [
        OperationKind::InstantiateTemplate,
        OperationKind::InsertSibling,
        OperationKind::ReplaceSymbolBody,
        OperationKind::DeleteSymbol,
        OperationKind::UpdateImports,
        OperationKind::Format,
    ];

    /// The kind on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::InstantiateTemplate => "instantiate_template",
            OperationKind::InsertSibling => "insert_sibling",
            OperationKind::ReplaceSymbolBody => "replace_symbol_body",
            OperationKind::DeleteSymbol => "delete_symbol",
            OperationKind::UpdateImports => "update_imports",
            OperationKind::Format => "format",
        }
    }

    /// The kind of a wire value; an unknown value is refused.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(kind: &str) -> Result<Self, TaseError> {
        match kind {
            "instantiate_template" => Ok(OperationKind::InstantiateTemplate),
            "insert_sibling" => Ok(OperationKind::InsertSibling),
            "replace_symbol_body" => Ok(OperationKind::ReplaceSymbolBody),
            "delete_symbol" => Ok(OperationKind::DeleteSymbol),
            "update_imports" => Ok(OperationKind::UpdateImports),
            "format" => Ok(OperationKind::Format),
            other => Err(TaseError::invalid_argument(format!(
                "unknown operation kind {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for OperationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for OperationKind {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OperationKind::from_str(s)
    }
}

/// The state of a check of a plan or of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckStatus {
    /// The check passed.
    Pass,
    /// The check failed.
    Fail,
    /// The check was skipped.
    Skipped,
    /// The check did not run.
    NotRun,
}

impl CheckStatus {
    /// Every state of a check.
    pub const ALL: [CheckStatus; 4] = [
        CheckStatus::Pass,
        CheckStatus::Fail,
        CheckStatus::Skipped,
        CheckStatus::NotRun,
    ];

    /// The state on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Skipped => "SKIPPED",
            CheckStatus::NotRun => "NOT_RUN",
        }
    }

    /// The state of a wire value; an unknown value is refused.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(status: &str) -> Result<Self, TaseError> {
        match status {
            "PASS" => Ok(CheckStatus::Pass),
            "FAIL" => Ok(CheckStatus::Fail),
            "SKIPPED" => Ok(CheckStatus::Skipped),
            "NOT_RUN" => Ok(CheckStatus::NotRun),
            other => Err(TaseError::invalid_argument(format!(
                "unknown check status {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CheckStatus {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CheckStatus::from_str(s)
    }
}

/// The state of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobState {
    /// The job waits for a worker.
    Queued,
    /// The job runs.
    Running,
    /// The job validates the result.
    Validating,
    /// The job commits the result.
    Committing,
    /// The job is done and the commit exists.
    Committed,
    /// The job failed.
    Failed,
    /// The job was cancelled.
    Cancelled,
    /// The job was rebased onto a new base and waits again.
    Rebased,
}

impl JobState {
    /// Every state of a job.
    pub const ALL: [JobState; 8] = [
        JobState::Queued,
        JobState::Running,
        JobState::Validating,
        JobState::Committing,
        JobState::Committed,
        JobState::Failed,
        JobState::Cancelled,
        JobState::Rebased,
    ];

    /// The state on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "QUEUED",
            JobState::Running => "RUNNING",
            JobState::Validating => "VALIDATING",
            JobState::Committing => "COMMITTING",
            JobState::Committed => "COMMITTED",
            JobState::Failed => "FAILED",
            JobState::Cancelled => "CANCELLED",
            JobState::Rebased => "REBASED",
        }
    }

    /// The state of a wire value; an unknown value is refused.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(state: &str) -> Result<Self, TaseError> {
        match state {
            "QUEUED" => Ok(JobState::Queued),
            "RUNNING" => Ok(JobState::Running),
            "VALIDATING" => Ok(JobState::Validating),
            "COMMITTING" => Ok(JobState::Committing),
            "COMMITTED" => Ok(JobState::Committed),
            "FAILED" => Ok(JobState::Failed),
            "CANCELLED" => Ok(JobState::Cancelled),
            "REBASED" => Ok(JobState::Rebased),
            other => Err(TaseError::invalid_argument(format!(
                "unknown job state {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for JobState {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        JobState::from_str(s)
    }
}

/// The envelope every request carries (design §2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestEnvelope {
    /// The version of the contract the caller speaks.
    pub contract_version: String,
    /// The id of the request; it is used for the trace only, never as the key of an idempotency.
    pub request_id: RequestId,
    /// The tenant of the caller. It is a hint in the body: the subject of the authorization comes from the
    /// credential, never from here.
    pub tenant_id: TenantId,
    /// The workspace the request works in.
    pub workspace_id: WorkspaceId,
    /// The repository the request works in.
    pub repository_id: RepositoryId,
    /// The agent that asks.
    pub agent_id: AgentId,
    /// The trace the call belongs to.
    pub trace_id: TraceId,
}

impl RequestEnvelope {
    /// The envelope as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("contract_version", Json::str(&self.contract_version)),
            ("request_id", Json::str(self.request_id.as_str())),
            ("tenant_id", Json::str(self.tenant_id.as_str())),
            ("workspace_id", Json::str(self.workspace_id.as_str())),
            ("repository_id", Json::str(self.repository_id.as_str())),
            ("agent_id", Json::str(self.agent_id.as_str())),
            ("trace_id", Json::str(self.trace_id.as_str())),
        ])
    }

    /// The envelope as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The envelope of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(RequestEnvelope {
            contract_version: req_str(value, "contract_version")?,
            request_id: req_id(value, "request_id")?,
            tenant_id: req_id(value, "tenant_id")?,
            workspace_id: req_id(value, "workspace_id")?,
            repository_id: req_id(value, "repository_id")?,
            agent_id: req_id(value, "agent_id")?,
            trace_id: req_id(value, "trace_id")?,
        })
    }
}

/// The query of the templates of the registry (design §2.2).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TemplateQuery {
    /// Only the templates of this language.
    pub language: Option<String>,
    /// Only the templates that carry this tag.
    pub tag: Option<String>,
    /// How many templates to return at most.
    pub limit: Option<u32>,
    /// The cursor of the page to return.
    pub cursor: Option<String>,
}

impl TemplateQuery {
    /// The query as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            (
                "language",
                match &self.language {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            (
                "tag",
                match &self.tag {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            (
                "limit",
                match self.limit {
                    Some(v) => Json::int(v as i64),
                    None => Json::Null,
                },
            ),
            (
                "cursor",
                match &self.cursor {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The query as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The query of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(TemplateQuery {
            language: opt_str(value, "language")?,
            tag: opt_str(value, "tag")?,
            limit: match value.get("limit") {
                None | Some(Json::Null) => None,
                Some(_) => Some(req_u32(value, "limit")?),
            },
            cursor: opt_str(value, "cursor")?,
        })
    }
}

/// The summary of a template (design §2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSummary {
    /// The id of the template.
    pub id: TemplateId,
    /// The version of the template.
    pub version: String,
    /// The digest of the template package (64 hex digits of SHA-256).
    pub digest: String,
    /// The language of the template.
    pub language: String,
    /// The tags of the template.
    pub tags: Vec<String>,
    /// Where the schema of the parameters of the template lives.
    pub params_schema_ref: String,
    /// The state of the approval of the template (`ACTIVE`, ...).
    pub approval_status: String,
    /// The license of the template.
    pub license: String,
}

impl TemplateSummary {
    /// The summary as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("id", Json::str(self.id.as_str())),
            ("version", Json::str(&self.version)),
            ("digest", Json::str(&self.digest)),
            ("language", Json::str(&self.language)),
            ("tags", Json::strs(&self.tags)),
            ("params_schema_ref", Json::str(&self.params_schema_ref)),
            ("approval_status", Json::str(&self.approval_status)),
            ("license", Json::str(&self.license)),
        ])
    }

    /// The summary as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The summary of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(TemplateSummary {
            id: req_id(value, "id")?,
            version: req_str(value, "version")?,
            digest: req_str(value, "digest")?,
            language: req_str(value, "language")?,
            tags: req_strings(value, "tags")?,
            params_schema_ref: req_str(value, "params_schema_ref")?,
            approval_status: req_str(value, "approval_status")?,
            license: req_str(value, "license")?,
        })
    }
}

/// The answer of a template search (design §2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSearchResult {
    /// The templates of this page.
    pub templates: Vec<TemplateSummary>,
    /// The cursor of the next page, when there is one.
    pub next_cursor: Option<String>,
}

impl TemplateSearchResult {
    /// The answer as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            (
                "templates",
                Json::Arr(
                    self.templates
                        .iter()
                        .map(TemplateSummary::to_json_value)
                        .collect(),
                ),
            ),
            (
                "next_cursor",
                match &self.next_cursor {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The answer as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The answer of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let templates = req_array(value, "templates")?
            .iter()
            .map(TemplateSummary::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TemplateSearchResult {
            templates,
            next_cursor: opt_str(value, "next_cursor")?,
        })
    }
}

/// Where an operation works (design §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The path of the file, relative to the root of the repository, with `/` as the separator.
    pub path: String,
    /// The symbol the operation works on.
    pub symbol: Option<String>,
    /// The kind of the symbol (`function`, `struct`, ...).
    pub symbol_kind: Option<String>,
}

impl Target {
    /// The target as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("path", Json::str(&self.path)),
            (
                "symbol",
                match &self.symbol {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            (
                "symbol_kind",
                match &self.symbol_kind {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The target as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The target of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(Target {
            path: req_str(value, "path")?,
            symbol: opt_str(value, "symbol")?,
            symbol_kind: opt_str(value, "symbol_kind")?,
        })
    }
}

/// The template an operation instantiates (design §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateRef {
    /// The id of the template.
    pub id: TemplateId,
    /// The version of the template.
    pub version: String,
    /// The digest of the template package the operation is pinned to.
    pub digest: String,
}

impl TemplateRef {
    /// The reference as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("id", Json::str(self.id.as_str())),
            ("version", Json::str(&self.version)),
            ("digest", Json::str(&self.digest)),
        ])
    }

    /// The reference as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The reference of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(TemplateRef {
            id: req_id(value, "id")?,
            version: req_str(value, "version")?,
            digest: req_str(value, "digest")?,
        })
    }
}

/// What must still be true when the operation runs (design §2.3).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Preconditions {
    /// The digest of the blob of the file. It may be absent for a file that does not exist yet.
    pub file_blob_sha: Option<String>,
    /// The digest of the signature of the symbol.
    pub symbol_signature_hash: Option<String>,
    /// Whether the symbol is expected to exist.
    pub expected_symbol_exists: Option<bool>,
}

impl Preconditions {
    /// The preconditions as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            (
                "file_blob_sha",
                match &self.file_blob_sha {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            (
                "symbol_signature_hash",
                match &self.symbol_signature_hash {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            (
                "expected_symbol_exists",
                match self.expected_symbol_exists {
                    Some(v) => Json::Bool(v),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The preconditions as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The preconditions of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(Preconditions {
            file_blob_sha: opt_str(value, "file_blob_sha")?,
            symbol_signature_hash: opt_str(value, "symbol_signature_hash")?,
            expected_symbol_exists: opt_bool(value, "expected_symbol_exists")?,
        })
    }
}

/// One operation of a plan (design §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    /// The id of the operation inside its plan.
    pub op_id: OperationId,
    /// What the operation does.
    pub kind: OperationKind,
    /// Where the operation works.
    pub target: Target,
    /// The template the operation instantiates, when it instantiates one.
    pub template: Option<TemplateRef>,
    /// The parameters of the template.
    pub params: Vec<(String, String)>,
    /// What must still be true when the operation runs.
    pub preconditions: Option<Preconditions>,
}

impl Operation {
    /// The operation as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("op_id", Json::str(self.op_id.as_str())),
            ("kind", Json::str(self.kind.as_str())),
            ("target", self.target.to_json_value()),
            (
                "template",
                match &self.template {
                    Some(v) => v.to_json_value(),
                    None => Json::Null,
                },
            ),
            (
                "params",
                Json::Obj(
                    self.params
                        .iter()
                        .map(|(name, value)| (name.clone(), Json::str(value)))
                        .collect(),
                ),
            ),
            (
                "preconditions",
                match &self.preconditions {
                    Some(v) => v.to_json_value(),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The operation as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The operation of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let kind = req_str(value, "kind")?;
        Ok(Operation {
            op_id: req_id(value, "op_id")?,
            kind: OperationKind::from_str(&kind)?,
            target: Target::from_json(field(value, "target")?)?,
            template: match value.get("template") {
                None | Some(Json::Null) => None,
                Some(inner) => Some(TemplateRef::from_json(inner)?),
            },
            params: params_of(value, "params")?,
            preconditions: match value.get("preconditions") {
                None | Some(Json::Null) => None,
                Some(inner) => Some(Preconditions::from_json(inner)?),
            },
        })
    }

    /// The value of one parameter of the operation.
    pub fn params_get(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// The request of a plan (design §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRequest {
    /// The version of the contract the caller speaks.
    pub contract_version: String,
    /// The repository the plan works in.
    pub repository_id: RepositoryId,
    /// The ref the plan is built on (`refs/heads/agent-feature`).
    pub target_ref: String,
    /// The commit the plan starts from.
    pub base_commit: String,
    /// The strategy of the plan (`template`, `semantic`, `freeform`).
    pub strategy: String,
    /// The operations of the plan.
    pub operations: Vec<Operation>,
    /// The profile the plan is validated with.
    pub validation_profile: String,
    /// The policy the plan is approved with.
    pub approval_policy: String,
}

impl PlanRequest {
    /// The request as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("contract_version", Json::str(&self.contract_version)),
            ("repository_id", Json::str(self.repository_id.as_str())),
            ("target_ref", Json::str(&self.target_ref)),
            ("base_commit", Json::str(&self.base_commit)),
            ("strategy", Json::str(&self.strategy)),
            (
                "operations",
                Json::Arr(
                    self.operations
                        .iter()
                        .map(Operation::to_json_value)
                        .collect(),
                ),
            ),
            ("validation_profile", Json::str(&self.validation_profile)),
            ("approval_policy", Json::str(&self.approval_policy)),
        ])
    }

    /// The request as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The request of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let operations = req_array(value, "operations")?
            .iter()
            .map(Operation::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PlanRequest {
            contract_version: req_str(value, "contract_version")?,
            repository_id: req_id(value, "repository_id")?,
            target_ref: req_str(value, "target_ref")?,
            base_commit: req_str(value, "base_commit")?,
            strategy: req_str(value, "strategy")?,
            operations,
            validation_profile: req_str(value, "validation_profile")?,
            approval_policy: req_str(value, "approval_policy")?,
        })
    }
}

/// The answer of a plan (design §2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanResponse {
    /// The id of the plan.
    pub plan_id: PlanId,
    /// The digest of the plan (the digest of its canonical JSON).
    pub plan_digest: String,
    /// The version of the contract the answer speaks.
    pub contract_version: String,
    /// How many operations the plan holds.
    pub operation_count: u32,
    /// When the plan was created (RFC 3339, UTC).
    pub created_at: String,
}

impl PlanResponse {
    /// The answer as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("plan_id", Json::str(self.plan_id.as_str())),
            ("plan_digest", Json::str(&self.plan_digest)),
            ("contract_version", Json::str(&self.contract_version)),
            ("operation_count", Json::int(self.operation_count as i64)),
            ("created_at", Json::str(&self.created_at)),
        ])
    }

    /// The answer as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The answer of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(PlanResponse {
            plan_id: req_id(value, "plan_id")?,
            plan_digest: req_str(value, "plan_digest")?,
            contract_version: req_str(value, "contract_version")?,
            operation_count: req_u32(value, "operation_count")?,
            created_at: req_str(value, "created_at")?,
        })
    }
}

/// The request of a preview (design §2.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRequest {
    /// The version of the contract the caller speaks.
    pub contract_version: String,
    /// The plan the preview is made of.
    pub plan_id: PlanId,
    /// The digest the caller expects the plan to have.
    pub expected_plan_digest: String,
    /// The policy of the snapshot (`pinned_base`, ...).
    pub snapshot_policy: String,
    /// Whether the diff of the changes is wanted.
    pub include_diff: bool,
}

impl PreviewRequest {
    /// The request as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("contract_version", Json::str(&self.contract_version)),
            ("plan_id", Json::str(self.plan_id.as_str())),
            (
                "expected_plan_digest",
                Json::str(&self.expected_plan_digest),
            ),
            ("snapshot_policy", Json::str(&self.snapshot_policy)),
            ("include_diff", Json::Bool(self.include_diff)),
        ])
    }

    /// The request as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The request of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(PreviewRequest {
            contract_version: req_str(value, "contract_version")?,
            plan_id: req_id(value, "plan_id")?,
            expected_plan_digest: req_str(value, "expected_plan_digest")?,
            snapshot_policy: req_str(value, "snapshot_policy")?,
            include_diff: req_bool(value, "include_diff")?,
        })
    }
}

/// One change of a preview (design §2.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The path of the file that changed.
    pub path: String,
    /// The symbols the change touches.
    pub symbols: Vec<String>,
    /// How many lines were added.
    pub added_lines: u32,
    /// How many lines were removed.
    pub deleted_lines: u32,
    /// Where the diff of the change is stored.
    pub diff_uri: Option<String>,
}

impl Change {
    /// The change as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("path", Json::str(&self.path)),
            ("symbols", Json::strs(&self.symbols)),
            ("added_lines", Json::int(self.added_lines as i64)),
            ("deleted_lines", Json::int(self.deleted_lines as i64)),
            (
                "diff_uri",
                match &self.diff_uri {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The change as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The change of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(Change {
            path: req_str(value, "path")?,
            symbols: req_strings(value, "symbols")?,
            added_lines: req_u32(value, "added_lines")?,
            deleted_lines: req_u32(value, "deleted_lines")?,
            diff_uri: opt_str(value, "diff_uri")?,
        })
    }
}

/// One check of a preview or of a job (design §2.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// The name of the check (`format`, `parse`, `cargo-check`).
    pub name: String,
    /// The state of the check.
    pub status: CheckStatus,
}

impl Check {
    /// The check as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("name", Json::str(&self.name)),
            ("status", Json::str(self.status.as_str())),
        ])
    }

    /// The check as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The check of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let status = req_str(value, "status")?;
        Ok(Check {
            name: req_str(value, "name")?,
            status: CheckStatus::from_str(&status)?,
        })
    }
}

/// The result of a preview (design §2.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewResult {
    /// The id of the preview.
    pub preview_id: PreviewId,
    /// The plan the preview was made of.
    pub plan_id: PlanId,
    /// The commit the preview starts from.
    pub base_commit: String,
    /// The digest of the tree the preview would produce. It is not the SHA of the commit that does not exist yet.
    pub new_tree_digest: String,
    /// The changes of the preview.
    pub changes: Vec<Change>,
    /// The checks that ran on the preview.
    pub checks: Vec<Check>,
    /// Whether a human has to approve the plan before it is applied.
    pub requires_approval: bool,
    /// When the preview expires (RFC 3339, UTC). A preview must not be reused after that.
    pub expires_at: String,
}

impl PreviewResult {
    /// The result as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("preview_id", Json::str(self.preview_id.as_str())),
            ("plan_id", Json::str(self.plan_id.as_str())),
            ("base_commit", Json::str(&self.base_commit)),
            ("new_tree_digest", Json::str(&self.new_tree_digest)),
            (
                "changes",
                Json::Arr(self.changes.iter().map(Change::to_json_value).collect()),
            ),
            (
                "checks",
                Json::Arr(self.checks.iter().map(Check::to_json_value).collect()),
            ),
            ("requires_approval", Json::Bool(self.requires_approval)),
            ("expires_at", Json::str(&self.expires_at)),
        ])
    }

    /// The result as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The result of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let changes = req_array(value, "changes")?
            .iter()
            .map(Change::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let checks = req_array(value, "checks")?
            .iter()
            .map(Check::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PreviewResult {
            preview_id: req_id(value, "preview_id")?,
            plan_id: req_id(value, "plan_id")?,
            base_commit: req_str(value, "base_commit")?,
            new_tree_digest: req_str(value, "new_tree_digest")?,
            changes,
            checks,
            requires_approval: req_bool(value, "requires_approval")?,
            expires_at: req_str(value, "expires_at")?,
        })
    }
}

/// The request of an apply (design §2.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyRequest {
    /// The version of the contract the caller speaks.
    pub contract_version: String,
    /// The plan that is applied.
    pub plan_id: PlanId,
    /// The preview the caller has seen.
    pub preview_id: PreviewId,
    /// The digest the caller expects the plan to have.
    pub expected_plan_digest: String,
    /// The commit the caller expects to be the base of the plan.
    pub expected_base_commit: String,
    /// The token of the approval, when the plan needs one.
    pub approval_token: Option<String>,
    /// The policy of the commit (`commit_on_green`, ...).
    pub commit_policy: String,
}

impl ApplyRequest {
    /// The request as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("contract_version", Json::str(&self.contract_version)),
            ("plan_id", Json::str(self.plan_id.as_str())),
            ("preview_id", Json::str(self.preview_id.as_str())),
            (
                "expected_plan_digest",
                Json::str(&self.expected_plan_digest),
            ),
            (
                "expected_base_commit",
                Json::str(&self.expected_base_commit),
            ),
            (
                "approval_token",
                match &self.approval_token {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            ("commit_policy", Json::str(&self.commit_policy)),
        ])
    }

    /// The request as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The request of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(ApplyRequest {
            contract_version: req_str(value, "contract_version")?,
            plan_id: req_id(value, "plan_id")?,
            preview_id: req_id(value, "preview_id")?,
            expected_plan_digest: req_str(value, "expected_plan_digest")?,
            expected_base_commit: req_str(value, "expected_base_commit")?,
            approval_token: opt_str(value, "approval_token")?,
            commit_policy: req_str(value, "commit_policy")?,
        })
    }
}

/// The answer of an apply: the job that does the work (design §2.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyAccepted {
    /// The id of the job.
    pub job_id: JobId,
    /// The state the job is in.
    pub state: JobState,
    /// Whether the job already existed (the same idempotency key with the same digest).
    pub deduplicated: bool,
    /// Where the state of the job is read.
    pub status_url: String,
}

impl ApplyAccepted {
    /// The answer as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("job_id", Json::str(self.job_id.as_str())),
            ("state", Json::str(self.state.as_str())),
            ("deduplicated", Json::Bool(self.deduplicated)),
            ("status_url", Json::str(&self.status_url)),
        ])
    }

    /// The answer as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The answer of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let state = req_str(value, "state")?;
        Ok(ApplyAccepted {
            job_id: req_id(value, "job_id")?,
            state: JobState::from_str(&state)?,
            deduplicated: req_bool(value, "deduplicated")?,
            status_url: req_str(value, "status_url")?,
        })
    }
}

/// The counters of a job (design §2.6).
///
/// A counter that the supplier did not report is `null`, never `0`: the difference between "no token was used" and
/// "the supplier did not tell" is the difference between a free operation and an unknown bill.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    /// The tokens that were read.
    pub input_tokens: u64,
    /// The tokens that were written.
    pub output_tokens: u64,
    /// The tokens that were read from the cache of the supplier.
    pub cached_input_tokens: u64,
    /// The tokens of the reasoning, when the supplier reports them apart.
    pub reasoning_tokens: Option<u64>,
    /// What the model cost, in dollars.
    pub llm_cost_usd: Option<f64>,
    /// What the worker cost, in dollars.
    pub worker_cost_usd: Option<f64>,
    /// How long the job took, in milliseconds.
    pub elapsed_ms: Option<u64>,
}

impl Metrics {
    /// The counters as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("input_tokens", Json::int(self.input_tokens as i64)),
            ("output_tokens", Json::int(self.output_tokens as i64)),
            (
                "cached_input_tokens",
                Json::int(self.cached_input_tokens as i64),
            ),
            (
                "reasoning_tokens",
                match self.reasoning_tokens {
                    Some(v) => Json::int(v as i64),
                    None => Json::Null,
                },
            ),
            (
                "llm_cost_usd",
                match self.llm_cost_usd {
                    Some(v) => Json::Num(v),
                    None => Json::Null,
                },
            ),
            (
                "worker_cost_usd",
                match self.worker_cost_usd {
                    Some(v) => Json::Num(v),
                    None => Json::Null,
                },
            ),
            (
                "elapsed_ms",
                match self.elapsed_ms {
                    Some(v) => Json::int(v as i64),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The counters as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The counters of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(Metrics {
            input_tokens: req_u64(value, "input_tokens")?,
            output_tokens: req_u64(value, "output_tokens")?,
            cached_input_tokens: req_u64(value, "cached_input_tokens")?,
            reasoning_tokens: opt_u64(value, "reasoning_tokens")?,
            llm_cost_usd: opt_f64(value, "llm_cost_usd")?,
            worker_cost_usd: opt_f64(value, "worker_cost_usd")?,
            elapsed_ms: opt_u64(value, "elapsed_ms")?,
        })
    }
}

/// The state of a job (design §2.6).
#[derive(Clone, Debug, PartialEq)]
pub struct JobStatus {
    /// The id of the job.
    pub job_id: JobId,
    /// The state the job is in.
    pub state: JobState,
    /// How many times the job ran.
    pub attempt: u32,
    /// The commit the job started from.
    pub base_commit: String,
    /// The commit the job produced, when it produced one.
    pub new_commit: Option<String>,
    /// The checks that ran on the job.
    pub checks: Vec<Check>,
    /// The counters of the job, when the job reports them.
    pub metrics: Option<Metrics>,
}

impl JobStatus {
    /// The state as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("job_id", Json::str(self.job_id.as_str())),
            ("state", Json::str(self.state.as_str())),
            ("attempt", Json::int(self.attempt as i64)),
            ("base_commit", Json::str(&self.base_commit)),
            (
                "new_commit",
                match &self.new_commit {
                    Some(v) => Json::str(v),
                    None => Json::Null,
                },
            ),
            (
                "checks",
                Json::Arr(self.checks.iter().map(Check::to_json_value).collect()),
            ),
            (
                "metrics",
                match &self.metrics {
                    Some(v) => v.to_json_value(),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// The state as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The state of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let state = req_str(value, "state")?;
        let checks = req_array(value, "checks")?
            .iter()
            .map(Check::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(JobStatus {
            job_id: req_id(value, "job_id")?,
            state: JobState::from_str(&state)?,
            attempt: req_u32(value, "attempt")?,
            base_commit: req_str(value, "base_commit")?,
            new_commit: opt_str(value, "new_commit")?,
            checks,
            metrics: match value.get("metrics") {
                None | Some(Json::Null) => None,
                Some(inner) => Some(Metrics::from_json(inner)?),
            },
        })
    }
}

/// The query of a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobQuery {
    /// The id of the job.
    pub job_id: JobId,
}

impl JobQuery {
    /// The query as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![("job_id", Json::str(self.job_id.as_str()))])
    }

    /// The query as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The query of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(JobQuery {
            job_id: req_id(value, "job_id")?,
        })
    }
}

/// The name of an asset in the two languages of the catalog (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetName {
    /// The name in Chinese (`zh-CN`).
    pub zh_cn: String,
    /// The name in English (`en-US`).
    pub en_us: String,
}

impl AssetName {
    /// The name as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("zh-CN", Json::str(&self.zh_cn)),
            ("en-US", Json::str(&self.en_us)),
        ])
    }

    /// The name as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The name of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(AssetName {
            zh_cn: req_str(value, "zh-CN")?,
            en_us: req_str(value, "en-US")?,
        })
    }
}

/// When an asset applies (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetApplicability {
    /// The conditions the asset needs.
    pub constraints: Vec<(String, String)>,
    /// The cases the asset does not solve.
    pub counterexamples: Vec<String>,
    /// The assets that solve those cases instead.
    pub alternatives: Vec<String>,
}

impl AssetApplicability {
    /// The applicability as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            (
                "constraints",
                Json::Obj(
                    self.constraints
                        .iter()
                        .map(|(name, value)| (name.clone(), Json::str(value)))
                        .collect(),
                ),
            ),
            ("counterexamples", Json::strs(&self.counterexamples)),
            ("alternatives", Json::strs(&self.alternatives)),
        ])
    }

    /// The applicability as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The applicability of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(AssetApplicability {
            constraints: params_of(value, "constraints")?,
            counterexamples: req_strings(value, "counterexamples")?,
            alternatives: req_strings(value, "alternatives")?,
        })
    }
}

/// The cost of an asset (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetComplexity {
    /// The time it takes.
    pub time: String,
    /// The memory it needs on top of the input.
    pub extra_space: String,
}

impl AssetComplexity {
    /// The complexity as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("time", Json::str(&self.time)),
            ("extra_space", Json::str(&self.extra_space)),
        ])
    }

    /// The complexity as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The complexity of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(AssetComplexity {
            time: req_str(value, "time")?,
            extra_space: req_str(value, "extra_space")?,
        })
    }
}

/// One implementation of an asset (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetImplementation {
    /// The id of the implementation.
    pub implementation_id: ImplementationId,
    /// The language of the implementation.
    pub language: String,
    /// The version of the implementation.
    pub version: String,
    /// The state of the approval of the implementation.
    pub status: String,
    /// The engine that renders it.
    pub engine: String,
    /// Where the implementation lives.
    pub path: String,
    /// The digest of the package of the implementation.
    pub sha256: String,
    /// Where the schema of its parameters lives.
    pub parameters_schema: String,
    /// Where the manifest of its tests lives.
    pub tests_manifest: String,
}

impl AssetImplementation {
    /// The implementation as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            (
                "implementation_id",
                Json::str(self.implementation_id.as_str()),
            ),
            ("language", Json::str(&self.language)),
            ("version", Json::str(&self.version)),
            ("status", Json::str(&self.status)),
            ("engine", Json::str(&self.engine)),
            ("path", Json::str(&self.path)),
            ("sha256", Json::str(&self.sha256)),
            ("parameters_schema", Json::str(&self.parameters_schema)),
            ("tests_manifest", Json::str(&self.tests_manifest)),
        ])
    }

    /// The implementation as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The implementation of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(AssetImplementation {
            implementation_id: req_id(value, "implementation_id")?,
            language: req_str(value, "language")?,
            version: req_str(value, "version")?,
            status: req_str(value, "status")?,
            engine: req_str(value, "engine")?,
            path: req_str(value, "path")?,
            sha256: req_str(value, "sha256")?,
            parameters_schema: req_str(value, "parameters_schema")?,
            tests_manifest: req_str(value, "tests_manifest")?,
        })
    }
}

/// Where an asset comes from (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetProvenance {
    /// The references of the asset.
    pub references: Vec<String>,
    /// The license of the asset.
    pub license: String,
}

impl AssetProvenance {
    /// The provenance as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("references", Json::strs(&self.references)),
            ("license", Json::str(&self.license)),
        ])
    }

    /// The provenance as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The provenance of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(AssetProvenance {
            references: req_strings(value, "references")?,
            license: req_str(value, "license")?,
        })
    }
}

/// The manifest of an asset of the catalog (design §19.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetManifest {
    /// The version of the schema of the manifest.
    pub schema_version: String,
    /// The id of the asset.
    pub asset_id: AssetId,
    /// The version of the asset.
    pub version: String,
    /// The kind of the asset (`algorithm`, `pattern`, ...).
    pub kind: String,
    /// The name of the asset in the two languages.
    pub name: AssetName,
    /// The other names of the asset.
    pub aliases: Vec<String>,
    /// The category of the asset.
    pub category: String,
    /// The state of the approval of the asset.
    pub status: String,
    /// What the asset is for.
    pub intent: String,
    /// When the asset applies.
    pub applicability: AssetApplicability,
    /// What the asset costs.
    pub complexity: AssetComplexity,
    /// The implementations of the asset.
    pub implementations: Vec<AssetImplementation>,
    /// Where the asset comes from.
    pub provenance: AssetProvenance,
}

impl AssetManifest {
    /// The manifest as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("schema_version", Json::str(&self.schema_version)),
            ("asset_id", Json::str(self.asset_id.as_str())),
            ("version", Json::str(&self.version)),
            ("kind", Json::str(&self.kind)),
            ("name", self.name.to_json_value()),
            ("aliases", Json::strs(&self.aliases)),
            ("category", Json::str(&self.category)),
            ("status", Json::str(&self.status)),
            ("intent", Json::str(&self.intent)),
            ("applicability", self.applicability.to_json_value()),
            ("complexity", self.complexity.to_json_value()),
            (
                "implementations",
                Json::Arr(
                    self.implementations
                        .iter()
                        .map(AssetImplementation::to_json_value)
                        .collect(),
                ),
            ),
            ("provenance", self.provenance.to_json_value()),
        ])
    }

    /// The manifest as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The manifest of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let implementations = req_array(value, "implementations")?
            .iter()
            .map(AssetImplementation::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AssetManifest {
            schema_version: req_str(value, "schema_version")?,
            asset_id: req_id(value, "asset_id")?,
            version: req_str(value, "version")?,
            kind: req_str(value, "kind")?,
            name: AssetName::from_json(field(value, "name")?)?,
            aliases: req_strings(value, "aliases")?,
            category: req_str(value, "category")?,
            status: req_str(value, "status")?,
            intent: req_str(value, "intent")?,
            applicability: AssetApplicability::from_json(field(value, "applicability")?)?,
            complexity: AssetComplexity::from_json(field(value, "complexity")?)?,
            implementations,
            provenance: AssetProvenance::from_json(field(value, "provenance")?)?,
        })
    }
}

/// What a template package is allowed to do (design §3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplatePermission {
    /// The file system scopes the template may write in.
    pub filesystem: Vec<String>,
    /// Whether the template may reach the network (`deny`, `allowlist`).
    pub network: String,
    /// Whether the template may start a process (`deny`, `allowlist`).
    pub subprocess: String,
}

impl TemplatePermission {
    /// The permissions as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("filesystem", Json::strs(&self.filesystem)),
            ("network", Json::str(&self.network)),
            ("subprocess", Json::str(&self.subprocess)),
        ])
    }

    /// The permissions as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The permissions of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(TemplatePermission {
            filesystem: req_strings(value, "filesystem")?,
            network: req_str(value, "network")?,
            subprocess: req_str(value, "subprocess")?,
        })
    }
}

/// One file a template produces (design §3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateOutput {
    /// The pattern of the path of the file (`src/api/{entity_snake}.rs`).
    pub path_pattern: String,
    /// The fragment that fills the file.
    pub content: String,
    /// How the file is merged into the tree (`semantic_insert`, ...).
    pub update_mode: String,
}

impl TemplateOutput {
    /// The output as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("path_pattern", Json::str(&self.path_pattern)),
            ("content", Json::str(&self.content)),
            ("update_mode", Json::str(&self.update_mode)),
        ])
    }

    /// The output as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The output of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(TemplateOutput {
            path_pattern: req_str(value, "path_pattern")?,
            content: req_str(value, "content")?,
            update_mode: req_str(value, "update_mode")?,
        })
    }
}

/// One golden fixture of a template (design §3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateFixture {
    /// Where the input of the fixture is.
    pub input: String,
    /// Where the expected output of the fixture is.
    pub expected: String,
}

impl TemplateFixture {
    /// The fixture as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("input", Json::str(&self.input)),
            ("expected", Json::str(&self.expected)),
        ])
    }

    /// The fixture as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The fixture of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        Ok(TemplateFixture {
            input: req_str(value, "input")?,
            expected: req_str(value, "expected")?,
        })
    }
}

/// The manifest of a template package (design §3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplatePackageManifest {
    /// The version of the schema of the manifest.
    pub schema_version: String,
    /// The id of the template.
    pub id: TemplateId,
    /// The version of the template.
    pub version: String,
    /// The language of the template.
    pub language: String,
    /// The engine that renders the template.
    pub engine: String,
    /// The version of the engine.
    pub engine_version: String,
    /// The license of the template.
    pub license: String,
    /// Where the template comes from (`internal-reviewed`).
    pub provenance: String,
    /// The state of the template.
    pub status: String,
    /// What the template is allowed to do.
    pub permissions: TemplatePermission,
    /// Where the schema of the parameters of the template lives.
    pub parameters_schema: String,
    /// The files the template produces.
    pub outputs: Vec<TemplateOutput>,
    /// The checks that run on the result.
    pub validation: Vec<String>,
    /// The golden fixtures of the template.
    pub fixtures: Vec<TemplateFixture>,
}

impl TemplatePackageManifest {
    /// The manifest as the JSON that is sent.
    pub fn to_json_value(&self) -> Json {
        Json::obj(vec![
            ("schema_version", Json::str(&self.schema_version)),
            ("id", Json::str(self.id.as_str())),
            ("version", Json::str(&self.version)),
            ("language", Json::str(&self.language)),
            ("engine", Json::str(&self.engine)),
            ("engine_version", Json::str(&self.engine_version)),
            ("license", Json::str(&self.license)),
            ("provenance", Json::str(&self.provenance)),
            ("status", Json::str(&self.status)),
            ("permissions", self.permissions.to_json_value()),
            ("parameters_schema", Json::str(&self.parameters_schema)),
            (
                "outputs",
                Json::Arr(
                    self.outputs
                        .iter()
                        .map(TemplateOutput::to_json_value)
                        .collect(),
                ),
            ),
            ("validation", Json::strs(&self.validation)),
            (
                "fixtures",
                Json::Arr(
                    self.fixtures
                        .iter()
                        .map(TemplateFixture::to_json_value)
                        .collect(),
                ),
            ),
        ])
    }

    /// The manifest as the JSON that is sent.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_json_string()
    }

    /// The manifest of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let outputs = req_array(value, "outputs")?
            .iter()
            .map(TemplateOutput::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let fixtures = req_array(value, "fixtures")?
            .iter()
            .map(TemplateFixture::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TemplatePackageManifest {
            schema_version: req_str(value, "schema_version")?,
            id: req_id(value, "id")?,
            version: req_str(value, "version")?,
            language: req_str(value, "language")?,
            engine: req_str(value, "engine")?,
            engine_version: req_str(value, "engine_version")?,
            license: req_str(value, "license")?,
            provenance: req_str(value, "provenance")?,
            status: req_str(value, "status")?,
            permissions: TemplatePermission::from_json(field(value, "permissions")?)?,
            parameters_schema: req_str(value, "parameters_schema")?,
            outputs,
            validation: req_strings(value, "validation")?,
            fixtures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::CONTRACT_VERSION;
    use pime_core::json::{Limits, parse};
    use std::collections::BTreeSet;

    /// The DTOs that can be written and read again.
    trait JsonRoundTrip: Sized + PartialEq + std::fmt::Debug {
        /// The DTO as a JSON text.
        fn as_json_text(&self) -> String;
        /// The DTO of a JSON value.
        fn read_json(value: &Json) -> Result<Self, TaseError>;
    }

    /// Implements [`JsonRoundTrip`] for the DTOs.
    macro_rules! impl_round_trip {
        ($($ty:ty),* $(,)?) => {
            $(
                impl JsonRoundTrip for $ty {
                    fn as_json_text(&self) -> String {
                        <$ty>::to_json(self)
                    }

                    fn read_json(value: &Json) -> Result<Self, TaseError> {
                        <$ty>::from_json(value)
                    }
                }
            )*
        };
    }

    impl_round_trip!(
        RequestEnvelope,
        TemplateQuery,
        TemplateSummary,
        TemplateSearchResult,
        Target,
        TemplateRef,
        Preconditions,
        Operation,
        PlanRequest,
        PlanResponse,
        PreviewRequest,
        Change,
        Check,
        PreviewResult,
        ApplyRequest,
        ApplyAccepted,
        Metrics,
        JobStatus,
        JobQuery,
        AssetName,
        AssetApplicability,
        AssetComplexity,
        AssetImplementation,
        AssetProvenance,
        AssetManifest,
        TemplatePermission,
        TemplateOutput,
        TemplateFixture,
        TemplatePackageManifest,
    );

    /// Checks that a DTO survives the round trip through its JSON.
    fn round_trip<T: JsonRoundTrip>(dto: &T) -> T {
        let text = dto.as_json_text();
        let value =
            parse(&text, Limits::DEFAULT).expect("the DTO writes a JSON the parser accepts");
        T::read_json(&value).expect("the DTO reads its own JSON")
    }

    fn sample_envelope() -> RequestEnvelope {
        RequestEnvelope {
            contract_version: CONTRACT_VERSION.to_string(),
            request_id: RequestId::parse("req_01HXYZ").unwrap(),
            tenant_id: TenantId::parse("tn_demo").unwrap(),
            workspace_id: WorkspaceId::parse("ws_demo").unwrap(),
            repository_id: RepositoryId::parse("repo_ide1").unwrap(),
            agent_id: AgentId::parse("agent_17").unwrap(),
            trace_id: TraceId::parse("trace_01HXYZ").unwrap(),
        }
    }

    fn sample_template_summary() -> TemplateSummary {
        TemplateSummary {
            id: TemplateId::parse("rust.handler.create").unwrap(),
            version: "1.0.0".to_string(),
            digest: "sha256:012345".to_string(),
            language: "rust".to_string(),
            tags: vec!["handler".to_string(), "crud".to_string()],
            params_schema_ref: "artifact://templates/rust.handler.create/1.0.0/params.schema.json"
                .to_string(),
            approval_status: "ACTIVE".to_string(),
            license: "MIT".to_string(),
        }
    }

    fn sample_template_search_result() -> TemplateSearchResult {
        TemplateSearchResult {
            templates: vec![sample_template_summary()],
            next_cursor: None,
        }
    }

    fn sample_operation() -> Operation {
        Operation {
            op_id: OperationId::parse("op_001").unwrap(),
            kind: OperationKind::InstantiateTemplate,
            target: Target {
                path: "src/api/user.rs".to_string(),
                symbol: Some("crate::api::user::create_user".to_string()),
                symbol_kind: Some("function".to_string()),
            },
            template: Some(TemplateRef {
                id: TemplateId::parse("rust.handler.create").unwrap(),
                version: "1.0.0".to_string(),
                digest: "sha256:FULL64HEX".to_string(),
            }),
            params: vec![
                ("entity".to_string(), "User".to_string()),
                ("database".to_string(), "postgres".to_string()),
                ("authorization".to_string(), "true".to_string()),
            ],
            preconditions: Some(Preconditions {
                file_blob_sha: Some("GIT_BLOB_SHA".to_string()),
                symbol_signature_hash: Some("sha256:FULL64HEX".to_string()),
                expected_symbol_exists: Some(false),
            }),
        }
    }

    fn sample_plan_request() -> PlanRequest {
        PlanRequest {
            contract_version: CONTRACT_VERSION.to_string(),
            repository_id: RepositoryId::parse("repo_ide1").unwrap(),
            target_ref: "refs/heads/agent-feature".to_string(),
            base_commit: "a1b2c3d4fullsha".to_string(),
            strategy: "template".to_string(),
            operations: vec![sample_operation()],
            validation_profile: "rust-ci-default".to_string(),
            approval_policy: "project-default".to_string(),
        }
    }

    fn sample_preview_result() -> PreviewResult {
        PreviewResult {
            preview_id: PreviewId::parse("prev_001").unwrap(),
            plan_id: PlanId::parse("plan_001").unwrap(),
            base_commit: "a1b2c3d4fullsha".to_string(),
            new_tree_digest: "sha256:FULL64HEX".to_string(),
            changes: vec![Change {
                path: "src/api/user.rs".to_string(),
                symbols: vec!["crate::api::user::create_user".to_string()],
                added_lines: 83,
                deleted_lines: 0,
                diff_uri: Some("artifact://diff/prev_001/user".to_string()),
            }],
            checks: vec![
                Check {
                    name: "format".to_string(),
                    status: CheckStatus::Pass,
                },
                Check {
                    name: "parse".to_string(),
                    status: CheckStatus::Pass,
                },
            ],
            requires_approval: false,
            expires_at: "2026-11-05T12:10:00Z".to_string(),
        }
    }

    fn sample_metrics() -> Metrics {
        Metrics {
            input_tokens: 640,
            output_tokens: 96,
            cached_input_tokens: 400,
            reasoning_tokens: None,
            llm_cost_usd: None,
            worker_cost_usd: None,
            elapsed_ms: Some(2840),
        }
    }

    fn sample_job_status() -> JobStatus {
        JobStatus {
            job_id: JobId::parse("job_001").unwrap(),
            state: JobState::Committed,
            attempt: 1,
            base_commit: "a1b2c3d4fullsha".to_string(),
            new_commit: Some("e5f6fullsha".to_string()),
            checks: vec![Check {
                name: "cargo-check".to_string(),
                status: CheckStatus::Pass,
            }],
            metrics: Some(sample_metrics()),
        }
    }

    fn sample_asset_manifest() -> AssetManifest {
        AssetManifest {
            schema_version: crate::version::ASSET_SCHEMA_VERSION.to_string(),
            asset_id: AssetId::parse("algorithm.graph.dijkstra").unwrap(),
            version: "1.0.0".to_string(),
            kind: "algorithm".to_string(),
            name: AssetName {
                zh_cn: "Dijkstra 最短路径".to_string(),
                en_us: "Dijkstra Shortest Path".to_string(),
            },
            aliases: vec!["最短路径".to_string(), "shortest_path".to_string()],
            category: "graph".to_string(),
            status: "APPROVED_CARD".to_string(),
            intent: "单源最短路径，边权非负".to_string(),
            applicability: AssetApplicability {
                constraints: vec![
                    ("directed_graph".to_string(), "optional".to_string()),
                    ("weight_type".to_string(), "nonnegative".to_string()),
                    ("negative_edges".to_string(), "forbidden".to_string()),
                    (
                        "output".to_string(),
                        "distances_and_predecessors".to_string(),
                    ),
                ],
                counterexamples: vec!["存在负权边时不应使用 Dijkstra".to_string()],
                alternatives: vec!["algorithm.graph.bellman_ford".to_string()],
            },
            complexity: AssetComplexity {
                time: "O((V + E) log V)".to_string(),
                extra_space: "O(V + E)".to_string(),
            },
            implementations: vec![AssetImplementation {
                implementation_id: ImplementationId::parse(
                    "algorithm.graph.dijkstra.rust.binary_heap",
                )
                .unwrap(),
                language: "rust".to_string(),
                version: "1.0.0".to_string(),
                status: "APPROVED".to_string(),
                engine: "tase-template".to_string(),
                path: "templates/rust/graph/dijkstra".to_string(),
                sha256: "release-sha256-placeholder".to_string(),
                parameters_schema: "schemas/dijkstra-params.json".to_string(),
                tests_manifest: "tests/dijkstra.yaml".to_string(),
            }],
            provenance: AssetProvenance {
                references: vec![
                    "https://en.wikipedia.org/wiki/Dijkstra%27s_algorithm".to_string(),
                ],
                license: "MIT".to_string(),
            },
        }
    }

    fn sample_package_manifest() -> TemplatePackageManifest {
        TemplatePackageManifest {
            schema_version: "1.0".to_string(),
            id: TemplateId::parse("rust.handler.create").unwrap(),
            version: "1.0.0".to_string(),
            language: "rust".to_string(),
            engine: "tase-safe-template".to_string(),
            engine_version: "1.0.0".to_string(),
            license: "MIT".to_string(),
            provenance: "internal-reviewed".to_string(),
            status: "ACTIVE".to_string(),
            permissions: TemplatePermission {
                filesystem: vec!["workspace_write_scoped".to_string()],
                network: "deny".to_string(),
                subprocess: "deny".to_string(),
            },
            parameters_schema: "params.schema.json".to_string(),
            outputs: vec![TemplateOutput {
                path_pattern: "src/api/{entity_snake}.rs".to_string(),
                content: "fragments/handler.rs.tmpl".to_string(),
                update_mode: "semantic_insert".to_string(),
            }],
            validation: vec!["rustfmt".to_string(), "cargo_check".to_string()],
            fixtures: vec![TemplateFixture {
                input: "fixtures/input".to_string(),
                expected: "fixtures/expected".to_string(),
            }],
        }
    }

    /// The names of the fields of a serialized DTO.
    ///
    /// "params" and "constraints" are free-form tables: their keys are data, not fields, so they are not descended
    /// into.
    fn dto_field_names(value: &Json) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        collect_dto_field_names(value, &mut names);
        names
    }

    fn collect_dto_field_names(value: &Json, names: &mut BTreeSet<String>) {
        match value {
            Json::Obj(pairs) => {
                for (key, child) in pairs {
                    names.insert(key.clone());
                    if !matches!(key.as_str(), "params" | "constraints") {
                        collect_dto_field_names(child, names);
                    }
                }
            }
            Json::Arr(items) => {
                for item in items {
                    collect_dto_field_names(item, names);
                }
            }
            _ => {}
        }
    }

    /// The names of the properties of a JSON Schema document.
    fn schema_property_names(schema: &Json) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        collect_schema_property_names(schema, &mut names);
        names
    }

    fn collect_schema_property_names(value: &Json, names: &mut BTreeSet<String>) {
        match value {
            Json::Obj(pairs) => {
                for (key, child) in pairs {
                    if key == "properties"
                        && let Json::Obj(properties) = child
                    {
                        for (name, _) in properties {
                            names.insert(name.clone());
                        }
                    }
                    collect_schema_property_names(child, names);
                }
            }
            Json::Arr(items) => {
                for item in items {
                    collect_schema_property_names(item, names);
                }
            }
            _ => {}
        }
    }

    /// The schema of a name, parsed.
    fn parsed_schema(name: &str) -> Json {
        let text = crate::schema(name).unwrap_or_else(|| panic!("the schema {name} must exist"));
        parse(text, Limits::DEFAULT).expect("the schema must be a JSON the parser accepts")
    }

    #[test]
    fn the_envelope_survives_the_round_trip() {
        let envelope = sample_envelope();
        assert_eq!(round_trip(&envelope), envelope);
    }

    #[test]
    fn the_template_query_survives_the_round_trip() {
        let query = TemplateQuery {
            language: Some("rust".to_string()),
            tag: Some("crud".to_string()),
            limit: Some(20),
            cursor: Some("c2".to_string()),
        };
        assert_eq!(round_trip(&query), query);
        let empty = TemplateQuery::default();
        assert_eq!(round_trip(&empty), empty);
    }

    #[test]
    fn the_template_summary_survives_the_round_trip() {
        let summary = sample_template_summary();
        assert_eq!(round_trip(&summary), summary);
    }

    #[test]
    fn the_template_search_result_survives_the_round_trip() {
        let result = TemplateSearchResult {
            templates: vec![sample_template_summary()],
            next_cursor: None,
        };
        assert_eq!(round_trip(&result), result);
    }

    #[test]
    fn the_target_survives_the_round_trip() {
        let target = sample_operation().target;
        assert_eq!(round_trip(&target), target);
    }

    #[test]
    fn the_template_reference_survives_the_round_trip() {
        let reference = sample_operation().template.unwrap();
        assert_eq!(round_trip(&reference), reference);
    }

    #[test]
    fn the_preconditions_survive_the_round_trip() {
        let preconditions = sample_operation().preconditions.unwrap();
        assert_eq!(round_trip(&preconditions), preconditions);
        let empty = Preconditions::default();
        assert_eq!(round_trip(&empty), empty);
    }

    #[test]
    fn the_operation_survives_the_round_trip() {
        let operation = sample_operation();
        assert_eq!(round_trip(&operation), operation);
    }

    #[test]
    fn the_plan_request_survives_the_round_trip() {
        let request = sample_plan_request();
        assert_eq!(round_trip(&request), request);
    }

    #[test]
    fn the_plan_response_survives_the_round_trip() {
        let response = PlanResponse {
            plan_id: PlanId::parse("plan_001").unwrap(),
            plan_digest: "sha256:FULL64HEX".to_string(),
            contract_version: CONTRACT_VERSION.to_string(),
            operation_count: 1,
            created_at: "2026-11-05T12:00:00Z".to_string(),
        };
        assert_eq!(round_trip(&response), response);
    }

    #[test]
    fn the_preview_request_survives_the_round_trip() {
        let request = PreviewRequest {
            contract_version: CONTRACT_VERSION.to_string(),
            plan_id: PlanId::parse("plan_001").unwrap(),
            expected_plan_digest: "sha256:FULL64HEX".to_string(),
            snapshot_policy: "pinned_base".to_string(),
            include_diff: true,
        };
        assert_eq!(round_trip(&request), request);
    }

    #[test]
    fn the_change_survives_the_round_trip() {
        let change = sample_preview_result().changes.into_iter().next().unwrap();
        assert_eq!(round_trip(&change), change);
    }

    #[test]
    fn the_check_survives_the_round_trip() {
        let check = Check {
            name: "format".to_string(),
            status: CheckStatus::Pass,
        };
        assert_eq!(round_trip(&check), check);
    }

    #[test]
    fn the_preview_result_survives_the_round_trip() {
        let result = sample_preview_result();
        assert_eq!(round_trip(&result), result);
    }

    #[test]
    fn the_apply_request_survives_the_round_trip() {
        let request = ApplyRequest {
            contract_version: CONTRACT_VERSION.to_string(),
            plan_id: PlanId::parse("plan_001").unwrap(),
            preview_id: PreviewId::parse("prev_001").unwrap(),
            expected_plan_digest: "sha256:FULL64HEX".to_string(),
            expected_base_commit: "a1b2c3d4fullsha".to_string(),
            approval_token: None,
            commit_policy: "commit_on_green".to_string(),
        };
        assert_eq!(round_trip(&request), request);
    }

    #[test]
    fn the_apply_answer_survives_the_round_trip() {
        let accepted = ApplyAccepted {
            job_id: JobId::parse("job_001").unwrap(),
            state: JobState::Queued,
            deduplicated: false,
            status_url: "/api/v1/tase/jobs/job_001".to_string(),
        };
        assert_eq!(round_trip(&accepted), accepted);
    }

    #[test]
    fn the_metrics_survive_the_round_trip() {
        let metrics = sample_metrics();
        assert_eq!(round_trip(&metrics), metrics);
        let with_cost = Metrics {
            llm_cost_usd: Some(0.25),
            worker_cost_usd: Some(0.5),
            reasoning_tokens: Some(12),
            ..metrics
        };
        assert_eq!(round_trip(&with_cost), with_cost);
    }

    #[test]
    fn the_job_status_survives_the_round_trip() {
        let status = sample_job_status();
        assert_eq!(round_trip(&status), status);
    }

    #[test]
    fn the_job_query_survives_the_round_trip() {
        let query = JobQuery {
            job_id: JobId::parse("job_001").unwrap(),
        };
        assert_eq!(round_trip(&query), query);
    }

    #[test]
    fn the_asset_manifest_survives_the_round_trip() {
        let manifest = sample_asset_manifest();
        assert_eq!(round_trip(&manifest), manifest);
    }

    #[test]
    fn the_package_manifest_survives_the_round_trip() {
        let manifest = sample_package_manifest();
        assert_eq!(round_trip(&manifest), manifest);
    }

    #[test]
    fn the_parts_of_the_manifests_survive_the_round_trip() {
        let name = sample_asset_manifest().name;
        assert_eq!(round_trip(&name), name);
        let applicability = sample_asset_manifest().applicability;
        assert_eq!(round_trip(&applicability), applicability);
        let complexity = sample_asset_manifest().complexity;
        assert_eq!(round_trip(&complexity), complexity);
        let implementation = sample_asset_manifest()
            .implementations
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(round_trip(&implementation), implementation);
        let provenance = sample_asset_manifest().provenance;
        assert_eq!(round_trip(&provenance), provenance);
        let permission = sample_package_manifest().permissions;
        assert_eq!(round_trip(&permission), permission);
        let output = sample_package_manifest()
            .outputs
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(round_trip(&output), output);
        let fixture = sample_package_manifest()
            .fixtures
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(round_trip(&fixture), fixture);
    }

    #[test]
    fn the_kind_of_an_operation_knows_its_wire_values() {
        let names = [
            "instantiate_template",
            "insert_sibling",
            "replace_symbol_body",
            "delete_symbol",
            "update_imports",
            "format",
        ];
        assert_eq!(OperationKind::ALL.len(), names.len());
        for kind in OperationKind::ALL {
            assert!(names.contains(&kind.as_str()), "{kind:?}");
            assert_eq!(OperationKind::from_str(kind.as_str()).unwrap(), kind);
            assert_eq!(kind.as_str().parse::<OperationKind>().unwrap(), kind);
        }
        let unknown = OperationKind::from_str("nonsense").unwrap_err();
        assert_eq!(unknown.code, crate::error::ErrorCode::InvalidArgument);
        assert!("nonsense".parse::<OperationKind>().is_err());
    }

    #[test]
    fn the_state_of_a_check_knows_its_wire_values() {
        let names = ["PASS", "FAIL", "SKIPPED", "NOT_RUN"];
        assert_eq!(CheckStatus::ALL.len(), names.len());
        for status in CheckStatus::ALL {
            assert!(names.contains(&status.as_str()), "{status:?}");
            assert_eq!(CheckStatus::from_str(status.as_str()).unwrap(), status);
            assert_eq!(status.as_str().parse::<CheckStatus>().unwrap(), status);
        }
        assert!(CheckStatus::from_str("pass").is_err());
        assert!(CheckStatus::from_str("").is_err());
    }

    #[test]
    fn the_state_of_a_job_knows_its_wire_values() {
        let names = [
            "QUEUED",
            "RUNNING",
            "VALIDATING",
            "COMMITTING",
            "COMMITTED",
            "FAILED",
            "CANCELLED",
            "REBASED",
        ];
        assert_eq!(JobState::ALL.len(), names.len());
        for state in JobState::ALL {
            assert!(names.contains(&state.as_str()), "{state:?}");
            assert_eq!(JobState::from_str(state.as_str()).unwrap(), state);
            assert_eq!(state.as_str().parse::<JobState>().unwrap(), state);
        }
        assert!(JobState::from_str("queued").is_err());
        assert!(JobState::from_str("DONE").is_err());
    }

    #[test]
    fn a_missing_field_is_refused() {
        let value = parse(r#"{"contract_version":"1.1.0"}"#, Limits::DEFAULT).unwrap();
        let error = RequestEnvelope::from_json(&value).unwrap_err();
        assert_eq!(error.code, crate::error::ErrorCode::InvalidArgument);
        assert!(error.message.contains("request_id"), "{}", error.message);
    }

    #[test]
    fn a_field_of_the_wrong_type_is_refused() {
        let value = parse(r#"{"contract_version":7}"#, Limits::DEFAULT).unwrap();
        let error = RequestEnvelope::from_json(&value).unwrap_err();
        assert_eq!(error.code, crate::error::ErrorCode::InvalidArgument);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        let text = r#"{
            "contract_version":"1.1.0",
            "request_id":"req 01",
            "tenant_id":"tn_demo",
            "workspace_id":"ws_demo",
            "repository_id":"repo_ide1",
            "agent_id":"agent_17",
            "trace_id":"trace_01HXYZ"
        }"#;
        let value = parse(text, Limits::DEFAULT).unwrap();
        let error = RequestEnvelope::from_json(&value).unwrap_err();
        assert_eq!(error.code, crate::error::ErrorCode::InvalidArgument);
        assert!(error.message.contains("request_id"), "{}", error.message);
    }

    #[test]
    fn a_wire_value_that_does_not_exist_is_refused() {
        let text = r#"{"job_id":"job_001","state":"DONE","attempt":1,"base_commit":"a","new_commit":null,"checks":[],"metrics":null}"#;
        let value = parse(text, Limits::DEFAULT).unwrap();
        let error = JobStatus::from_json(&value).unwrap_err();
        assert_eq!(error.code, crate::error::ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_parameters_keep_their_order() {
        let operation = sample_operation();
        assert_eq!(operation.params_get("entity"), Some("User"));
        assert_eq!(operation.params_get("authorization"), Some("true"));
        assert_eq!(operation.params_get("missing"), None);
        let text = operation.to_json();
        let entity = text.find("entity").expect("the parameter is written");
        let database = text.find("database").expect("the parameter is written");
        let authorization = text
            .find("authorization")
            .expect("the parameter is written");
        assert!(
            entity < database && database < authorization,
            "the order must be kept"
        );
        assert_eq!(round_trip(&operation), operation);
    }

    #[test]
    fn a_string_is_escaped_and_a_non_ascii_one_is_written_as_utf8() {
        let mut request = sample_plan_request();
        request.target_ref = "refs/heads/a\"b\\c\nd\u{1}功能-🎉".to_string();
        let text = request.to_json();
        assert!(text.contains(r#"\"#), "the quote must be escaped");
        assert!(text.contains(r"\\"), "the backslash must be escaped");
        assert!(text.contains(r"\n"), "the newline must be escaped");
        assert!(
            text.contains(r"\u0001"),
            "the control character must be escaped"
        );
        assert!(
            text.contains("功能-🎉"),
            "the non ascii text must be written as UTF-8"
        );
        assert_eq!(round_trip(&request), request);
    }

    #[test]
    fn the_plan_schema_matches_the_plan_request() {
        let schema = parsed_schema("plan");
        let dto = dto_field_names(&sample_plan_request().to_json_value());
        assert_eq!(schema_property_names(&schema), dto);
    }

    #[test]
    fn the_preview_schema_matches_the_preview_result() {
        let schema = parsed_schema("preview");
        let dto = dto_field_names(&sample_preview_result().to_json_value());
        assert_eq!(schema_property_names(&schema), dto);
    }

    #[test]
    fn the_job_schema_matches_the_job_status() {
        let schema = parsed_schema("job");
        let dto = dto_field_names(&sample_job_status().to_json_value());
        assert_eq!(schema_property_names(&schema), dto);
    }

    #[test]
    fn the_template_schema_matches_the_templates() {
        let schema = parsed_schema("template");
        let mut dto = dto_field_names(&sample_template_search_result().to_json_value());
        dto.extend(dto_field_names(&sample_asset_manifest().to_json_value()));
        dto.extend(dto_field_names(&sample_package_manifest().to_json_value()));
        assert_eq!(schema_property_names(&schema), dto);
    }

    #[test]
    fn the_four_schemas_are_json_and_name_their_fields() {
        for name in ["plan", "template", "job", "preview"] {
            let schema = parsed_schema(name);
            assert!(matches!(schema, Json::Obj(_)), "{name} must be an object");
            assert!(
                schema.get("$schema").and_then(Json::as_str)
                    == Some("https://json-schema.org/draft/2020-12/schema"),
                "{name} must be a draft 2020-12 schema"
            );
            assert!(
                !schema_property_names(&schema).is_empty(),
                "{name} must have properties"
            );
        }
    }
}
