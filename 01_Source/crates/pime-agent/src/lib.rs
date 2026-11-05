//! The agent API of the input method: one set of operations, three front-ends.
//!
//! The input method is a TSF text service: it lives **inside** the process of the application that has the focus, so
//! an external agent (an MCP client, a CLI, a script) cannot write into the focused document by itself. It asks the
//! text service through the bridge ([`bridge`]: a `WM_COPYDATA` message to the window the text service owns) and it
//! asks the engine through the core ([`core`]: JSON Lines to `pime-core.exe`).
//!
//! Everything an agent can do is here: [`dispatch_op`] takes an operation and its parameters and answers with one JSON
//! object. The MCP server ([`mcp`]), the CLI ([`cli`]) and the local server ([`serve`]) are thin wrappers over it, so
//! the three of them always expose the same operations and the same answers.
//!
//! The answers are small on purpose: they travel into the context of a model, so every byte counts.
//!
//! The errors are the codes of the bridge (`pime_core::agent_proto::Error`: `NO_IME`, `NO_CONTEXT`, `REFUSED`,
//! `SECURE_FIELD`, `BUSY`, `TOO_LARGE`, `MALFORMED`, `UNSUPPORTED`) plus `CORE_UNAVAILABLE` for the core engine.
#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

pub mod bridge;
pub mod cli;
pub mod core;
pub mod mcp;
pub mod serve;

use pime_core::agent_proto::{Error, Op as BridgeOp, Reply, Request, SourceIdentity};
use pime_core::json::Json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// The version of the agent API.
pub const VERSION: &str = "0.1.0";

/// The error code of the core engine: the codes of the bridge do not cover it.
pub const CORE_UNAVAILABLE: &str = "CORE_UNAVAILABLE";

/// The languages the input method can be switched to.
pub const LANGUAGES: [&str; 3] = ["zh-CN", "en-US", "ja-JP"];

/// The operations of the agent API. Every front-end exposes exactly these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Is the input method running in the focused window, which language, is the core there.
    Status,
    /// Write text at the caret of the focused document.
    Insert,
    /// Read the text that is selected in the focused document.
    Read,
    /// Switch the language of the input method.
    Language,
    /// The candidates of the built-in dictionary (pinyin to Chinese, romaji to kana).
    Candidates,
    /// The code candidates of the core engine.
    Predict,
    /// Validate a snippet with the core.
    Validate,
    /// Store the words an AI proposes in the word bank of the user.
    Curate,
    /// The words the user typed and that had no candidate.
    Misses,
    /// The language of a file.
    Resolve,
    /// The secret scan of a text.
    Scan,
}

impl Op {
    /// The name of the operation.
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Status => "status",
            Op::Insert => "insert",
            Op::Read => "read",
            Op::Language => "language",
            Op::Candidates => "candidates",
            Op::Predict => "predict",
            Op::Validate => "validate",
            Op::Curate => "curate",
            Op::Misses => "misses",
            Op::Resolve => "resolve",
            Op::Scan => "scan",
        }
    }

    /// The operation of a name; an unknown name is refused.
    pub fn parse(name: &str) -> Option<Op> {
        match name {
            "status" => Some(Op::Status),
            "insert" => Some(Op::Insert),
            "read" => Some(Op::Read),
            "language" => Some(Op::Language),
            "candidates" => Some(Op::Candidates),
            "predict" => Some(Op::Predict),
            "validate" => Some(Op::Validate),
            "curate" => Some(Op::Curate),
            "misses" => Some(Op::Misses),
            "resolve" => Some(Op::Resolve),
            "scan" => Some(Op::Scan),
            _ => None,
        }
    }

    /// Every operation, in the order the documentation lists them.
    pub fn all() -> [Op; 11] {
        [
            Op::Status,
            Op::Insert,
            Op::Read,
            Op::Language,
            Op::Candidates,
            Op::Predict,
            Op::Validate,
            Op::Curate,
            Op::Misses,
            Op::Resolve,
            Op::Scan,
        ]
    }
}

impl std::str::FromStr for Op {
    type Err = &'static str;

    fn from_str(name: &str) -> Result<Op, &'static str> {
        Op::parse(name).ok_or("there is no operation with that name")
    }
}

/// The environment variable that names the provider of the client ("openai", "anthropic", ...).
pub const PROVIDER_ENV: &str = "PIME_AGENT_PROVIDER";

/// The environment variable that names the agent of the client ("claude-code", "cursor", ...).
pub const NAME_ENV: &str = "PIME_AGENT_NAME";

/// The environment variable that names the instance of the client.
///
/// The instance is what tells two clients of the same agent apart, so that the input method can give each of them a
/// mouth of light of its own.
pub const INSTANCE_ENV: &str = "PIME_AGENT_INSTANCE";

/// The identity of a client: who speaks, and which instance of it.
///
/// Two sub-agents of the **same** provider are two different sources: the identity carries the name of the agent and
/// the instance of the client, so the key of the source tells them apart. The input method draws a mouth of light
/// for every source and fires the beam of a write in the colour of that mouth.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// The provider of the client ("openai", "anthropic", ...).
    pub provider: String,
    /// The name of the agent ("claude-code", "cursor", ...).
    pub agent: String,
    /// The instance of the client: a value that is unique per client process.
    pub instance: String,
}

impl Identity {
    /// The identity of this process: the environment first, then the defaults.
    pub fn detect() -> Identity {
        Identity::default().resolve()
    }

    /// The identity with the empty parts filled from the environment and then from the defaults.
    ///
    /// What is given wins over the environment (the command line names the client), and the environment wins over
    /// the defaults (the user of the machine names the client).
    pub fn resolve(&self) -> Identity {
        Identity {
            provider: first_of(&self.provider, env(PROVIDER_ENV), default_provider()),
            agent: first_of(&self.agent, env(NAME_ENV), default_agent()),
            instance: first_of(&self.instance, env(INSTANCE_ENV), instance_id()),
        }
    }

    /// The source of the requests of this client.
    pub fn source(&self) -> SourceIdentity {
        SourceIdentity::agent(&self.provider, &self.agent, &self.instance)
    }

    /// The source of the requests, with the name of the agent replaced.
    ///
    /// The MCP tool may name the sub-agent that writes (the parameter `agent`): two sub-agents of the same provider
    /// in the same process are then two different sources, and therefore two different mouths of light.
    pub fn source_with_agent(&self, agent: Option<&str>) -> SourceIdentity {
        let mut source = self.source();
        if let Some(agent) = agent.filter(|name| !name.is_empty()) {
            source.agent = agent.to_string();
        }
        source
    }
}

/// The value of an environment variable, when it is set and not empty.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The first of the values that is not empty.
fn first_of(chosen: &str, from_env: Option<String>, default: String) -> String {
    if !chosen.is_empty() {
        return chosen.to_string();
    }
    from_env.unwrap_or(default)
}

/// The provider of a client that does not say one: the product itself.
fn default_provider() -> String {
    "pime".to_string()
}

/// The name of the agent of a client that does not say one: the name of the program.
fn default_agent() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "pime-agent".to_string())
}

/// The instance of this client process: sixteen hex digits.
///
/// It is **not a secret**: it only tells the clients apart, so that the input method gives each of them a mouth of
/// light of its own. Two processes never have the same one (the clock, the process and a counter are in it).
pub fn instance_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    instance_id_of(now, std::process::id(), count)
}

/// The instance of a client, out of the three numbers that make it.
pub fn instance_id_of(nanos: u128, pid: u32, count: u64) -> String {
    let digest = pime_core::sha256::sha256(format!("{nanos:x}:{pid:x}:{count:x}").as_bytes());
    pime_core::sha256::to_hex(&digest[..8])
}

/// Why an operation did not do what it was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    /// The bridge refused the request, or could not be reached. The code is one of the codes of the bridge.
    Protocol {
        /// The code the agents see.
        code: Error,
        /// One line for the user.
        message: String,
    },
    /// The core engine is not there, or it did not answer.
    CoreUnavailable {
        /// One line for the user.
        message: String,
    },
}

impl AgentError {
    /// A failure of the bridge.
    pub fn protocol(code: Error, message: impl Into<String>) -> AgentError {
        AgentError::Protocol {
            code,
            message: message.into(),
        }
    }

    /// A failure of the core engine.
    pub fn core(message: impl Into<String>) -> AgentError {
        AgentError::CoreUnavailable {
            message: message.into(),
        }
    }

    /// The code of the failure.
    pub fn code(&self) -> &'static str {
        match self {
            AgentError::Protocol { code, .. } => code.as_str(),
            AgentError::CoreUnavailable { .. } => CORE_UNAVAILABLE,
        }
    }

    /// The line for the user.
    pub fn message(&self) -> &str {
        match self {
            AgentError::Protocol { message, .. } => message,
            AgentError::CoreUnavailable { message } => message,
        }
    }

    /// Whether trying again can change the answer.
    pub fn is_retryable(&self) -> bool {
        matches!(self, AgentError::Protocol { code: Error::Busy, .. })
    }

    /// The answer of the failure: `{"ok":false,"error":"CODE","message":"..."}`.
    pub fn to_answer(&self) -> Json {
        Json::obj(vec![
            ("ok", Json::Bool(false)),
            ("error", Json::str(self.code())),
            ("message", Json::str(self.message())),
        ])
    }
}

/// The bridge to the input method: the text service of the focused process.
pub trait ImeBridge: Send + Sync {
    /// Sends one request to the text service and returns its answer.
    fn send(&self, request: &Request) -> Result<Reply, AgentError>;
}

/// The core engine: the part of the input method that predicts, validates and resolves.
pub trait CoreEngine: Send + Sync {
    /// Sends one request to the core and returns its answer (the whole JSON object it answered with).
    fn call(&self, request: &Json) -> Result<Json, AgentError>;
}

/// The two backends of the agent API.
#[derive(Clone)]
pub struct Backend {
    bridge: Arc<dyn ImeBridge>,
    core: Arc<dyn CoreEngine>,
    /// Who this client is (the provider, the agent and the instance of it).
    identity: Identity,
    /// The name the MCP client gave in `initialize`: it is the name of the agent that writes.
    client_name: Arc<Mutex<Option<String>>>,
}

impl Backend {
    /// The backends of this machine: the bridge of the focused process and `pime-core.exe`.
    pub fn system() -> Backend {
        Backend::system_with(Identity::detect())
    }

    /// The backends of this machine, with the identity of the client.
    pub fn system_with(identity: Identity) -> Backend {
        Backend {
            bridge: Arc::new(bridge::BridgeClient::new()),
            core: Arc::new(core::CoreClient::new()),
            identity: identity.resolve(),
            client_name: Arc::new(Mutex::new(None)),
        }
    }

    /// The backends of a test (or of a host that has its own).
    pub fn new(bridge: Arc<dyn ImeBridge>, core: Arc<dyn CoreEngine>) -> Backend {
        Backend {
            bridge,
            core,
            identity: Identity::detect(),
            client_name: Arc::new(Mutex::new(None)),
        }
    }

    /// The same backends with another identity (the command line names the client).
    pub fn with_identity(&self, identity: Identity) -> Backend {
        Backend {
            bridge: Arc::clone(&self.bridge),
            core: Arc::clone(&self.core),
            identity: identity.resolve(),
            client_name: Arc::clone(&self.client_name),
        }
    }

    /// The identity of this client.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// The name the MCP client gave in `initialize` (it is the name of the agent that writes).
    pub fn set_client_name(&self, name: &str) {
        if let Ok(mut slot) = self.client_name.lock() {
            *slot = Some(name.to_string());
        }
    }

    /// The source of the requests: who is writing.
    ///
    /// The parameter `agent` of the operation names the sub-agent that writes: it replaces the name of the agent of
    /// the process, so that two sub-agents of the same provider are two different sources (and therefore two
    /// different mouths of light on the screen of the user).
    pub fn source_for(&self, params: &Json) -> SourceIdentity {
        let named = params
            .get("agent")
            .and_then(Json::as_str)
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string());
        let named = named.or_else(|| {
            self.client_name
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .filter(|name| !name.is_empty())
        });
        self.identity.source_with_agent(named.as_deref())
    }

    /// The bridge.
    pub fn bridge(&self) -> &dyn ImeBridge {
        self.bridge.as_ref()
    }

    /// The core engine.
    pub fn core(&self) -> &dyn CoreEngine {
        self.core.as_ref()
    }
}

/// The detail of an error of the operating system, in English (the text of the system is translated).
pub fn os_detail(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("os error {code}"),
        None => error.kind().to_string(),
    }
}

/// The answer of an operation that worked: `{"ok":true,...}`.
fn ok_answer(pairs: Vec<(&str, Json)>) -> Json {
    let mut all = vec![("ok", Json::Bool(true))];
    all.extend(pairs);
    Json::obj(all)
}

/// The answer of a request object: the operation is its `op` field, the rest are the parameters.
pub fn dispatch_request(body: &Json, backend: &Backend) -> Json {
    let op = body.get("op").and_then(Json::as_str).unwrap_or("");
    dispatch(op, body, backend)
}

/// The answer of an operation by name. An unknown name is refused.
pub fn dispatch(op: &str, params: &Json, backend: &Backend) -> Json {
    match Op::parse(op) {
        Some(op) => dispatch_op(op, params, backend),
        None => AgentError::protocol(
            Error::Malformed,
            format!("there is no operation named {op}"),
        )
        .to_answer(),
    }
}

/// The answer of an operation. Every operation answers with one JSON object.
pub fn dispatch_op(op: Op, params: &Json, backend: &Backend) -> Json {
    let answer = match op {
        Op::Status => status(params, backend),
        Op::Insert => insert(params, backend),
        Op::Read => read(params, backend),
        Op::Language => language(params, backend),
        Op::Candidates => candidates(params),
        Op::Predict => predict(params, backend),
        Op::Validate => validate(params, backend),
        Op::Curate => curate(params),
        Op::Misses => misses(),
        Op::Resolve => resolve(params, backend),
        Op::Scan => scan(params, backend),
    };
    match answer {
        Ok(answer) => answer,
        Err(error) => error.to_answer(),
    }
}

/// `status`: is the input method running in the focused window, which language, is the core there.
fn status(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let ime = match backend
        .bridge()
        .send(&Request::with_source(BridgeOp::Status, backend.source_for(params)))
    {
        Ok(reply) if reply.ok => Json::obj(vec![
            ("available", Json::Bool(true)),
            (
                "language",
                reply.language.as_deref().map_or(Json::Null, Json::str),
            ),
            (
                "message",
                Json::str(if reply.message.is_empty() {
                    "the input method is running in the focused window"
                } else {
                    &reply.message
                }),
            ),
        ]),
        Ok(reply) => Json::obj(vec![
            ("available", Json::Bool(false)),
            (
                "error",
                Json::str(reply.error.unwrap_or(Error::Refused).as_str()),
            ),
            ("message", Json::str(&reply.message)),
        ]),
        Err(error) => Json::obj(vec![
            ("available", Json::Bool(false)),
            ("error", Json::str(error.code())),
            ("message", Json::str(error.message())),
        ]),
    };
    let core = match backend.core().call(&core_request("health", Vec::new())) {
        Ok(answer) => Json::obj(vec![
            ("available", Json::Bool(true)),
            ("name", answer.get("name").cloned().unwrap_or(Json::Null)),
            (
                "version",
                answer.get("version").cloned().unwrap_or(Json::Null),
            ),
            (
                "protocol",
                answer.get("protocol").cloned().unwrap_or(Json::Null),
            ),
        ]),
        Err(error) => Json::obj(vec![
            ("available", Json::Bool(false)),
            ("error", Json::str(error.code())),
            ("message", Json::str(error.message())),
        ]),
    };
    Ok(ok_answer(vec![("ime", ime), ("core", core)]))
}

/// `insert`: write the text at the caret of the focused document.
fn insert(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let text = required_text(params, "text")?;
    let language = optional_text(params, "language")?;
    let replace = optional_bool(params, "replace")?;
    let mut request = Request::with_source(BridgeOp::Insert, backend.source_for(params));
    request.text = text;
    request.language = language.clone();
    request.replace = replace;
    let reply = backend.bridge().send(&request)?;
    let language = reply.language.clone().or(language);
    Ok(bridge_answer(
        &reply,
        vec![
            ("inserted", Json::int(reply.inserted as i64)),
            (
                "language",
                language.as_deref().map_or(Json::Null, Json::str),
            ),
        ],
    ))
}

/// `read`: the text that is selected in the focused document.
fn read(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let reply = backend
        .bridge()
        .send(&Request::with_source(BridgeOp::Read, backend.source_for(params)))?;
    Ok(bridge_answer(
        &reply,
        vec![("text", Json::str(&reply.text))],
    ))
}

/// `language`: switch the language of the input method.
fn language(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let language = required_text(params, "language")?;
    if !LANGUAGES.contains(&language.as_str()) {
        return Err(AgentError::protocol(
            Error::Unsupported,
            format!(
                "{language} is not a language of the input method ({} are)",
                LANGUAGES.join(", ")
            ),
        ));
    }
    let mut request = Request::with_source(BridgeOp::Language, backend.source_for(params));
    request.language = Some(language.clone());
    let reply = backend.bridge().send(&request)?;
    let language = reply.language.clone().unwrap_or(language);
    Ok(bridge_answer(
        &reply,
        vec![("language", Json::str(&language))],
    ))
}

/// `candidates`: the candidates of the built-in dictionary.
///
/// The dictionaries are the ones of the input method itself (the tables of `pime_core::cjk`), so the answer needs no
/// process and no file.
fn candidates(params: &Json) -> Result<Json, AgentError> {
    let text = required_text(params, "text")?;
    let language = optional_text(params, "language")?.unwrap_or_else(|| "zh-CN".to_string());
    let list = match language.as_str() {
        "ja-JP" => pime_core::cjk::japanese_candidates(&text),
        "en-US" => pime_core::cjk::english_candidates(&text),
        "zh-CN" => pime_core::cjk::pinyin_candidates(&text),
        other => {
            return Err(AgentError::protocol(
                Error::Unsupported,
                format!("{other} has no dictionary in the input method"),
            ));
        }
    };
    Ok(ok_answer(vec![(
        "candidates",
        Json::Arr(list.iter().map(|c| Json::str(c)).collect()),
    )]))
}

/// `predict`: the code candidates of the core engine.
fn predict(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let text = required_text(params, "text")?;
    let mut pairs = vec![("prefix", Json::str(&text))];
    if let Some(language) = optional_text(params, "language")? {
        pairs.push(("language", Json::str(&language)));
    }
    if let Some(limit) = optional_int(params, "limit")? {
        pairs.push(("limit", Json::int(limit)));
    }
    let answer = backend.core().call(&core_request("predict", pairs))?;
    core_answer(&answer, "candidates")
}

/// `validate`: the validation of a snippet by the core.
fn validate(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let text = required_text(params, "text")?;
    let language = required_text(params, "language")?;
    let mut pairs = vec![
        ("text", Json::str(&text)),
        ("language", Json::str(&language)),
    ];
    if let Some(dialect) = optional_object(params, "dialect")? {
        pairs.push(("dialect", dialect));
    }
    let answer = backend.core().call(&core_request("validate", pairs))?;
    core_answer(&answer, "result")
}

/// `curate`: store the words an AI proposes in the word bank of the user.
///
/// The word bank belongs to the text service (the DLL side owns the storage, it is written in parallel with this
/// crate). This build has no backend for it, so the operation is refused instead of pretending to work.
fn curate(params: &Json) -> Result<Json, AgentError> {
    let items = optional_array(params, "proposals")?.ok_or_else(|| {
        AgentError::protocol(Error::Malformed, "the parameter proposals is required")
    })?;
    // the shape of the proposals is checked with the parser of the bridge, so a malformed request is refused as
    // malformed and not as an unsupported operation
    pime_core::agent_proto::proposals_from_json(&Json::Arr(items)).map_err(|e| {
        AgentError::protocol(e, "the proposals are not the ones of the bridge")
    })?;
    Err(AgentError::protocol(
        Error::Unsupported,
        "the word bank of the user is not in this build: the storage is written by the text service",
    ))
}

/// `misses`: the words the user typed and that had no candidate.
///
/// The list belongs to the text service (the DLL side owns the storage, it is written in parallel with this crate).
/// This build has no backend for it, so the operation is refused instead of pretending to work.
fn misses() -> Result<Json, AgentError> {
    Err(AgentError::protocol(
        Error::Unsupported,
        "the list of the words without candidate is not in this build: the storage is written by the text service",
    ))
}

/// `resolve`: the language of a file.
fn resolve(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let path = required_text(params, "path")?;
    let answer = backend
        .core()
        .call(&core_request("resolve", vec![("fileName", Json::str(&path))]))?;
    core_answer(&answer, "decision")
}

/// `scan`: the secret scan of a text.
fn scan(params: &Json, backend: &Backend) -> Result<Json, AgentError> {
    let text = required_text(params, "text")?;
    let answer = backend
        .core()
        .call(&core_request("scan", vec![("text", Json::str(&text))]))?;
    core_answer(&answer, "result")
}

/// The answer of the bridge: the shape of a successful operation, or the failure of the text service.
fn bridge_answer(reply: &Reply, pairs: Vec<(&str, Json)>) -> Json {
    if reply.ok {
        ok_answer(pairs)
    } else {
        let code = reply.error.unwrap_or(Error::Refused);
        AgentError::protocol(
            code,
            if reply.message.is_empty() {
                "the input method refused the request".to_string()
            } else {
                reply.message.clone()
            },
        )
        .to_answer()
    }
}

/// The answer of the core: the field of the answer of the core, or the failure of the core.
fn core_answer(answer: &Json, key: &str) -> Result<Json, AgentError> {
    if answer.get("ok").and_then(Json::as_bool) != Some(true) {
        let code = answer.get("error").and_then(Json::as_str).unwrap_or("INVALID_REQUEST");
        let field = answer.get("field").and_then(Json::as_str).unwrap_or("");
        let message = if field.is_empty() {
            format!("the core refused the request: {code}")
        } else {
            format!("the core refused the request: {code} ({field})")
        };
        return Err(AgentError::protocol(Error::Malformed, message));
    }
    match answer.get(key) {
        Some(value) => Ok(ok_answer(vec![(key, value.clone())])),
        None => Ok(ok_answer(Vec::new())),
    }
}

/// The request of the core: the operation and its fields.
fn core_request(op: &str, pairs: Vec<(&str, Json)>) -> Json {
    let mut all = vec![("op", Json::str(op))];
    all.extend(pairs);
    Json::obj(all)
}

/// The text of a parameter.
fn optional_text(params: &Json, key: &str) -> Result<Option<String>, AgentError> {
    match params.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(text)) => Ok(Some(text.clone())),
        Some(_) => Err(AgentError::protocol(
            Error::Malformed,
            format!("the parameter {key} must be a string"),
        )),
    }
}

/// The text of a parameter that has to be there.
fn required_text(params: &Json, key: &str) -> Result<String, AgentError> {
    match optional_text(params, key)? {
        Some(text) if !text.is_empty() => Ok(text),
        _ => Err(AgentError::protocol(
            Error::Malformed,
            format!("the parameter {key} is required"),
        )),
    }
}

/// The flag of a parameter.
fn optional_bool(params: &Json, key: &str) -> Result<bool, AgentError> {
    match params.get(key) {
        None | Some(Json::Null) => Ok(false),
        Some(Json::Bool(value)) => Ok(*value),
        Some(_) => Err(AgentError::protocol(
            Error::Malformed,
            format!("the parameter {key} must be true or false"),
        )),
    }
}

/// The number of a parameter.
fn optional_int(params: &Json, key: &str) -> Result<Option<i64>, AgentError> {
    match params.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Num(value)) => match Json::Num(*value).as_i64() {
            Some(number) => Ok(Some(number)),
            None => Err(AgentError::protocol(
                Error::Malformed,
                format!("the parameter {key} must be a whole number"),
            )),
        },
        Some(_) => Err(AgentError::protocol(
            Error::Malformed,
            format!("the parameter {key} must be a number"),
        )),
    }
}

/// The object of a parameter.
fn optional_object(params: &Json, key: &str) -> Result<Option<Json>, AgentError> {
    match params.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(value @ Json::Obj(_)) => Ok(Some(value.clone())),
        Some(_) => Err(AgentError::protocol(
            Error::Malformed,
            format!("the parameter {key} must be an object"),
        )),
    }
}

/// The array of a parameter.
fn optional_array(params: &Json, key: &str) -> Result<Option<Vec<Json>>, AgentError> {
    match params.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Arr(items)) => Ok(Some(items.clone())),
        Some(_) => Err(AgentError::protocol(
            Error::Malformed,
            format!("the parameter {key} must be an array"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json;

    /// A bridge that keeps the request it was given, so that the test can see who is writing.
    struct RecordingBridge {
        reply: Reply,
        seen: Arc<Mutex<Option<Request>>>,
    }

    impl ImeBridge for RecordingBridge {
        fn send(&self, request: &Request) -> Result<Reply, AgentError> {
            if let Ok(mut slot) = self.seen.lock() {
                *slot = Some(request.clone());
            }
            Ok(self.reply.clone())
        }
    }

    /// A backend that keeps the request it was given.
    fn recording_backend(seen: &Arc<Mutex<Option<Request>>>) -> Backend {
        Backend::new(
            Arc::new(RecordingBridge {
                reply: Reply::ok(),
                seen: Arc::clone(seen),
            }),
            Arc::new(FakeCore {
                answer: Json::Null,
            }),
        )
        .with_identity(Identity {
            provider: "anthropic".to_string(),
            agent: "claude-code".to_string(),
            instance: "0123456789abcdef".to_string(),
        })
    }

    /// A bridge that answers what the test says.
    struct FakeBridge {
        reply: Reply,
    }

    impl ImeBridge for FakeBridge {
        fn send(&self, _request: &Request) -> Result<Reply, AgentError> {
            Ok(self.reply.clone())
        }
    }

    /// A core that answers what the test says.
    struct FakeCore {
        answer: Json,
    }

    impl CoreEngine for FakeCore {
        fn call(&self, _request: &Json) -> Result<Json, AgentError> {
            Ok(self.answer.clone())
        }
    }

    fn params(text: &str) -> Json {
        json::parse(text, json::Limits::DEFAULT).expect("the parameters are JSON")
    }

    fn backend(reply: Reply, core: Json) -> Backend {
        Backend::new(
            Arc::new(FakeBridge { reply }),
            Arc::new(FakeCore { answer: core }),
        )
    }

    /// The instance of a client is sixteen hex digits, and two clients of the same machine never have the same one.
    ///
    /// It is not a secret: it only tells the clients apart, so that the input method can give each of them a mouth of
    /// light of its own.
    #[test]
    fn the_instance_of_a_client_is_sixteen_hex_digits_and_two_clients_differ() {
        let first = instance_id();
        let second = instance_id();
        assert_eq!(first.len(), 16, "the instance is sixteen hex digits: {first}");
        assert!(
            first.chars().all(|digit| digit.is_ascii_hexdigit()),
            "the instance is hex: {first}"
        );
        assert_ne!(
            first, second,
            "two clients of the same machine must not have the same instance"
        );
        // two processes: the number of the process is in the instance
        assert_ne!(
            instance_id_of(1_700_000_000_000_000_000, 1234, 0),
            instance_id_of(1_700_000_000_000_000_000, 5678, 0),
            "two processes are two clients"
        );
        // and the same numbers give the same instance
        assert_eq!(
            instance_id_of(1_700_000_000_000_000_000, 1234, 0),
            instance_id_of(1_700_000_000_000_000_000, 1234, 0)
        );
    }

    /// Two clients of the same provider are two different sources: the key of the source tells them apart.
    #[test]
    fn two_clients_of_the_same_provider_are_two_different_sources() {
        let one = Identity {
            provider: "anthropic".to_string(),
            agent: "claude-code".to_string(),
            instance: "aaaaaaaa".to_string(),
        };
        let other = Identity {
            provider: "anthropic".to_string(),
            agent: "claude-code".to_string(),
            instance: "bbbbbbbb".to_string(),
        };
        assert_eq!(one.source().key(), "agent:anthropic:claude-code:aaaaaaaa");
        assert_ne!(
            one.source().key(),
            other.source().key(),
            "two instances of the same agent are two different speakers"
        );
    }

    /// The name of the sub-agent replaces the name of the agent of the process: two sub-agents of the same provider
    /// in the same process are two different sources, and therefore two different mouths of light.
    #[test]
    fn the_name_of_the_sub_agent_makes_a_second_source() {
        let identity = Identity {
            provider: "anthropic".to_string(),
            agent: "claude-code".to_string(),
            instance: "aaaaaaaa".to_string(),
        };
        let planner = identity.source_with_agent(Some("planner"));
        let coder = identity.source_with_agent(Some("coder"));
        assert_eq!(planner.agent, "planner");
        assert_eq!(coder.agent, "coder");
        assert_ne!(
            planner.key(),
            coder.key(),
            "two sub-agents of the same provider are two different speakers"
        );
        // without a name the agent of the process speaks
        assert_eq!(identity.source_with_agent(None).key(), identity.source().key());
    }

    /// Every request says who is writing: the input method fires the beam of that source in its own colour.
    #[test]
    fn every_request_carries_the_source_of_the_client() {
        let seen = Arc::new(Mutex::new(None));
        let backend = recording_backend(&seen);
        dispatch_op(Op::Insert, &params(r#"{"text":"hello"}"#), &backend);
        let request = seen
            .lock()
            .expect("the lock")
            .clone()
            .expect("the bridge was asked");
        assert_eq!(
            request.source.key(),
            "agent:anthropic:claude-code:0123456789abcdef"
        );
        assert_eq!(request.source.provider, "anthropic");
        assert!(!request.source.instance.is_empty(), "the client has an instance");
    }

    /// The parameter `agent` names the sub-agent that writes: it replaces the name of the agent of the process.
    #[test]
    fn the_parameter_agent_names_the_sub_agent_that_writes() {
        let seen = Arc::new(Mutex::new(None));
        let backend = recording_backend(&seen);
        dispatch_op(
            Op::Insert,
            &params(r#"{"text":"hello","agent":"planner"}"#),
            &backend,
        );
        let request = seen
            .lock()
            .expect("the lock")
            .clone()
            .expect("the bridge was asked");
        assert_eq!(request.source.key(), "agent:anthropic:planner:0123456789abcdef");
    }

    #[test]
    fn every_operation_has_a_name_and_comes_back_from_it() {
        for op in Op::all() {
            assert_eq!(Op::parse(op.as_str()), Some(op), "{op:?}");
            assert_eq!(op.as_str().parse::<Op>(), Ok(op), "{op:?}");
        }
        assert_eq!(Op::parse("nonsense"), None);
        assert!("nonsense".parse::<Op>().is_err());
        assert_eq!(Op::all().len(), 11);
    }

    #[test]
    fn insert_answers_what_the_text_service_wrote() {
        let mut reply = Reply::ok();
        reply.inserted = 5;
        reply.language = Some("zh-CN".to_string());
        let backend = backend(reply, Json::Null);
        let answer = dispatch_op(
            Op::Insert,
            &params(r#"{"text":"hello"}"#),
            &backend,
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(answer.get("inserted").and_then(Json::as_i64), Some(5));
        assert_eq!(answer.get("language").and_then(Json::as_str), Some("zh-CN"));
    }

    #[test]
    fn a_refused_insert_is_an_error_answer_with_the_code_of_the_bridge() {
        let reply = Reply::failed(Error::Secure, "the field is a password field");
        let backend = backend(reply, Json::Null);
        let answer = dispatch_op(Op::Insert, &params(r#"{"text":"hello"}"#), &backend);
        assert_eq!(answer.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(answer.get("error").and_then(Json::as_str), Some("SECURE_FIELD"));
        assert_eq!(
            answer.get("message").and_then(Json::as_str),
            Some("the field is a password field")
        );
    }

    #[test]
    fn a_missing_parameter_is_refused_as_malformed() {
        let backend = backend(Reply::ok(), Json::Null);
        let answer = dispatch_op(Op::Insert, &params("{}"), &backend);
        assert_eq!(answer.get("error").and_then(Json::as_str), Some("MALFORMED"));
        let answer = dispatch_op(Op::Validate, &params(r#"{"text":"x"}"#), &backend);
        assert_eq!(answer.get("error").and_then(Json::as_str), Some("MALFORMED"));
    }

    #[test]
    fn an_unknown_operation_is_refused() {
        let backend = backend(Reply::ok(), Json::Null);
        let answer = dispatch("nonsense", &params("{}"), &backend);
        assert_eq!(answer.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(answer.get("error").and_then(Json::as_str), Some("MALFORMED"));
    }

    #[test]
    fn the_dictionary_answers_the_candidates_of_the_input_method() {
        let answer = dispatch_op(Op::Candidates, &params(r#"{"text":"ni"}"#), &backend(Reply::ok(), Json::Null));
        assert_eq!(answer.get("ok"), Some(&Json::Bool(true)));
        let list = answer.get("candidates").and_then(Json::as_array).expect("candidates");
        assert!(list.iter().any(|c| c.as_str() == Some("你")), "{:?}", list);
    }

    #[test]
    fn curate_and_misses_are_refused_instead_of_pretending_to_work() {
        let backend = backend(Reply::ok(), Json::Null);
        let answer = dispatch_op(
            Op::Curate,
            &params(r#"{"proposals":[{"word":"x","reading":"y","reason":"z"}]}"#),
            &backend,
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(
            answer.get("error").and_then(Json::as_str),
            Some("UNSUPPORTED")
        );
        assert!(answer
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .contains("text service"));
        let answer = dispatch_op(Op::Misses, &params("{}"), &backend);
        assert_eq!(
            answer.get("error").and_then(Json::as_str),
            Some("UNSUPPORTED")
        );
    }

    #[test]
    fn the_status_reports_both_backends() {
        let mut reply = Reply::ok();
        reply.language = Some("ja-JP".to_string());
        let core = params(r#"{"ok":true,"name":"pime-core","version":"0.2.0","protocol":"0.3"}"#);
        let answer = dispatch_op(Op::Status, &params("{}"), &backend(reply, core));
        let ime = answer.get("ime").expect("the state of the input method");
        assert_eq!(ime.get("available"), Some(&Json::Bool(true)));
        assert_eq!(ime.get("language").and_then(Json::as_str), Some("ja-JP"));
        let core = answer.get("core").expect("the state of the core");
        assert_eq!(core.get("available"), Some(&Json::Bool(true)));
        assert_eq!(core.get("version").and_then(Json::as_str), Some("0.2.0"));
    }

    #[test]
    fn a_core_that_is_not_there_is_reported_as_core_unavailable() {
        struct DeadCore;
        impl CoreEngine for DeadCore {
            fn call(&self, _request: &Json) -> Result<Json, AgentError> {
                Err(AgentError::core("the core is not there"))
            }
        }
        let backend = Backend::new(
            Arc::new(FakeBridge { reply: Reply::ok() }),
            Arc::new(DeadCore),
        );
        let answer = dispatch_op(Op::Status, &params("{}"), &backend);
        let core = answer.get("core").expect("the state of the core");
        assert_eq!(core.get("available"), Some(&Json::Bool(false)));
        assert_eq!(
            core.get("error").and_then(Json::as_str),
            Some(CORE_UNAVAILABLE)
        );
    }

    #[test]
    fn a_request_object_carries_the_operation() {
        let backend = backend(Reply::ok(), Json::Null);
        let answer = dispatch_request(&params(r#"{"op":"read"}"#), &backend);
        assert_eq!(answer.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(answer.get("text").and_then(Json::as_str), Some(""));
    }
}

/// The backends of the tests: no desktop and no core process.
///
/// The front-ends (the MCP server, the CLI, the local server) are tested against these, so their tests do not need
/// the text service of a running application and do not start a process.
#[cfg(test)]
pub mod testsupport {
    use super::{AgentError, Backend, CoreEngine, ImeBridge};
    use pime_core::agent_proto::{Reply, Request};
    use pime_core::json::Json;
    use std::sync::{Arc, Mutex};

    /// A bridge that always answers the same thing, and that keeps the request it was given.
    pub struct FakeBridge {
        /// The answer of the bridge.
        pub reply: Reply,
        /// The request the bridge was given last.
        pub seen: Arc<Mutex<Option<Request>>>,
    }

    impl ImeBridge for FakeBridge {
        fn send(&self, request: &Request) -> Result<Reply, AgentError> {
            if let Ok(mut slot) = self.seen.lock() {
                *slot = Some(request.clone());
            }
            Ok(self.reply.clone())
        }
    }

    /// A core that always answers the same thing.
    pub struct FakeCore {
        /// The answer of the core.
        pub answer: Json,
    }

    impl CoreEngine for FakeCore {
        fn call(&self, _request: &Json) -> Result<Json, AgentError> {
            Ok(self.answer.clone())
        }
    }

    /// The backend of the tests.
    pub struct FakeBackend {
        inner: Backend,
        /// The request the bridge was given last.
        seen: Arc<Mutex<Option<Request>>>,
    }

    impl Default for FakeBackend {
        fn default() -> Self {
            FakeBackend::new()
        }
    }

    impl FakeBackend {
        /// A backend that answers like a text service that works and like a core that is there.
        pub fn new() -> FakeBackend {
            let mut reply = Reply::ok();
            reply.inserted = 3;
            reply.language = Some("zh-CN".to_string());
            reply.text = "selected".to_string();
            let core = Json::obj(vec![
                ("ok", Json::Bool(true)),
                ("name", Json::str("pime-core")),
                ("version", Json::str("0.2.0")),
                ("protocol", Json::str("0.3")),
            ]);
            let seen = Arc::new(Mutex::new(None));
            FakeBackend {
                inner: Backend::new(
                    Arc::new(FakeBridge {
                        reply,
                        seen: Arc::clone(&seen),
                    }),
                    Arc::new(FakeCore { answer: core }),
                ),
                seen,
            }
        }

        /// The request the bridge was given last (the tests look at the source of it).
        pub fn last_request(&self) -> Option<Request> {
            self.seen.lock().ok().and_then(|slot| slot.clone())
        }

        /// The backend itself.
        pub fn backend(&self) -> &Backend {
            &self.inner
        }
    }
}
