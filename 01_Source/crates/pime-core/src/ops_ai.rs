//! Protocol 0.3 operations of migration steps 1.2 and 1.3: the AI request logic (`ai.*`), the outbound privacy gate
//! (`privacy.*`), annotations (`annotate.*`), the symbol index (`symbols.*`) and input scheduling (`sched.*`,
//! `lang.*`). They follow the conventions of `ops.rs`: every operation names an open `session`; new top-level request
//! fields are snake_case; nested objects that mirror the bridge's own shapes keep their camelCase names.
//!
//! There is no HTTP in the core. `ai.prepare` runs everything that has to happen before bytes leave the machine (in the
//! order of `AiClient.infer`) and returns the request to send; the bridge sends it and reports the outcome with
//! `ai.complete`. An AI failure is an ordinary error response (`ok:false`) that additionally carries the metadata of
//! the reference `PimeError`: `retryable`, `user_action`, and where they exist `retry_after_sec` and `detail`, plus the
//! current `budget`.

use crate::ai::{self, AiError, BuildParams, Family};
use crate::annotate::{self, PromptArgs};
use crate::budget::{Snapshot as BudgetSnapshot, Ticket, Usage};
use crate::grants::{self, Criteria};
use crate::json::{self, Json, Limits};
use crate::ops::{OpError, OpResult, need_str};
use crate::protocol::dialect_of;
use crate::resolve::ResolveContext;
use crate::scheduler::{Effects, Key, LockScope, Stats, Verdict};
use crate::secrets::{self, Redacted};
use crate::session::Session;
use crate::symbols::{self, DirEntry};
use crate::util::{js_trim, utf16_len};

/// Operations of steps 1.2 / 1.3 (the older 0.3 operations are in `ops::OPS`).
pub const OPS: &[&str] = &[
    "ai.prompt",
    "ai.prepare",
    "ai.complete",
    "ai.parse",
    "privacy.assess",
    "privacy.redact",
    "privacy.restore",
    "privacy.release",
    "annotate.supports",
    "annotate.prompt",
    "annotate.f1_addendum",
    "annotate.apply",
    "symbols.extract",
    "symbols.dir_candidates",
    "sched.configure",
    "sched.schedule",
    "sched.fire",
    "sched.finish",
    "sched.cancel",
    "sched.touch",
    "sched.is_current",
    "lang.state",
    "lang.lock",
    "lang.unlock",
];

/// Operations that carry document text, a prompt or a provider reply and may use a long request line.
pub const BULK_OPS: &[&str] = &[
    "ai.prompt",
    "ai.prepare",
    "ai.complete",
    "ai.parse",
    "privacy.assess",
    "privacy.redact",
    "privacy.restore",
    "annotate.prompt",
    "annotate.apply",
    "symbols.extract",
    "lang.state",
];

/// Prepared requests waiting for their outcome; the oldest is settled as failed when more are open.
pub const MAX_PENDING: usize = 64;
pub const MAX_REDACTION_SETS: usize = 16;
const MAX_REDACTIONS_PER_SET: usize = 100_000;
const MAX_DIR_ENTRIES: usize = 10_000;
const BODY_LIMITS: Limits = Limits {
    max_bytes: 3 * ai::MAX_RESPONSE_UNITS + 1024,
    max_depth: 64,
};

/// A prepared AI request: its budget reservation and what the outcome is judged against.
pub struct PendingAi {
    pub id: String,
    pub ticket: Ticket,
    pub family: Family,
    pub effort: Json,
    pub started: u64,
}

/// The `PIME_REDACTED_n` map of one request (all parts of a prompt that were redacted share one set).
pub struct RedactionSet {
    pub id: String,
    pub entries: Vec<Redacted>,
}

// ---------------------------------------------------------------- request parsing

fn opt_str<'a>(req: &'a Json, key: &str) -> Result<Option<&'a str>, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s)),
        Some(_) => Err(OpError::invalid(key)),
    }
}

/// A string the reference reads without checking its type: anything else counts as empty.
fn lenient<'a>(req: &'a Json, key: &str) -> &'a str {
    req.get(key).and_then(Json::as_str).unwrap_or("")
}

fn need_bool(req: &Json, key: &str) -> Result<bool, OpError> {
    match req.get(key) {
        Some(Json::Bool(b)) => Ok(*b),
        _ => Err(OpError::invalid(key)),
    }
}

fn opt_bool(req: &Json, key: &str, default: bool) -> Result<bool, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(OpError::invalid(key)),
    }
}

/// An optional positive integer (`0`, absent and `null` all mean "not set", like `|| default`).
fn opt_count(req: &Json, key: &str) -> Result<Option<i64>, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => match v.as_i64() {
            Some(0) => Ok(None),
            Some(n) if n > 0 => Ok(Some(n)),
            _ => Err(OpError::invalid(key)),
        },
    }
}

fn workspace_of(req: &Json) -> Result<String, OpError> {
    match (req.get("workspace"), req.get("workspace_uris")) {
        (Some(Json::Str(w)), _) => Ok(w.clone()),
        (_, Some(Json::Arr(uris))) => {
            let uris: Vec<String> = uris
                .iter()
                .map(|u| {
                    u.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| OpError::invalid("workspace_uris"))
                })
                .collect::<Result<_, _>>()?;
            Ok(grants::workspace_token(&uris))
        }
        _ => Err(OpError::invalid("workspace")),
    }
}

fn dialect(req: &Json) -> Result<crate::validators::Dialect, OpError> {
    dialect_of(req).map_err(|f| OpError::invalid(&f))
}

/// `{seq, profileEpoch, documentEpoch}`; `null` is `None` (an absent key never matches anything).
fn parse_key(v: Option<&Json>, field: &str) -> Result<Option<Key>, OpError> {
    let o = match v {
        None | Some(Json::Null) => return Ok(None),
        Some(o @ Json::Obj(_)) => o,
        Some(_) => return Err(OpError::invalid(field)),
    };
    let n = |k: &str| match o.get(k).and_then(Json::as_i64) {
        Some(n) if n >= 0 => Ok(n as u64),
        _ => Err(OpError::invalid(&format!("{field}.{k}"))),
    };
    Ok(Some(Key {
        seq: n("seq")?,
        profile_epoch: n("profileEpoch")?,
        document_epoch: n("documentEpoch")?,
    }))
}

fn need_key(req: &Json) -> Result<Key, OpError> {
    parse_key(req.get("key"), "key")?.ok_or_else(|| OpError::invalid("key"))
}

// ---------------------------------------------------------------- response building

fn budget_json(s: &BudgetSnapshot) -> Json {
    Json::obj(vec![
        ("requests", Json::int(s.requests)),
        ("tokens", Json::int(s.tokens)),
        ("estimated", Json::Bool(s.estimated)),
        ("cost", s.cost.map_or(Json::Null, Json::Num)),
    ])
}

/// An AI failure as an error response with the metadata of the reference `PimeError`.
fn ai_error(e: AiError, session: &Session) -> OpError {
    let mut extra = vec![
        ("retryable", Json::Bool(e.retryable)),
        ("user_action", Json::Str(e.user_action)),
    ];
    if let Some(s) = e.retry_after_sec {
        extra.push(("retry_after_sec", Json::Num(s)));
    }
    if let Some(d) = e.detail {
        extra.push(("detail", Json::Str(d)));
    }
    extra.push(("budget", budget_json(&session.budget.snapshot())));
    OpError {
        code: e.code.to_string(),
        message: Some(e.message),
        field: None,
        extra,
    }
}

fn stats_json(s: &Stats) -> Json {
    Json::obj(vec![
        ("scheduled", Json::int(s.scheduled as i64)),
        ("started", Json::int(s.started as i64)),
        ("delivered", Json::int(s.delivered as i64)),
        ("discarded", Json::int(s.discarded as i64)),
        ("inFlight", Json::int(s.in_flight as i64)),
        ("maxInFlight", Json::int(s.max_in_flight as i64)),
    ])
}

fn effects_json(e: &Effects) -> Json {
    Json::obj(vec![
        ("clear_timer", Json::Bool(e.clear_timer)),
        ("abort", Json::Bool(e.abort)),
        ("arm_ms", e.arm_ms.map_or(Json::Null, Json::int)),
    ])
}

fn key_json(k: &Key) -> Json {
    Json::obj(vec![
        ("seq", Json::int(k.seq as i64)),
        ("profileEpoch", Json::int(k.profile_epoch as i64)),
        ("documentEpoch", Json::int(k.document_epoch as i64)),
    ])
}

// ---------------------------------------------------------------- ai.*

fn ai_prompt(req: &Json) -> OpResult {
    let task = need_str(req, "task")?;
    let payload = req
        .get("payload")
        .ok_or_else(|| OpError::invalid("payload"))?;
    let (instructions, input) =
        ai::build_prompt(task, payload).ok_or_else(|| OpError::invalid("task"))?;
    Ok(vec![
        ("instructions", Json::Str(instructions)),
        ("input", Json::Str(input)),
    ])
}

/// Everything `AiClient.infer` does before the HTTP call, in the same order, so the first failing check is the same.
fn ai_prepare(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let enabled = need_bool(req, "enabled")?;
    let trusted = need_bool(req, "trusted")?;
    let instructions = need_str(req, "instructions")?;
    let input = need_str(req, "input")?;
    let provider_id = need_str(req, "provider_id")?;
    let scope = need_str(req, "grant_scope")?;
    let workspace = workspace_of(req)?;
    let grant_id = opt_str(req, "grant_id")?.filter(|g| !g.is_empty());
    let allow_sensitive = opt_bool(req, "allow_sensitive", false)?;
    let max_output = opt_count(req, "max_output_tokens")?;
    let timeout_ms = opt_count(req, "timeout_ms")?.unwrap_or(ai::DEFAULT_TIMEOUT_MS);

    let fail = |session: &Session, e: AiError| Err(ai_error(e, session));
    if !enabled {
        return fail(session, AiError::new("AI_DISABLED", "AI is disabled"));
    }
    if session.ai_cooldown_until > now {
        return fail(
            session,
            AiError::new("AI_RATE_LIMIT", "AI is paused after a provider rate limit").retryable(),
        );
    }
    let Some(family) = Family::from_id(lenient(req, "family")) else {
        return fail(
            session,
            AiError::new("MODEL_UNSUPPORTED", "unknown API family"),
        );
    };
    let model = lenient(req, "model");
    if model.is_empty() {
        return fail(
            session,
            AiError::new("MODEL_UNSUPPORTED", "no model configured")
                .action("Choose an exact model id"),
        );
    }
    let url = match ai::endpoint_url(lenient(req, "endpoint")) {
        Ok(u) => u,
        Err(e) => return fail(session, e.into()),
    };
    let key = lenient(req, "key");
    if key.is_empty() {
        return fail(
            session,
            AiError::new("VAULT_ERROR", "no API key stored for this provider")
                .action("Configure the API key"),
        );
    }
    // authorisation: the workspace must be trusted and a grant must cover this provider, workspace and scope
    if !trusted {
        return fail(
            session,
            AiError::new("CONSENT_REQUIRED", "workspace is not trusted"),
        );
    }
    let criteria = || Criteria {
        provider_id,
        workspace: &workspace,
        scope,
    };
    match grant_id {
        Some(gid) => {
            if let Err(e) = session.broker.require(now, gid, criteria()) {
                return fail(
                    session,
                    AiError::from(e).action("Review and re-authorise the data transfer"),
                );
            }
            session.broker.consume(gid);
        }
        None => {
            if session.broker.find(now, criteria()).is_none() {
                return fail(
                    session,
                    AiError::new(
                        "CONSENT_REQUIRED",
                        "no authorisation for this provider and scope",
                    ),
                );
            }
        }
    }
    // the last gate before bytes leave: secrets must have been redacted or explicitly accepted by the user
    let outbound = format!("{instructions}\n{input}");
    if !allow_sensitive && secrets::scan(&outbound, "").blocked {
        return fail(
            session,
            AiError::new(
                "SENSITIVE_BLOCKED",
                "payload still contains high-risk secrets; nothing was sent",
            ),
        );
    }
    let estimate = ai::estimate_tokens(&outbound) + max_output.unwrap_or(0);
    let ticket = match session.budget.reserve(estimate) {
        Ok(t) => t,
        Err(e) => {
            return fail(
                session,
                AiError::from(e).action("Raise the budget or reset usage"),
            );
        }
    };
    let effort = ai::negotiate_effort(
        req.get("effort"),
        req.get("capability"),
        Some(family),
        now as f64,
    );
    let built = family.build(&BuildParams {
        key,
        model,
        instructions,
        input,
        max_output_tokens: Json::int(max_output.unwrap_or(ai::DEFAULT_MAX_OUTPUT_TOKENS)),
        effort: effort.sent,
        api_version: req.get("api_version").and_then(Json::as_str),
        max_tokens_param: req.get("max_tokens_param").and_then(Json::as_str),
    });
    let id = session.ids.next("airequest", 16);
    if session.ai_pending.len() >= MAX_PENDING {
        let mut oldest = session.ai_pending.remove(0);
        session.budget.settle(&mut oldest.ticket, None);
    }
    session.ai_pending.push(PendingAi {
        id: id.clone(),
        ticket,
        family,
        effort: effort.json.clone(),
        started: now,
    });
    let headers = Json::Obj(
        built
            .headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), Json::Str(v)))
            .collect(),
    );
    Ok(vec![
        ("request", Json::Str(id)),
        (
            "http",
            Json::obj(vec![
                ("url", Json::Str(url)),
                ("headers", headers),
                ("body", built.body),
            ]),
        ),
        ("timeout_ms", Json::int(timeout_ms)),
        (
            "max_response_units",
            Json::int(ai::MAX_RESPONSE_UNITS as i64),
        ),
        ("effort", effort.json),
        ("budget", budget_json(&session.budget.snapshot())),
    ])
}

/// The outcome of a prepared request: a transport failure, an HTTP status, or the response body.
fn ai_complete(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let request = need_str(req, "request")?;
    let Some(at) = session.ai_pending.iter().position(|p| p.id == request) else {
        return Err(OpError::invalid("request"));
    };
    let mut pending = session.ai_pending.remove(at);
    // everything except a usable response settles the reservation with its estimate
    let mut failed = |session: &mut Session, e: AiError| {
        session.budget.settle(&mut pending.ticket, None);
        Err(ai_error(e, session))
    };

    if let Some(t) = req.get("transport_error").filter(|t| !t.is_null()) {
        let user_aborted = opt_bool(t, "user_aborted", false)?;
        let e = ai::map_fetch_error(lenient(t, "name"), lenient(t, "code"), user_aborted);
        return failed(session, e);
    }
    let status = req
        .get("status")
        .and_then(Json::as_i64)
        .ok_or_else(|| OpError::invalid("status"))?;
    if !(200..300).contains(&status) {
        // `Number(headers.get('retry-after'))`: absent header = null = 0, no header API = undefined = NaN
        let retry = match req.get("retry_after") {
            None => f64::NAN,
            Some(Json::Null) => 0.0,
            Some(Json::Str(s)) => ai::js_number(s),
            Some(Json::Num(n)) => *n,
            Some(_) => return Err(OpError::invalid("retry_after")),
        };
        let seconds = retry.is_finite().then_some(retry);
        let e = ai::map_http_status(status, seconds);
        if e.code == "AI_RATE_LIMIT" {
            let wait_ms = match seconds {
                Some(s) if s > 0.0 => (s * 1000.0) as u64,
                _ => 30_000,
            };
            session.ai_cooldown_until = now.saturating_add(wait_ms);
        }
        return failed(session, e);
    }
    let body = match req.get("body") {
        Some(Json::Str(b)) => Some(b),
        _ => None,
    };
    let too_large = opt_bool(req, "oversize", false)?
        || body.is_some_and(|b| utf16_len(b) > ai::MAX_RESPONSE_UNITS);
    if too_large {
        return failed(
            session,
            AiError::new("OUTPUT_INVALID", "response too large"),
        );
    }
    let Some(body) = body else {
        return Err(OpError::invalid("body"));
    };
    let Ok(parsed_json) = json::parse(body, BODY_LIMITS) else {
        return failed(
            session,
            AiError::new("OUTPUT_INVALID", "response is not JSON"),
        );
    };
    let parsed = pending.family.parse(&parsed_json);
    let usage = parsed.usage.map(|u| Usage {
        input: u.input,
        output: u.output,
    });
    session.budget.settle(&mut pending.ticket, usage);
    if js_trim(&parsed.text).is_empty() {
        return Err(ai_error(
            AiError::new("OUTPUT_INVALID", "empty model result"),
            session,
        ));
    }
    Ok(vec![
        ("text", Json::Str(parsed.text)),
        (
            "usage",
            parsed.usage.map_or(Json::Null, |u| {
                Json::obj(vec![
                    ("input", Json::int(u.input)),
                    ("output", Json::int(u.output)),
                ])
            }),
        ),
        ("estimated", Json::Bool(parsed.usage.is_none())),
        ("effort", pending.effort),
        (
            "elapsed_ms",
            Json::int(now.saturating_sub(pending.started) as i64),
        ),
        ("budget", budget_json(&session.budget.snapshot())),
    ])
}

/// `parseModelResult`: the strict validation of a model reply (model output is data, never instructions).
fn ai_parse(req: &Json) -> OpResult {
    let raw = match req.get("raw") {
        Some(Json::Str(r)) => r.as_str(),
        _ => {
            return Err(OpError::coded(
                "OUTPUT_INVALID",
                "model output missing or too large",
            ));
        }
    };
    let task = opt_str(req, "task")?.unwrap_or("full");
    let baseline = opt_str(req, "baseline")?.unwrap_or("");
    let current_file = opt_str(req, "current_file")?.unwrap_or("");
    let result = ai::parse_model_result(raw, task, baseline, current_file)?;
    Ok(vec![("result", result.to_json())])
}

// ---------------------------------------------------------------- privacy.*

fn privacy_assess(req: &Json) -> OpResult {
    let text = need_str(req, "text")?;
    let file_name = opt_str(req, "file_name")?.unwrap_or("");
    let scope = opt_str(req, "scope")?.unwrap_or("realtime");
    let (action, scanned) = secrets::assess_outbound(text, file_name, scope);
    // findings carry the kind, severity and line, never the value
    let findings = scanned
        .findings
        .iter()
        .map(|f| {
            Json::obj(vec![
                ("kind", Json::str(f.kind)),
                ("severity", Json::str(f.severity)),
                ("line", Json::int(f.line as i64)),
            ])
        })
        .collect();
    let summary = secrets::summarize(&scanned.findings)
        .into_iter()
        .map(|(k, n)| (k.to_string(), Json::int(n as i64)))
        .collect();
    Ok(vec![
        ("action", Json::str(action)),
        (
            "scanned",
            Json::obj(vec![
                ("highest", Json::str(scanned.highest)),
                ("sensitiveFile", Json::Bool(scanned.sensitive_file)),
                ("blocked", Json::Bool(scanned.blocked)),
                ("truncated", Json::Bool(scanned.truncated)),
                ("findings", Json::Arr(findings)),
            ]),
        ),
        ("summary", Json::Obj(summary)),
    ])
}

fn privacy_redact(session: &mut Session, req: &Json) -> OpResult {
    let text = need_str(req, "text")?;
    let existing = opt_str(req, "redaction")?;
    let scanned = secrets::scan(text, "");
    let (redacted, entries) = secrets::redact(text, &scanned.findings);
    let at = match existing {
        Some(id) => session
            .redactions
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| OpError::invalid("redaction"))?,
        None => {
            if session.redactions.len() >= MAX_REDACTION_SETS {
                session.redactions.remove(0);
            }
            let id = session.ids.next("redaction", 8);
            session.redactions.push(RedactionSet {
                id,
                entries: Vec::new(),
            });
            session.redactions.len() - 1
        }
    };
    let set = &mut session.redactions[at];
    if set.entries.len() + entries.len() > MAX_REDACTIONS_PER_SET {
        return Err(OpError::coded(
            "PAYLOAD_TOO_LARGE",
            "too many redacted values",
        ));
    }
    let count = entries.len();
    set.entries.extend(entries);
    Ok(vec![
        ("text", Json::Str(redacted)),
        ("count", Json::int(count as i64)),
        ("total", Json::int(set.entries.len() as i64)),
        ("redaction", Json::str(&set.id)),
    ])
}

fn privacy_restore(session: &Session, req: &Json) -> OpResult {
    let text = need_str(req, "text")?;
    let id = need_str(req, "redaction")?;
    let set = session
        .redactions
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| OpError::invalid("redaction"))?;
    let (restored, unresolved) = secrets::restore(text, &set.entries);
    Ok(vec![
        ("text", Json::Str(restored)),
        ("unresolved", Json::int(unresolved as i64)),
    ])
}

// ---------------------------------------------------------------- annotate.* and symbols.*

fn annotate_prompt(req: &Json) -> OpResult {
    let dialect_values = dialect(req)?;
    let comment = opt_bool(req, "comment", false)?;
    let graph = opt_bool(req, "graph", false)?;
    let args = PromptArgs {
        language: need_str(req, "language")?,
        dialect: match req.get("dialect") {
            None | Some(Json::Null) => Json::Obj(Vec::new()),
            Some(d) => d.clone(),
        },
        locale: opt_str(req, "locale")?.unwrap_or("en-US"),
        comment,
        graph,
        file_name: opt_str(req, "file_name")?.unwrap_or(""),
        file_text: opt_str(req, "file_text")?.unwrap_or(""),
        snippet: opt_str(req, "snippet")?,
        line: req.get("line").filter(|l| !l.is_null()).cloned(),
        current_symbols: req
            .get("current_symbols")
            .filter(|s| !s.is_null())
            .cloned()
            .unwrap_or(Json::Arr(Vec::new())),
        directory_symbols: req
            .get("directory_symbols")
            .filter(|s| !s.is_null())
            .cloned()
            .unwrap_or(Json::Arr(Vec::new())),
    };
    let (instructions, input) = annotate::build_annotate_prompt(&args, &dialect_values);
    Ok(vec![
        ("instructions", Json::Str(instructions)),
        ("input", Json::Str(input)),
    ])
}

fn annotate_apply(req: &Json) -> OpResult {
    let out = annotate::apply_annotate_reply(
        need_str(req, "raw")?,
        need_str(req, "profile")?,
        need_str(req, "original")?,
        &dialect(req)?,
        opt_bool(req, "comment", false)?,
        opt_bool(req, "graph", false)?,
    );
    let mut reply = vec![
        ("text", Json::Str(out.text)),
        ("notes", Json::strs(&out.notes)),
    ];
    if let Some((commented, graph)) = out.flags {
        reply.push(("commented", Json::Bool(commented)));
        reply.push(("graph", Json::Bool(graph)));
    }
    Ok(reply)
}

fn symbols_extract(req: &Json) -> OpResult {
    let max = match req.get("max") {
        None | Some(Json::Null) => symbols::DEFAULT_MAX,
        Some(v) => match v.as_i64() {
            Some(n) if n >= 0 => (n as usize).min(symbols::LIMIT_MAX),
            _ => return Err(OpError::invalid("max")),
        },
    };
    let found = symbols::extract_symbols(need_str(req, "language")?, need_str(req, "text")?, max);
    Ok(vec![(
        "symbols",
        Json::Arr(
            found
                .iter()
                .map(|s| {
                    Json::obj(vec![
                        ("kind", Json::str(&s.kind)),
                        ("name", Json::str(&s.name)),
                    ])
                })
                .collect(),
        ),
    )])
}

fn symbols_dir_candidates(req: &Json) -> OpResult {
    let current = need_str(req, "current_name")?;
    let Some(Json::Arr(list)) = req.get("entries") else {
        return Err(OpError::invalid("entries"));
    };
    if list.len() > MAX_DIR_ENTRIES {
        return Err(OpError::invalid("entries"));
    }
    let mut entries = Vec::with_capacity(list.len());
    for e in list {
        let name = e
            .get("name")
            .and_then(Json::as_str)
            .ok_or_else(|| OpError::invalid("entries.name"))?;
        let is_file = match e.get("is_file") {
            Some(Json::Bool(b)) => *b,
            _ => return Err(OpError::invalid("entries.is_file")),
        };
        entries.push(DirEntry { name, is_file });
    }
    let found = symbols::directory_candidates(current, &entries);
    Ok(vec![(
        "candidates",
        Json::Arr(
            found
                .iter()
                .map(|(name, language)| {
                    Json::obj(vec![
                        ("name", Json::str(name)),
                        ("language", Json::str(language)),
                    ])
                })
                .collect(),
        ),
    )])
}

// ---------------------------------------------------------------- sched.* and lang.*

fn sched_reply(session: &Session, mut rest: Vec<(&'static str, Json)>) -> OpResult {
    rest.push(("stats", stats_json(&session.sched.stats)));
    Ok(rest)
}

fn sched_finish(session: &mut Session, req: &Json) -> OpResult {
    let key = need_key(req)?;
    let outcome = if need_bool(req, "ok")? {
        Ok(())
    } else {
        Err(opt_str(req, "code")?)
    };
    let verdict = session.sched.finish(key, outcome);
    let mut reply = Vec::new();
    match verdict {
        Verdict::Deliver => reply.push(("action", Json::str("deliver"))),
        Verdict::Discard(code) => {
            reply.push(("action", Json::str("discard")));
            reply.push(("code", Json::Str(code)));
        }
    }
    sched_reply(session, reply)
}

fn sched_touch(session: &mut Session, req: &Json) -> OpResult {
    let mut reply = Vec::new();
    match need_str(req, "kind")? {
        "input" => {
            session.input.note_input();
        }
        "document" => {
            session.input.note_document_change();
        }
        "language" => {
            let changed = session.input.set_language(need_str(req, "language")?);
            reply.push(("changed", Json::Bool(changed)));
        }
        _ => return Err(OpError::invalid("kind")),
    }
    reply.push(("key", key_json(&session.input.key())));
    Ok(reply)
}

fn scope_of(req: &Json) -> Result<LockScope, OpError> {
    match opt_str(req, "scope")?.unwrap_or("file") {
        "file" => Ok(LockScope::File),
        "global" => Ok(LockScope::Global),
        _ => Err(OpError::invalid("scope")),
    }
}

fn locks_json(session: &Session) -> Vec<(&'static str, Json)> {
    vec![
        (
            "file_locks",
            Json::Obj(
                session
                    .input
                    .file_locks()
                    .iter()
                    .map(|(k, v)| (k.clone(), Json::str(v)))
                    .collect(),
            ),
        ),
        (
            "global_lock",
            session.input.global_lock().map_or(Json::Null, Json::str),
        ),
    ]
}

fn lang_state(session: &mut Session, req: &Json) -> OpResult {
    let ctx = ResolveContext {
        language_id: opt_str(req, "language_id")?,
        file_name: opt_str(req, "file_name")?,
        text: opt_str(req, "text")?,
    };
    let file_key = opt_str(req, "file_key")?.unwrap_or("");
    let registry = crate::predict::engine().registry();
    let d = session.input.resolve(registry, &ctx, file_key);
    let hints = Json::Obj(
        d.dialect_hints
            .iter()
            .map(|(k, v)| (k.clone(), Json::str(v)))
            .collect(),
    );
    Ok(vec![
        (
            "decision",
            Json::obj(vec![
                ("language", Json::str(&d.language)),
                ("reason", Json::str(d.reason)),
                ("changed", Json::Bool(d.changed)),
                ("uncertain", Json::Bool(d.uncertain)),
                (
                    "suggestion",
                    d.suggestion.as_deref().map_or(Json::Null, Json::str),
                ),
                ("confidence", Json::int(i64::from(d.confidence))),
                ("dialectHints", hints),
                ("locked", Json::Bool(d.locked)),
            ]),
        ),
        ("key", key_json(&session.input.key())),
    ])
}

pub fn run(session: &mut Session, now: u64, op: &str, req: &Json) -> OpResult {
    match op {
        "ai.prompt" => ai_prompt(req),
        "ai.prepare" => ai_prepare(session, now, req),
        "ai.complete" => ai_complete(session, now, req),
        "ai.parse" => ai_parse(req),
        "privacy.assess" => privacy_assess(req),
        "privacy.redact" => privacy_redact(session, req),
        "privacy.restore" => privacy_restore(session, req),
        "privacy.release" => {
            let id = need_str(req, "redaction")?;
            let before = session.redactions.len();
            session.redactions.retain(|r| r.id != id);
            Ok(vec![(
                "released",
                Json::Bool(session.redactions.len() != before),
            )])
        }
        "annotate.supports" => {
            let language = need_str(req, "language")?;
            let prefix = annotate::line_prefix(language, &dialect(req)?);
            Ok(vec![
                ("supported", Json::Bool(prefix.is_some())),
                ("prefix", prefix.map_or(Json::Null, Json::str)),
            ])
        }
        "annotate.prompt" => annotate_prompt(req),
        "annotate.f1_addendum" => Ok(vec![(
            "text",
            Json::Str(annotate::f1_addendum(
                need_str(req, "task")?,
                opt_bool(req, "comment", false)?,
                opt_bool(req, "graph", false)?,
                opt_str(req, "locale")?.unwrap_or("en-US"),
                need_str(req, "language")?,
                &dialect(req)?,
            )),
        )]),
        "annotate.apply" => annotate_apply(req),
        "symbols.extract" => symbols_extract(req),
        "symbols.dir_candidates" => symbols_dir_candidates(req),
        "sched.configure" => {
            let ms = req.get("debounce_ms").and_then(Json::as_f64);
            let applied = session.sched.set_debounce(ms);
            Ok(vec![("debounce_ms", Json::int(applied))])
        }
        "sched.schedule" => {
            let effects = session
                .sched
                .schedule(need_key(req)?, need_bool(req, "allowed")?);
            sched_reply(session, vec![("effects", effects_json(&effects))])
        }
        "sched.fire" => {
            let start = session
                .sched
                .fire(need_key(req)?, need_bool(req, "allowed")?);
            sched_reply(session, vec![("start", Json::Bool(start))])
        }
        "sched.finish" => sched_finish(session, req),
        "sched.cancel" => {
            let effects = session.sched.cancel();
            sched_reply(session, vec![("effects", effects_json(&effects))])
        }
        "sched.touch" => sched_touch(session, req),
        "sched.is_current" => {
            let current =
                parse_key(req.get("key"), "key")?.is_some_and(|k| session.input.is_current(&k));
            Ok(vec![("current", Json::Bool(current))])
        }
        "lang.state" => lang_state(session, req),
        "lang.lock" => {
            let locked = session.input.lock(
                need_str(req, "profile")?,
                scope_of(req)?,
                opt_str(req, "file_key")?.unwrap_or(""),
            );
            let mut reply = vec![("locked", Json::Bool(locked))];
            reply.extend(locks_json(session));
            Ok(reply)
        }
        "lang.unlock" => {
            session
                .input
                .unlock(scope_of(req)?, opt_str(req, "file_key")?.unwrap_or(""));
            Ok(locks_json(session))
        }
        _ => Err(OpError::coded("UNKNOWN_OPERATION", op)),
    }
}
