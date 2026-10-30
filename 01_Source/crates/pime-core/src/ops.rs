//! Protocol 0.3 operations (stateful, session based). Protocol 0.3 is a superset of 0.2: every 0.2 operation keeps
//! its exact behaviour (see `protocol.rs`), and the operations below are added.
//!
//! Conventions
//! - Every operation except `session.open` names an open session in the top-level field `session`; a missing,
//!   unknown, evicted or expired session answers `{"ok":false,"error":"NO_SESSION"}`.
//! - Failures are `{"ok":false,"id":..,"error":"<CODE>","message":"..."}` where `<CODE>` is a stable code of the
//!   shared error model (`error.rs`) or a protocol code (`INVALID_REQUEST` with a `field`, `NO_SESSION`). Messages
//!   never contain document text, prompts or credentials.
//! - New top-level request fields are snake_case. Nested objects that mirror the JavaScript bridge's own shapes
//!   keep their camelCase names (`state.isClosed`, edits `{start,end,text}`, grant objects, hunk geometry).
//! - All offsets are UTF-16 code units. The core keeps document text (snapshots, proposals) and never echoes it
//!   except where a reply is defined to carry it (`edit.result`, the 4000-unit `preview`, hunk display lines).

use crate::budget::Usage;
use crate::context::{self, SnapshotInput, selection_json};
use crate::edit::diff::{self, Hunk};
use crate::edit::transaction::{ApplyOutcome, DocState, Plan, PlanRequest, PrepareArgs};
use crate::error::PimeError;
use crate::grants::{self, AuthorizeRequest, Criteria, Grant};
use crate::ids::{IdGen, MAX_ENTROPY_BYTES};
use crate::json::Json;
use crate::protocol::dialect_of;
use crate::session::{MAX_TICKETS, OpenError, Proposal, Session, SessionStore};
use crate::settings;
use crate::sha256::{from_hex, sha256_hex};
use crate::textrange::{self, Edit, Position, Range};
use crate::util::{head_utf16, utf16_len};
use crate::validators::{self, Dialect};

/// Every 0.3 operation this core implements.
pub const OPS: &[&str] = &[
    "session.open",
    "session.close",
    "settings.apply",
    "settings.resolve",
    "ctx.snapshot",
    "ctx.excerpt",
    "ctx.validate",
    "ctx.release",
    "edit.propose",
    "edit.select",
    "edit.result",
    "edit.plan",
    "edit.prepare",
    "edit.begin_apply",
    "edit.finish_apply",
    "edit.invalidate",
    "edit.discard",
    "edit.purge",
    "edit.status",
    "grant.workspace_token",
    "grant.authorize",
    "grant.check",
    "grant.find",
    "grant.require",
    "grant.consume",
    "grant.revoke",
    "grant.revoke_provider",
    "grant.revoke_provider_prefix",
    "grant.revoke_all",
    "grant.still_valid",
    "grant.list",
    "grant.export",
    "grant.import",
    "budget.configure",
    "budget.reserve",
    "budget.settle",
    "budget.release",
    "budget.snapshot",
    "budget.reset",
];

/// Operations that carry document text and may therefore use a request line longer than the 0.2 limit.
pub const BULK_OPS: &[&str] = &[
    "ctx.snapshot",
    "ctx.validate",
    "edit.propose",
    "edit.plan",
    "edit.prepare",
    "edit.begin_apply",
    "edit.finish_apply",
];

/// The F1 preview of a result shows its first 4000 UTF-16 units.
pub const PREVIEW_UNITS: usize = 4000;
const MAX_HUNK_IDS: usize = 100_000;
const MAX_GRANT_ENTRIES: usize = 1000;

/// A failed operation, rendered as a protocol error response.
#[derive(Debug)]
pub struct OpError {
    pub code: String,
    pub message: Option<String>,
    pub field: Option<String>,
}

impl OpError {
    pub fn invalid(field: &str) -> OpError {
        OpError {
            code: "INVALID_REQUEST".to_string(),
            message: None,
            field: Some(field.to_string()),
        }
    }

    pub fn coded(code: &str, message: &str) -> OpError {
        OpError {
            code: code.to_string(),
            message: Some(message.to_string()),
            field: None,
        }
    }

    pub fn to_json(&self, id: Option<i64>) -> Json {
        let mut pairs = vec![("ok", Json::Bool(false))];
        if let Some(i) = id {
            pairs.push(("id", Json::int(i)));
        }
        pairs.push(("error", Json::str(&self.code)));
        if let Some(m) = &self.message {
            pairs.push((
                "message",
                Json::str(&m.chars().take(200).collect::<String>()),
            ));
        }
        if let Some(f) = &self.field {
            pairs.push(("field", Json::str(f)));
        }
        Json::obj(pairs)
    }
}

impl From<PimeError> for OpError {
    fn from(e: PimeError) -> OpError {
        OpError {
            code: e.code.to_string(),
            message: Some(e.message),
            field: None,
        }
    }
}

pub type OpResult = Result<Vec<(&'static str, Json)>, OpError>;

fn ok(id: Option<i64>, mut rest: Vec<(&'static str, Json)>) -> Json {
    let mut pairs = vec![("ok", Json::Bool(true))];
    if let Some(i) = id {
        pairs.push(("id", Json::int(i)));
    }
    pairs.append(&mut rest);
    Json::obj(pairs)
}

// ---------------------------------------------------------------- request parsing

/// Required string field.
pub fn need_str<'a>(req: &'a Json, key: &str) -> Result<&'a str, OpError> {
    match req.get(key) {
        Some(Json::Str(s)) => Ok(s),
        _ => Err(OpError::invalid(key)),
    }
}

/// Optional string field (absent or null = `None`).
fn opt_str<'a>(req: &'a Json, key: &str) -> Result<Option<&'a str>, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s)),
        Some(_) => Err(OpError::invalid(key)),
    }
}

fn opt_bool(req: &Json, key: &str, default: bool) -> Result<bool, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(OpError::invalid(key)),
    }
}

fn opt_usize(req: &Json, key: &str, default: usize) -> Result<usize, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(v) => match v.as_i64() {
            Some(n) if n >= 0 => Ok(n as usize),
            _ => Err(OpError::invalid(key)),
        },
    }
}

fn parse_position(v: Option<&Json>) -> Option<Position> {
    let v = v?;
    Some(Position {
        line: v.get("line")?.as_i64()?,
        character: v.get("character")?.as_i64()?,
    })
}

fn parse_selection(v: Option<&Json>, field: &str) -> Result<Option<Range>, OpError> {
    match v {
        None | Some(Json::Null) => Ok(None),
        Some(o) => match (parse_position(o.get("start")), parse_position(o.get("end"))) {
            (Some(start), Some(end)) => Ok(Some(Range { start, end })),
            _ => Err(OpError::invalid(field)),
        },
    }
}

/// The document state as the editor reports it; `null`/absent = no document (counts as closed).
fn parse_state(v: Option<&Json>, field: &str) -> Result<Option<DocState>, OpError> {
    let o = match v {
        None | Some(Json::Null) => return Ok(None),
        Some(o @ Json::Obj(_)) => o,
        Some(_) => return Err(OpError::invalid(field)),
    };
    let sub = |k: &str| OpError::invalid(&format!("{field}.{k}"));
    Ok(Some(DocState {
        uri: o
            .get("uri")
            .and_then(Json::as_str)
            .ok_or_else(|| sub("uri"))?
            .to_string(),
        version: o
            .get("version")
            .and_then(Json::as_i64)
            .ok_or_else(|| sub("version"))?,
        text: o
            .get("text")
            .and_then(Json::as_str)
            .ok_or_else(|| sub("text"))?
            .to_string(),
        is_closed: opt_bool(o, "isClosed", false).map_err(|_| sub("isClosed"))?,
        editable: opt_bool(o, "editable", true).map_err(|_| sub("editable"))?,
        selection: parse_selection(o.get("selection"), &format!("{field}.selection"))?,
    }))
}

/// `[{start, end, text}]` in UTF-16 offsets.
fn parse_edits(v: Option<&Json>) -> Result<Vec<Edit>, OpError> {
    let Some(Json::Arr(a)) = v else {
        return Err(OpError::coded("PATCH_INVALID", "plan has no edits"));
    };
    let mut out = Vec::with_capacity(a.len());
    for e in a {
        let offset = |k: &str| match e.get(k).and_then(Json::as_i64) {
            Some(n) if n >= 0 => Ok(n as usize),
            _ => Err(OpError::coded("TARGET_RANGE_INVALID", "invalid edit range")),
        };
        let (start, end) = (offset("start")?, offset("end")?);
        let Some(Json::Str(text)) = e.get("text") else {
            return Err(OpError::coded("PATCH_INVALID", "edit text missing"));
        };
        out.push(Edit {
            start,
            end,
            text: text.clone(),
        });
    }
    Ok(out)
}

fn edits_json(edits: &[Edit]) -> Json {
    Json::Arr(
        edits
            .iter()
            .map(|e| {
                Json::obj(vec![
                    ("start", Json::int(e.start as i64)),
                    ("end", Json::int(e.end as i64)),
                    ("text", Json::str(&e.text)),
                ])
            })
            .collect(),
    )
}

fn str_list(v: Option<&Json>, field: &str) -> Result<Vec<String>, OpError> {
    let Some(Json::Arr(a)) = v else {
        return Err(OpError::invalid(field));
    };
    if a.len() > MAX_HUNK_IDS {
        return Err(OpError::invalid(field));
    }
    a.iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or_else(|| OpError::invalid(field))
        })
        .collect()
}

/// `field` is a workspace token (`workspace`) or the workspace folder uris (`workspace_uris`), which the core hashes.
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

// ---------------------------------------------------------------- response building

fn validation_json(r: &validators::ValidationResult) -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(r.ok)),
        ("level", Json::str(r.level)),
        (
            "issues",
            Json::Arr(
                r.issues
                    .iter()
                    .map(|i| {
                        Json::obj(vec![
                            ("line", Json::int(i.line as i64)),
                            ("code", Json::str(i.code)),
                            ("message", Json::str(&i.message)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn hunks_json(base: &str, hunks: &[Hunk]) -> Json {
    let views = diff::render_hunks(base, hunks, diff::DEFAULT_CONTEXT);
    Json::Arr(
        hunks
            .iter()
            .zip(views)
            .map(|(h, v)| {
                Json::obj(vec![
                    ("id", Json::str(&h.id)),
                    ("oldStartLine", Json::int(h.old_start_line as i64)),
                    ("oldLineCount", Json::int(h.old_line_count as i64)),
                    ("newStartLine", Json::int(h.new_start_line as i64)),
                    ("newLineCount", Json::int(h.new_line_count as i64)),
                    ("startOffset", Json::int(h.start_offset as i64)),
                    ("endOffset", Json::int(h.end_offset as i64)),
                    ("header", Json::str(&v.header)),
                    (
                        "lines",
                        Json::Arr(
                            v.lines
                                .iter()
                                .map(|(k, t)| {
                                    Json::obj(vec![
                                        ("kind", Json::str(&k.to_string())),
                                        ("text", Json::str(t)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    )
}

/// What the F1 panel shows for the result of a hunk selection: length, hash, the first 4000 units and the
/// validation verdict (when a language is known).
fn result_json(
    base: &str,
    hunks: &[Hunk],
    selected: &[String],
    language: Option<&str>,
    dialect: &Dialect,
) -> Result<Json, OpError> {
    let text = diff::apply_hunks(base, hunks, selected)?;
    let length = utf16_len(&text);
    let mut pairs = vec![
        ("length", Json::int(length as i64)),
        ("hash", Json::str(&sha256_hex(&text))),
        ("preview", Json::str(head_utf16(&text, PREVIEW_UNITS))),
        ("truncated", Json::Bool(length > PREVIEW_UNITS)),
    ];
    if let Some(l) = language {
        pairs.push((
            "validation",
            validation_json(&validators::validate_source(l, &text, dialect)),
        ));
    }
    Ok(Json::obj(pairs))
}

fn plan_json(p: &Plan) -> Json {
    Json::obj(vec![
        ("id", Json::str(&p.id)),
        ("kind", Json::str(p.kind.as_str())),
        ("uri", Json::str(&p.uri)),
        ("editorInstance", Json::str(&p.editor_instance)),
        (
            "baseline",
            Json::obj(vec![
                ("version", Json::int(p.baseline_version)),
                ("hash", Json::str(&p.baseline_hash)),
            ]),
        ),
        ("selection", selection_json(&p.selection)),
        ("proposedHash", Json::str(&p.proposed_hash)),
        ("expiresAt", Json::Num(p.expires_at as f64)),
        ("status", Json::str(p.status.as_str())),
        ("editCount", Json::int(p.edits.len() as i64)),
    ])
}

fn unknown_snapshot() -> OpError {
    OpError::coded("CONTEXT_UNAVAILABLE", "unknown or released snapshot")
}

// ---------------------------------------------------------------- session and settings

fn field_path(root: &str, field: &str) -> OpError {
    if field == "values" {
        OpError::invalid(root)
    } else {
        OpError::invalid(&format!("{root}.{field}"))
    }
}

fn issues_json(issues: &[settings::Issue]) -> Json {
    Json::Arr(
        issues
            .iter()
            .map(|i| {
                Json::obj(vec![
                    ("path", Json::str(&i.path)),
                    ("message", Json::str(i.message)),
                ])
            })
            .collect(),
    )
}

fn open_session(store: &mut SessionStore, now: u64, req: &Json) -> OpResult {
    let locale = match req.get("locale") {
        Some(Json::Str(l)) if !l.is_empty() && l.len() <= 32 => l.clone(),
        _ => return Err(OpError::invalid("locale")),
    };
    // 32..=512 bytes of caller supplied entropy as hex (see `ids.rs` for the construction and its limits)
    let entropy_hex = need_str(req, "entropy")?;
    if entropy_hex.len() > MAX_ENTROPY_BYTES * 2 {
        return Err(OpError::invalid("entropy"));
    }
    let ids = from_hex(entropy_hex)
        .and_then(|bytes| IdGen::new(&bytes))
        .ok_or_else(|| OpError::invalid("entropy"))?;
    let values = req.get("settings").unwrap_or(&Json::Null);
    let effective = settings::effective(values).map_err(|f| field_path("settings", &f))?;
    let issues = settings::validate(&effective);
    let session = store
        .open(now, ids, locale, effective)
        .map_err(|e| match e {
            OpenError::EntropyReused => OpError::invalid("entropy"),
        })?;
    Ok(vec![
        ("session", Json::str(&session.id)),
        ("protocol", Json::str(crate::PROTOCOL)),
        ("issues", issues_json(&issues)),
    ])
}

fn apply_settings(session: &mut Session, req: &Json) -> OpResult {
    let values = req
        .get("values")
        .ok_or_else(|| OpError::invalid("values"))?;
    let effective = settings::effective(values).map_err(|f| field_path("values", &f))?;
    if let Some(l) = req.get("locale") {
        match l {
            Json::Str(l) if !l.is_empty() && l.len() <= 32 => session.locale = l.clone(),
            Json::Null => {}
            _ => return Err(OpError::invalid("locale")),
        }
    }
    let issues = settings::validate(&effective);
    session.settings = effective.clone();
    Ok(vec![
        ("settings", effective),
        ("issues", issues_json(&issues)),
    ])
}

fn resolve_scenario(session: &Session, req: &Json) -> OpResult {
    let name = need_str(req, "scenario")?;
    let resolved = settings::resolve_scenario(&session.settings, name)?;
    Ok(vec![("scenario", resolved)])
}

// ---------------------------------------------------------------- context

fn ctx_snapshot(session: &mut Session, req: &Json) -> OpResult {
    let version = req
        .get("version")
        .and_then(Json::as_i64)
        .ok_or_else(|| OpError::invalid("version"))?;
    let level = opt_str(req, "level")?.unwrap_or("L3");
    if level.len() > 16 {
        return Err(OpError::invalid("level"));
    }
    let input = SnapshotInput {
        uri: need_str(req, "uri")?.to_string(),
        version,
        text: match req.get("text") {
            Some(Json::Str(t)) => Some(t.clone()),
            _ => None,
        },
        language_id: opt_str(req, "languageId")?.unwrap_or("").to_string(),
        selection: parse_selection(req.get("selection"), "selection")?,
        is_dirty: opt_bool(req, "isDirty", false)?,
        level: level.to_string(),
        editor_instance_id: opt_str(req, "editorInstanceId")?
            .unwrap_or("editor-1")
            .to_string(),
        file_name: opt_str(req, "fileName")?.unwrap_or("").to_string(),
    };
    let snapshot = context::make_snapshot(input)?;
    let stored = session.store_snapshot(snapshot);
    Ok(vec![("snapshot", stored.meta_json())])
}

fn ctx_excerpt(session: &mut Session, req: &Json) -> OpResult {
    let snap = session
        .snapshot(need_str(req, "snapshot_id")?)
        .ok_or_else(unknown_snapshot)?;
    let cursor = req
        .get("cursor")
        .and_then(Json::as_i64)
        .ok_or_else(|| OpError::invalid("cursor"))?;
    let before = opt_usize(req, "before", context::DEFAULT_BEFORE)?;
    let after = opt_usize(req, "after", context::DEFAULT_AFTER)?;
    if let Some(policy) = req.get("policy").filter(|p| !p.is_null()) {
        let bound = |k: &str| match policy.get(k).and_then(Json::as_i64) {
            Some(n) if n >= 0 => Ok(n as usize),
            _ => Err(OpError::invalid(&format!("policy.{k}"))),
        };
        context::require_within_policy((before, after), (bound("before")?, bound("after")?))?;
    }
    let ex = context::excerpt(&snap.content, cursor, before, after);
    Ok(vec![(
        "excerpt",
        Json::obj(vec![
            ("text", Json::str(&ex.text)),
            ("start", Json::int(ex.start as i64)),
            ("end", Json::int(ex.end as i64)),
            ("cursorInExcerpt", Json::int(ex.cursor_in_excerpt as i64)),
            ("truncated", Json::Bool(ex.truncated)),
        ]),
    )])
}

/// Validates the snapshot text, or the snapshot with `edits` applied (a preview of an insert).
fn ctx_validate(session: &mut Session, req: &Json) -> OpResult {
    let snap = session
        .snapshot(need_str(req, "snapshot_id")?)
        .ok_or_else(unknown_snapshot)?;
    let language = need_str(req, "language")?;
    let dialect = dialect_of(req).map_err(|f| OpError::invalid(&f))?;
    let result = match req.get("edits") {
        None | Some(Json::Null) => validators::validate_source(language, &snap.content, &dialect),
        Some(e) => {
            let text = textrange::apply_edits(&snap.content, &parse_edits(Some(e))?)?;
            validators::validate_source(language, &text, &dialect)
        }
    };
    Ok(vec![("result", validation_json(&result))])
}

fn ctx_release(session: &mut Session, req: &Json) -> OpResult {
    let released = session.release_snapshot(need_str(req, "snapshot_id")?);
    Ok(vec![("released", Json::Bool(released))])
}

// ---------------------------------------------------------------- edit

fn edit_propose(session: &mut Session, req: &Json) -> OpResult {
    let snapshot_id = need_str(req, "snapshot_id")?.to_string();
    let proposed = need_str(req, "proposed_text")?.to_string();
    let language = opt_str(req, "language")?.map(str::to_string);
    let dialect = dialect_of(req).map_err(|f| OpError::invalid(&f))?;
    let snap = session
        .snapshot(&snapshot_id)
        .ok_or_else(unknown_snapshot)?;
    let hunks = diff::build_hunks(&snap.content, &proposed);
    let all: Vec<String> = hunks.iter().map(|h| h.id.clone()).collect();
    let hunks_out = hunks_json(&snap.content, &hunks);
    let result = result_json(&snap.content, &hunks, &all, language.as_deref(), &dialect)?;
    let stats = diff::stats(&hunks);
    let after_hash = sha256_hex(&proposed);
    let after_length = utf16_len(&proposed);
    let id = session.ids.next("proposal", 16);
    session.store_proposal(Proposal {
        id: id.clone(),
        snapshot_id,
        proposed_text: proposed,
        hunks,
        language,
        dialect,
    });
    Ok(vec![
        ("proposal", Json::str(&id)),
        ("hunks", hunks_out),
        (
            "stats",
            Json::obj(vec![
                ("hunks", Json::int(stats.hunks as i64)),
                ("added", Json::int(stats.added as i64)),
                ("removed", Json::int(stats.removed as i64)),
            ]),
        ),
        ("after_hash", Json::str(&after_hash)),
        ("after_length", Json::int(after_length as i64)),
        ("result", result),
    ])
}

fn unknown_proposal() -> OpError {
    OpError::coded("TOKEN_INVALID", "unknown proposal")
}

/// The proposal, its baseline text and the language settings a selection or result is computed with.
fn proposal_context<'a>(
    session: &'a Session,
    req: &Json,
) -> Result<(&'a Proposal, &'a str, Option<String>, Dialect), OpError> {
    let p = session
        .proposal(need_str(req, "proposal")?)
        .ok_or_else(unknown_proposal)?;
    let base = session
        .snapshot(&p.snapshot_id)
        .ok_or_else(unknown_snapshot)?;
    let language = match opt_str(req, "language")? {
        Some(l) => Some(l.to_string()),
        None => p.language.clone(),
    };
    let dialect = if req.get("dialect").is_some_and(|d| !d.is_null()) {
        dialect_of(req).map_err(|f| OpError::invalid(&f))?
    } else {
        p.dialect.clone()
    };
    Ok((p, &base.content, language, dialect))
}

fn edit_select(session: &mut Session, req: &Json) -> OpResult {
    let ids = str_list(req.get("hunk_ids"), "hunk_ids")?;
    let (p, base, language, dialect) = proposal_context(session, req)?;
    let result = result_json(base, &p.hunks, &ids, language.as_deref(), &dialect)?;
    Ok(vec![("result", result)])
}

/// The whole text of the result of a selection (for the read-only Diff view).
fn edit_result(session: &mut Session, req: &Json) -> OpResult {
    let ids = str_list(req.get("hunk_ids"), "hunk_ids")?;
    let (p, base, _, _) = proposal_context(session, req)?;
    let text = diff::apply_hunks(base, &p.hunks, &ids)?;
    Ok(vec![("text", Json::Str(text))])
}

/// Creates a plan from explicit `edits` (insert, or any exact edit list) or from the chosen hunks of a proposal.
fn edit_plan(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let kind = need_str(req, "kind")?;
    let selection = parse_selection(req.get("selection"), "selection")?;
    let (snapshot_id, edits) = match opt_str(req, "proposal")? {
        Some(pid) => {
            let ids = str_list(req.get("hunk_ids"), "hunk_ids")?;
            let p = session.proposal(pid).ok_or_else(unknown_proposal)?;
            if opt_str(req, "snapshot_id")?.is_some_and(|s| s != p.snapshot_id) {
                return Err(OpError::invalid("snapshot_id"));
            }
            (p.snapshot_id.clone(), diff::hunk_edits(&p.hunks, &ids)?)
        }
        None => (
            need_str(req, "snapshot_id")?.to_string(),
            parse_edits(req.get("edits"))?,
        ),
    };
    let snapshot = session
        .snapshots
        .iter()
        .find(|s| s.snapshot_id == snapshot_id)
        .ok_or_else(unknown_snapshot)?;
    let targets = match req.get("targets") {
        None | Some(Json::Null) => vec![snapshot.document_uri.clone()],
        Some(_) => str_list(req.get("targets"), "targets")?,
    };
    let plan = session.coordinator.create_plan(
        now,
        &mut session.ids,
        PlanRequest {
            kind,
            snapshot,
            edits,
            targets,
            selection,
        },
    )?;
    Ok(vec![("plan", plan_json(&plan))])
}

fn edit_prepare(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let plan = need_str(req, "plan")?;
    let state = parse_state(req.get("state"), "state")?;
    // only the literal `true` of a deliberate user action counts as confirmation
    let confirmed = req.get("confirmed") == Some(&Json::Bool(true));
    // the core asks its own broker when it is given the grant id; a bare `grant_ok` is accepted only as literal true
    let grant_ok = match req.get("grant_id") {
        Some(Json::Str(id)) => session.broker.still_valid(now, id),
        _ => req.get("grant_ok") == Some(&Json::Bool(true)),
    };
    let validation_ok = !matches!(req.get("validation_ok"), Some(Json::Bool(false)));
    let token = session.coordinator.prepare(
        now,
        &mut session.ids,
        plan,
        PrepareArgs {
            state: state.as_ref(),
            confirmed,
            grant_ok,
            validation_ok,
        },
    )?;
    Ok(vec![("token", Json::Str(token))])
}

fn edit_begin_apply(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let state = parse_state(req.get("state"), "state")?;
    let edits = session
        .coordinator
        .begin_apply(now, need_str(req, "token")?, state.as_ref())?;
    Ok(vec![
        ("edits", edits_json(&edits)),
        (
            "apply_timeout_ms",
            Json::Num(session.coordinator.timing.apply_timeout_ms as f64),
        ),
    ])
}

fn edit_finish_apply(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let outcome = match need_str(req, "outcome")? {
        "accepted" => ApplyOutcome::Accepted,
        "rejected" => ApplyOutcome::Rejected,
        "error" => ApplyOutcome::Failed,
        _ => return Err(OpError::invalid("outcome")),
    };
    let state = parse_state(req.get("state"), "state")?;
    let applied =
        session
            .coordinator
            .finish_apply(now, need_str(req, "token")?, outcome, state.as_ref())?;
    Ok(vec![
        ("state", Json::str("APPLIED")),
        ("newVersion", Json::int(applied.new_version)),
        ("newHash", Json::Str(applied.new_hash)),
    ])
}

// ---------------------------------------------------------------- grants

fn persisted(session: &Session) -> Json {
    Json::Arr(session.broker.export())
}

fn criteria_of<'a>(req: &'a Json, workspace: &'a str) -> Result<Criteria<'a>, OpError> {
    Ok(Criteria {
        provider_id: need_str(req, "provider_id")?,
        workspace,
        scope: need_str(req, "scope")?,
    })
}

fn grant_authorize(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let workspace = workspace_of(req)?;
    let ttl_ms = match req.get("ttl_ms") {
        None | Some(Json::Null) => grants::DEFAULT_TTL_MS,
        Some(v) => match v.as_i64() {
            Some(n) if n >= 0 => n as u64,
            _ => return Err(OpError::invalid("ttl_ms")),
        },
    };
    let grant = session.broker.authorize(
        now,
        &mut session.ids,
        AuthorizeRequest {
            provider_id: need_str(req, "provider_id")?,
            workspace: &workspace,
            scope: need_str(req, "scope")?,
            ttl_ms,
            once: opt_bool(req, "once", false)?,
        },
    )?;
    Ok(vec![
        ("grant", grant.to_json()),
        ("persist", persisted(session)),
    ])
}

fn grant_check(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let workspace = workspace_of(req)?;
    let c = session.broker.check(
        now,
        need_str(req, "grant_id")?,
        criteria_of(req, &workspace)?,
    );
    Ok(vec![(
        "result",
        Json::obj(vec![
            ("ok", Json::Bool(c.ok)),
            ("reason", Json::str(c.reason)),
        ]),
    )])
}

fn grant_find(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let workspace = workspace_of(req)?;
    let found = session.broker.find(now, criteria_of(req, &workspace)?);
    Ok(vec![("grant", found.map_or(Json::Null, Grant::to_json))])
}

fn grant_require(session: &mut Session, now: u64, req: &Json) -> OpResult {
    let workspace = workspace_of(req)?;
    let g = session.broker.require(
        now,
        need_str(req, "grant_id")?,
        criteria_of(req, &workspace)?,
    )?;
    Ok(vec![("grant", g.to_json())])
}

fn revoke_reply(session: &Session, key: &'static str, value: Json) -> OpResult {
    Ok(vec![(key, value), ("persist", persisted(session))])
}

fn grant_list(session: &mut Session, now: u64) -> OpResult {
    let list = session
        .broker
        .list(now)
        .into_iter()
        .map(|g| {
            Json::obj(vec![
                ("id", Json::Str(g.id)),
                ("providerId", Json::Str(g.provider_id)),
                ("scope", Json::Str(g.scope)),
                ("once", Json::Bool(g.once)),
                ("expiresAt", Json::Num(g.expires_at as f64)),
            ])
        })
        .collect();
    Ok(vec![("grants", Json::Arr(list))])
}

fn grant_import(session: &mut Session, req: &Json) -> OpResult {
    let Some(Json::Arr(entries)) = req.get("grants") else {
        return Err(OpError::invalid("grants"));
    };
    if entries.len() > MAX_GRANT_ENTRIES {
        return Err(OpError::invalid("grants"));
    }
    let imported = session.broker.import(entries);
    Ok(vec![("imported", Json::int(imported as i64))])
}

// ---------------------------------------------------------------- budget

fn limit_of(req: &Json, key: &str) -> Result<Option<i64>, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() && n > 0.0 => Ok(Some(n as i64)),
            Some(n) if n.is_finite() => Ok(None), // 0 or negative = unlimited
            _ => Err(OpError::invalid(key)),
        },
    }
}

fn budget_configure(session: &mut Session, req: &Json) -> OpResult {
    let price = match req.get("price_per_1k_tokens") {
        None | Some(Json::Null) => 0.0,
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() && n >= 0.0 => n,
            _ => return Err(OpError::invalid("price_per_1k_tokens")),
        },
    };
    session.budget.configure(
        limit_of(req, "max_requests")?,
        limit_of(req, "max_tokens")?,
        price,
    );
    Ok(vec![])
}

fn budget_reserve(session: &mut Session, req: &Json) -> OpResult {
    let estimate = match req.get("estimate").and_then(Json::as_i64) {
        Some(n) if n >= 0 => n,
        _ => return Err(OpError::invalid("estimate")),
    };
    if session.tickets.len() >= MAX_TICKETS {
        return Err(OpError::coded(
            "AI_BUDGET_EXCEEDED",
            "too many unsettled reservations",
        ));
    }
    let ticket = session.budget.reserve(estimate)?;
    let id = session.ids.next("ticket", 8);
    session.tickets.insert(id.clone(), ticket);
    Ok(vec![("ticket", Json::Str(id))])
}

/// Settling or releasing a ticket that is unknown (already settled or released) is a no-op, as in the JS class.
fn budget_close(session: &mut Session, req: &Json, settle: bool) -> OpResult {
    let id = need_str(req, "ticket")?;
    let usage = match req.get("usage") {
        None | Some(Json::Null) => None,
        Some(u) => match (
            u.get("input").and_then(Json::as_i64),
            u.get("output").and_then(Json::as_i64),
        ) {
            (Some(input), Some(output)) if input >= 0 && output >= 0 => {
                Some(Usage { input, output })
            }
            _ => return Err(OpError::invalid("usage")),
        },
    };
    let Some(mut ticket) = session.tickets.remove(id) else {
        return Ok(vec![("settled", Json::Bool(false))]);
    };
    if settle {
        session.budget.settle(&mut ticket, usage);
    } else {
        session.budget.release(&mut ticket);
    }
    Ok(vec![("settled", Json::Bool(true))])
}

fn budget_snapshot(session: &Session) -> OpResult {
    let s = session.budget.snapshot();
    Ok(vec![(
        "budget",
        Json::obj(vec![
            ("requests", Json::int(s.requests)),
            ("tokens", Json::int(s.tokens)),
            ("estimated", Json::Bool(s.estimated)),
            ("cost", s.cost.map_or(Json::Null, Json::Num)),
        ]),
    )])
}

// ---------------------------------------------------------------- dispatch

fn run(session: &mut Session, now: u64, op: &str, req: &Json) -> OpResult {
    match op {
        "settings.apply" => apply_settings(session, req),
        "settings.resolve" => resolve_scenario(session, req),
        "ctx.snapshot" => ctx_snapshot(session, req),
        "ctx.excerpt" => ctx_excerpt(session, req),
        "ctx.validate" => ctx_validate(session, req),
        "ctx.release" => ctx_release(session, req),
        "edit.propose" => edit_propose(session, req),
        "edit.select" => edit_select(session, req),
        "edit.result" => edit_result(session, req),
        "edit.plan" => edit_plan(session, now, req),
        "edit.prepare" => edit_prepare(session, now, req),
        "edit.begin_apply" => edit_begin_apply(session, now, req),
        "edit.finish_apply" => edit_finish_apply(session, now, req),
        "edit.invalidate" => {
            let n = session.coordinator.invalidate(now, need_str(req, "uri")?);
            Ok(vec![("invalidated", Json::int(n as i64))])
        }
        "edit.discard" => {
            session.coordinator.discard(need_str(req, "plan")?);
            Ok(vec![])
        }
        "edit.purge" => {
            session.coordinator.purge(now);
            Ok(vec![])
        }
        "edit.status" => {
            let status = session.coordinator.status_of(now, need_str(req, "plan")?);
            Ok(vec![("status", Json::str(status))])
        }
        "grant.workspace_token" => {
            let uris = str_list(req.get("uris"), "uris")?;
            Ok(vec![(
                "workspace",
                Json::Str(grants::workspace_token(&uris)),
            )])
        }
        "grant.authorize" => grant_authorize(session, now, req),
        "grant.check" => grant_check(session, now, req),
        "grant.find" => grant_find(session, now, req),
        "grant.require" => grant_require(session, now, req),
        "grant.consume" => {
            session.broker.consume(need_str(req, "grant_id")?);
            Ok(vec![])
        }
        "grant.revoke" => {
            let revoked = session.broker.revoke(need_str(req, "grant_id")?);
            revoke_reply(session, "revoked", Json::Bool(revoked))
        }
        "grant.revoke_provider" => {
            let n = session
                .broker
                .revoke_provider(need_str(req, "provider_id")?);
            revoke_reply(session, "count", Json::int(n as i64))
        }
        "grant.revoke_provider_prefix" => {
            let n = session
                .broker
                .revoke_provider_prefix(need_str(req, "prefix")?);
            revoke_reply(session, "count", Json::int(n as i64))
        }
        "grant.revoke_all" => {
            let n = session.broker.revoke_all();
            revoke_reply(session, "count", Json::int(n as i64))
        }
        "grant.still_valid" => {
            let valid = session.broker.still_valid(now, need_str(req, "grant_id")?);
            Ok(vec![("valid", Json::Bool(valid))])
        }
        "grant.list" => grant_list(session, now),
        "grant.export" => Ok(vec![("grants", persisted(session))]),
        "grant.import" => grant_import(session, req),
        "budget.configure" => budget_configure(session, req),
        "budget.reserve" => budget_reserve(session, req),
        "budget.settle" => budget_close(session, req, true),
        "budget.release" => budget_close(session, req, false),
        "budget.snapshot" => budget_snapshot(session),
        "budget.reset" => {
            session.budget.reset();
            session.tickets.clear();
            Ok(vec![])
        }
        _ => Err(OpError::coded("UNKNOWN_OPERATION", op)),
    }
}

/// Handles one 0.3 operation. `op` is one of [`OPS`].
pub fn handle(store: &mut SessionStore, now: u64, op: &str, id: Option<i64>, req: &Json) -> Json {
    let outcome = if op == "session.open" {
        open_session(store, now, req)
    } else {
        let Some(sid) = req.get("session").and_then(Json::as_str) else {
            return OpError::coded("NO_SESSION", "no session").to_json(id);
        };
        if op == "session.close" {
            return if store.close(sid) {
                ok(id, vec![("closed", Json::Bool(true))])
            } else {
                OpError::coded("NO_SESSION", "no such session").to_json(id)
            };
        }
        match store.get(now, sid) {
            None => return OpError::coded("NO_SESSION", "no such session").to_json(id),
            Some(session) => run(session, now, op, req),
        }
    };
    match outcome {
        Ok(rest) => ok(id, rest),
        Err(e) => e.to_json(id),
    }
}
