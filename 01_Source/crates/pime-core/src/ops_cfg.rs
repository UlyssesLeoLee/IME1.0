//! Protocol 0.3 operations of migration step 1.3 (P1B): settings checks, migration, export/import and user rule
//! packs (`settings.*`), diagnostics (`diag.*`) and interface texts and UI models (`ui.*`). They follow the
//! conventions of `ops.rs`: every operation names an open `session`; new top-level request fields are snake_case;
//! nested objects that mirror the bridge's own shapes keep their camelCase names.
//!
//! None of these operations touches a file: the atomic file write and the `.bak` recovery (`settings_io.rs`) are a
//! library facility, deliberately not reachable from the protocol, because a request that names a path would let any
//! peer of the pipe write files.

use crate::diag::{DEFAULT_MAX, Diagnostics};
use crate::i18n;
use crate::json::Json;
use crate::ops::{OpError, OpResult, need_str};
use crate::session::Session;
use crate::settings::{self, Issue};
use crate::settings_migrate as migration;
use crate::uimodel::{self, Environment, Rect, Status};

/// Operations of step 1.3 (the older 0.3 operations are in `ops::OPS` and `ops_ai::OPS`).
pub const OPS: &[&str] = &[
    "settings.validate",
    "settings.migrate",
    "settings.export",
    "settings.import",
    "settings.rules_check",
    "diag.record",
    "diag.summary",
    "diag.text",
    "ui.resolve_locale",
    "ui.text",
    "ui.error_text",
    "ui.bundle",
    "ui.errors",
    "ui.candidate_rows",
    "ui.place_overlay",
    "ui.check_env",
    "ui.status_model",
    "ui.menu_model",
];

/// Operations that carry a settings file or rule packs and may use a long request line.
pub const BULK_OPS: &[&str] = &["settings.import", "settings.rules_check"];

const MAX_RULE_FILES: usize = 256;
const MAX_CANDIDATES: usize = 1000;
const MAX_MONITORS: usize = 64;
const MAX_ARGS: usize = 64;
const MAX_KEYS: usize = 4096;

// ---------------------------------------------------------------- request parsing

fn opt_str<'a>(req: &'a Json, key: &str) -> Result<Option<&'a str>, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s)),
        Some(_) => Err(OpError::invalid(key)),
    }
}

fn need_bool(req: &Json, key: &str) -> Result<bool, OpError> {
    match req.get(key) {
        Some(Json::Bool(b)) => Ok(*b),
        _ => Err(OpError::invalid(key)),
    }
}

fn opt_bool(req: &Json, key: &str) -> Result<bool, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(false),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(OpError::invalid(key)),
    }
}

/// The locale of a request: the named one, else the session's.
fn locale_of<'a>(session: &'a Session, req: &'a Json) -> Result<&'a str, OpError> {
    Ok(opt_str(req, "locale")?.unwrap_or(&session.locale))
}

fn num(v: Option<&Json>, field: &str) -> Result<f64, OpError> {
    match v {
        Some(Json::Num(n)) if n.is_finite() => Ok(*n),
        _ => Err(OpError::invalid(field)),
    }
}

fn point(v: Option<&Json>, field: &str, a: &str, b: &str) -> Result<(f64, f64), OpError> {
    let o = v.ok_or_else(|| OpError::invalid(field))?;
    Ok((
        num(o.get(a), &format!("{field}.{a}"))?,
        num(o.get(b), &format!("{field}.{b}"))?,
    ))
}

fn issues_json(issues: &[Issue]) -> Json {
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

fn dict(pairs: Vec<(String, String)>) -> Json {
    Json::Obj(pairs.into_iter().map(|(k, v)| (k, Json::Str(v))).collect())
}

// ---------------------------------------------------------------- settings.*

/// The settings of the request, or the session's current ones.
fn settings_of<'a>(session: &'a Session, req: &'a Json) -> Result<&'a Json, OpError> {
    match req.get("settings") {
        None | Some(Json::Null) => Ok(&session.settings),
        Some(o @ Json::Obj(_)) => Ok(o),
        Some(_) => Err(OpError::invalid("settings")),
    }
}

fn settings_migrate(now: u64, req: &Json) -> OpResult {
    let doc = match req.get("doc") {
        Some(d @ Json::Obj(_)) => d,
        _ => return Err(OpError::invalid("doc")),
    };
    let result = migration::migrate(doc, now as f64)?;
    let document = migration::store_document(doc, &result);
    Ok(vec![("result", result), ("document", document)])
}

fn settings_import(now: u64, req: &Json) -> OpResult {
    let imported = migration::import_settings(need_str(req, "text")?, now as f64)?;
    Ok(vec![
        ("settings", imported.settings),
        ("conflicts", imported.conflicts),
        ("notes", imported.notes),
    ])
}

fn settings_rules_check(req: &Json) -> OpResult {
    let Some(Json::Arr(list)) = req.get("files") else {
        return Err(OpError::invalid("files"));
    };
    if list.len() > MAX_RULE_FILES {
        return Err(OpError::invalid("files"));
    }
    let mut files = Vec::with_capacity(list.len());
    for (i, f) in list.iter().enumerate() {
        let (Some(name), Some(text)) = (
            f.get("name").and_then(Json::as_str),
            f.get("text").and_then(Json::as_str),
        ) else {
            return Err(OpError::invalid(&format!("files[{i}]")));
        };
        files.push((name.to_string(), text.to_string()));
    }
    let built = migration::build_registry_with_user_rules(&files)?;
    Ok(vec![
        ("accepted", Json::strs(&built.accepted)),
        (
            "rejected",
            Json::Arr(
                built
                    .rejected
                    .iter()
                    .map(|r| {
                        Json::obj(vec![
                            ("name", Json::str(&r.name)),
                            ("code", Json::str(&r.code)),
                            ("message", Json::str(&r.message)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

// ---------------------------------------------------------------- diag.*

/// The session's diagnostics ring, created on first use (its anonymised id is derived from the session's entropy).
fn diagnostics(session: &mut Session) -> &mut Diagnostics {
    if session.diag.is_none() {
        let seed = session.ids.next("diag", 16);
        session.diag = Some(Diagnostics::new(&seed, DEFAULT_MAX));
    }
    session.diag.as_mut().expect("created above")
}

// ---------------------------------------------------------------- ui.*

fn candidate_rows(session: &Session, req: &Json) -> OpResult {
    let locale = locale_of(session, req)?;
    let Some(Json::Arr(list)) = req.get("candidates") else {
        return Err(OpError::invalid("candidates"));
    };
    if list.len() > MAX_CANDIDATES {
        return Err(OpError::invalid("candidates"));
    }
    for (i, c) in list.iter().enumerate() {
        let bad = |field: &str| OpError::invalid(&format!("candidates[{i}].{field}"));
        if !matches!(c.get("label"), Some(Json::Str(_))) {
            return Err(bad("label"));
        }
        if !matches!(
            c.get("assumptions"),
            None | Some(Json::Null) | Some(Json::Arr(_))
        ) {
            return Err(bad("assumptions"));
        }
        if !matches!(
            c.get("sources"),
            None | Some(Json::Null) | Some(Json::Arr(_))
        ) {
            return Err(bad("sources"));
        }
        for scalar in ["source", "validation", "explain"] {
            if matches!(c.get(scalar), Some(Json::Arr(_) | Json::Obj(_))) {
                return Err(bad(scalar));
            }
        }
    }
    let total = list.len() as f64;
    let rows = list
        .iter()
        .enumerate()
        .map(|(i, c)| uimodel::candidate_row(locale, c, (i + 1) as f64, total))
        .collect();
    Ok(vec![("rows", Json::Arr(rows))])
}

fn rect_of(v: &Json, field: &str) -> Result<Rect, OpError> {
    let (x, y) = point(Some(v), field, "x", "y")?;
    let (w, h) = point(Some(v), field, "w", "h")?;
    Ok(Rect { x, y, w, h })
}

fn place_overlay(req: &Json) -> OpResult {
    let caret = point(req.get("caret"), "caret", "x", "y")?;
    let size = point(req.get("size"), "size", "w", "h")?;
    let Some(Json::Arr(list)) = req.get("monitors") else {
        return Err(OpError::invalid("monitors"));
    };
    if list.len() > MAX_MONITORS {
        return Err(OpError::invalid("monitors"));
    }
    let monitors = list
        .iter()
        .enumerate()
        .map(|(i, m)| rect_of(m, &format!("monitors[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let dpi = match req.get("dpi") {
        None | Some(Json::Null) => 100.0,
        Some(_) => num(req.get("dpi"), "dpi")?,
    };
    let placement = uimodel::place_overlay(caret, size, &monitors, dpi).map_or(Json::Null, |r| {
        Json::obj(vec![
            ("x", Json::Num(r.x)),
            ("y", Json::Num(r.y)),
            ("w", Json::Num(r.w)),
            ("h", Json::Num(r.h)),
        ])
    });
    Ok(vec![("placement", placement)])
}

fn check_env(req: &Json) -> OpResult {
    let (supported, reasons) = uimodel::check_environment(&Environment {
        platform: req.get("platform"),
        arch: req.get("arch"),
        build: req.get("build"),
        dpi: req.get("dpi"),
    });
    Ok(vec![
        ("supported", Json::Bool(supported)),
        ("reasons", Json::strs(&reasons)),
    ])
}

/// An optional object field (`null` and absent are `None`); `strings` names the members that must be strings if present.
fn opt_object<'a>(req: &'a Json, key: &str, strings: &[&str]) -> Result<Option<&'a Json>, OpError> {
    match req.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(o @ Json::Obj(_)) => {
            for member in strings {
                if o.get(member).is_some_and(|v| !matches!(v, Json::Str(_))) {
                    return Err(OpError::invalid(&format!("{key}.{member}")));
                }
            }
            Ok(Some(o))
        }
        Some(_) => Err(OpError::invalid(key)),
    }
}

fn status_model(session: &Session, req: &Json) -> OpResult {
    let info = opt_object(req, "info", &["language"])?;
    if info.is_some_and(|i| i.get("language").is_none()) {
        return Err(OpError::invalid("info.language"));
    }
    let scenario = opt_object(req, "scenario", &["model", "effort"])?;
    let budget = match req.get("budget") {
        Some(b @ Json::Obj(_)) => {
            num(b.get("requests"), "budget.requests")?;
            num(b.get("tokens"), "budget.tokens")?;
            b
        }
        _ => return Err(OpError::invalid("budget")),
    };
    let model = uimodel::status_model(
        locale_of(session, req)?,
        &Status {
            core_down: need_bool(req, "core_down")?,
            reason: opt_str(req, "reason")?.unwrap_or(""),
            enabled: need_bool(req, "enabled")?,
            info,
            session_language: need_str(req, "session_language")?,
            paused: need_bool(req, "paused")?,
            ai_enabled: need_bool(req, "ai_enabled")?,
            scenario,
            budget,
        },
    );
    Ok(vec![("model", model)])
}

fn ui_text(session: &Session, req: &Json) -> OpResult {
    let locale = locale_of(session, req)?;
    let args = match req.get("args") {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Arr(a)) if a.len() <= MAX_ARGS => a.iter().map(i18n::arg_text).collect(),
        Some(_) => return Err(OpError::invalid("args")),
    };
    Ok(vec![(
        "text",
        Json::Str(i18n::text(locale, need_str(req, "key")?, &args)),
    )])
}

fn ui_bundle(session: &Session, req: &Json) -> OpResult {
    let locale = locale_of(session, req)?;
    let keys: Vec<String> = match req.get("keys") {
        None | Some(Json::Null) => i18n::keys().map(str::to_string).collect(),
        Some(Json::Arr(a)) if a.len() <= MAX_KEYS => a
            .iter()
            .map(|k| {
                k.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| OpError::invalid("keys"))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(OpError::invalid("keys")),
    };
    Ok(vec![
        ("locale", Json::str(locale)),
        (
            "bundle",
            dict(i18n::bundle(locale, &keys, opt_bool(req, "raw")?)),
        ),
    ])
}

/// Handles one operation of this module. `now` is the clock of the core in epoch milliseconds.
pub fn run(session: &mut Session, now: u64, op: &str, req: &Json) -> OpResult {
    match op {
        "settings.validate" => {
            let issues = settings::validate(settings_of(session, req)?);
            Ok(vec![("issues", issues_json(&issues))])
        }
        "settings.migrate" => settings_migrate(now, req),
        "settings.export" => {
            let text = migration::export_settings(settings_of(session, req)?)?;
            Ok(vec![("text", Json::Str(text))])
        }
        "settings.import" => settings_import(now, req),
        "settings.rules_check" => settings_rules_check(req),
        "diag.record" => {
            let event = match req.get("event") {
                Some(e @ Json::Obj(_)) => e,
                _ => return Err(OpError::invalid("event")),
            };
            let clean = diagnostics(session).record(event, now as f64);
            Ok(vec![("event", clean)])
        }
        "diag.summary" => Ok(vec![("summary", diagnostics(session).summary())]),
        "diag.text" => Ok(vec![("text", Json::Str(diagnostics(session).to_text()))]),
        "ui.resolve_locale" => Ok(vec![(
            "locale",
            Json::str(i18n::resolve_locale_of(
                req.get("setting"),
                req.get("language"),
            )),
        )]),
        "ui.text" => ui_text(session, req),
        "ui.error_text" => {
            let locale = locale_of(session, req)?;
            let fallback = opt_str(req, "message")?.unwrap_or("");
            Ok(vec![(
                "text",
                Json::Str(i18n::describe_error(
                    locale,
                    need_str(req, "code")?,
                    fallback,
                )),
            )])
        }
        "ui.bundle" => ui_bundle(session, req),
        "ui.errors" => {
            let locale = locale_of(session, req)?;
            Ok(vec![
                ("locale", Json::str(locale)),
                ("errors", dict(i18n::error_table(locale))),
            ])
        }
        "ui.candidate_rows" => candidate_rows(session, req),
        "ui.place_overlay" => place_overlay(req),
        "ui.check_env" => check_env(req),
        "ui.status_model" => status_model(session, req),
        "ui.menu_model" => Ok(vec![(
            "model",
            uimodel::menu_model(
                locale_of(session, req)?,
                need_bool(req, "paused")?,
                need_bool(req, "ai_enabled")?,
            ),
        )]),
        _ => Err(OpError::coded("UNKNOWN_OPERATION", op)),
    }
}
