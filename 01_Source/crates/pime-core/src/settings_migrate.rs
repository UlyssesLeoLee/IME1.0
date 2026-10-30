//! Settings migration, export/import and user rule pack loading: the pure part of `core/settings.js`
//! (`migrateV0`, `migrate`, `migrateStore`, `exportSettings`, `importSettings`, `buildRegistryWithUserRules`).
//! Settings never contain keys; keys live in the credential store and are referenced by provider id only.
//!
//! Documents are plain JSON in the shape the JavaScript engine uses (see `settings.rs`). Time is explicit (`now_ms`),
//! never read from the clock here. Files are not touched here either: `settings_io.rs` holds the atomic file part.
//!
//! Differences from the reference, all in input the editor's typed settings never produce: a non-string
//! `endpoint` in a v0.1 document is a conflict (the reference stringifies arrays and objects before parsing them as a
//! URL); `JSON.parse` accepts duplicate object keys while the strict parser of this crate rejects them (such a file is
//! "not valid JSON" here); `effective` reports a wrongly typed `providers`/`scenarios`/`budget` as an invalid
//! setting instead of spreading it.

use crate::error::{PimeError, Result, fail};
use crate::json::{self, Json, Limits};
use crate::jsval::{iso_date, js_order, to_js_string, to_pretty_string, truthy};
use crate::pack::{PackLimits, Registry};
use crate::secrets;
use crate::settings::{
    EFFORT_LEVELS, Obj, SCENARIOS, SCHEMA_VERSION, check_endpoint, default_providers,
    default_scenario, effective, spread, validate,
};

const MIGRATION_NOTE: &str =
    "Authorisations from v0.1 are not carried over; authorise again on first use.";

fn set(obj: &mut Obj, key: &str, value: Json) {
    match obj.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => obj.push((key.to_string(), value)),
    }
}

fn conflict(key: &str, reason: String) -> Json {
    Json::obj(vec![("key", Json::str(key)), ("reason", Json::Str(reason))])
}

/// v0.1 flat keys `{provider, endpoint, model, effort, effortCapability}` -> the v1 structure (FR-046).
/// Anything that is not an object reads as an empty document, like property reads on a primitive.
pub fn migrate_v0(old: &Json, now_ms: f64) -> Result<Json> {
    let mut conflicts = Vec::new();
    let mut providers = default_providers();
    let provider = if old.get("provider").and_then(Json::as_str) == Some("openai-compatible") {
        "openai-compatible"
    } else {
        "openai"
    };
    if let Some(endpoint) = old.get("endpoint")
        && truthy(endpoint)
    {
        match endpoint {
            Json::Str(e) if check_endpoint(e).is_ok() => {
                if let Some((_, Json::Obj(p))) = providers.iter_mut().find(|(k, _)| k == provider) {
                    set(p, "endpoint", Json::str(e));
                }
            }
            _ => conflicts.push(conflict(
                "endpoint",
                "not a valid HTTPS endpoint; the default was kept".to_string(),
            )),
        }
    }
    let capability = match old.get("effortCapability").and_then(Json::as_str) {
        Some("supported") => {
            let checked_at = iso_date(now_ms)
                .ok_or_else(|| PimeError::new("SETTINGS_INVALID", "the clock is out of range"))?;
            Json::obj(vec![
                ("state", Json::str("supported")),
                ("supported", Json::strs(&["low", "medium", "high"])),
                ("checkedAt", Json::Str(checked_at)),
            ])
        }
        Some("unsupported") => Json::obj(vec![("state", Json::str("unsupported"))]),
        _ => Json::obj(vec![("state", Json::str("unknown"))]),
    };
    let old_effort = old.get("effort");
    let effort = match old_effort.and_then(Json::as_str) {
        Some(level) if EFFORT_LEVELS.contains(&level) => level,
        _ => "auto",
    };
    if let Some(raw) = old_effort
        && truthy(raw)
        && effort == "auto"
        && raw.as_str() != Some("auto")
    {
        conflicts.push(conflict(
            "effort",
            format!("unknown level \"{}\" reset to auto", to_js_string(raw)),
        ));
    }
    let model = old.get("model").and_then(Json::as_str).unwrap_or("");
    let base: Obj = vec![
        ("provider".to_string(), Json::str(provider)),
        ("model".to_string(), Json::str(model)),
        ("effort".to_string(), Json::str(effort)),
        ("effortCapability".to_string(), capability),
    ];
    let scenarios: Obj = SCENARIOS
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                Json::Obj(spread(default_scenario(name), &base)),
            )
        })
        .collect();
    Ok(Json::obj(vec![
        ("schema_version", Json::int(SCHEMA_VERSION)),
        ("providers", Json::Obj(providers)),
        ("scenarios", Json::Obj(scenarios)),
        ("conflicts", Json::Arr(conflicts)),
        ("notes", Json::Arr(vec![Json::str(MIGRATION_NOTE)])),
    ]))
}

/// `migrate(doc, now)`: a v1 document passes through (with empty `conflicts`/`notes`), a v0.1 one is migrated, a newer
/// one is refused ("downgrade is not supported").
pub fn migrate(doc: &Json, now_ms: f64) -> Result<Json> {
    // `const v = doc && doc.schema_version`: a falsy document is its own "version"
    let version: Option<Json> = match doc {
        Json::Obj(_) => doc.get("schema_version").cloned(),
        other if !truthy(other) => Some(other.clone()),
        _ => None,
    };
    match version {
        Some(Json::Num(n)) if n == SCHEMA_VERSION as f64 => {
            let mut out = doc.as_object().cloned().unwrap_or_default();
            set(&mut out, "conflicts", Json::Arr(Vec::new()));
            set(&mut out, "notes", Json::Arr(Vec::new()));
            Ok(Json::Obj(out))
        }
        None => migrate_v0(doc, now_ms),
        Some(Json::Num(0.0)) => migrate_v0(doc, now_ms),
        Some(v) => fail(
            "SETTINGS_INVALID",
            format!(
                "settings schema_version {} is newer than this build ({SCHEMA_VERSION}); downgrade is not supported",
                to_js_string(&v)
            ),
        ),
    }
}

/// The document `migrateStore` writes: everything the stored document holds that this module does not own (user
/// rules, preferences) passes through untouched, the legacy flat keys are retired, the new structure is added.
pub fn store_document(backup: &Json, next: &Json) -> Json {
    const LEGACY: [&str; 5] = [
        "provider",
        "endpoint",
        "model",
        "effort",
        "effortCapability",
    ];
    let mut out: Obj = backup
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| !LEGACY.contains(&k.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for key in ["schema_version", "providers", "scenarios"] {
        match next.get(key) {
            Some(v) => set(&mut out, key, v.clone()),
            None => out.retain(|(k, _)| k != key),
        }
    }
    Json::Obj(js_order(out))
}

/// Where the settings live: a bridge setting, a file (`settings_io::FileStore`), a test double.
pub trait SettingsStore {
    /// The stored document (an empty object when there is none yet).
    fn read(&mut self) -> Json;
    /// Replaces the stored document; the error is the error code or message of the failure.
    fn write(&mut self, document: &Json) -> std::result::Result<(), String>;
}

pub struct StoreMigration {
    /// The `migrate` result (settings structure, `conflicts`, `notes`).
    pub settings: Json,
    /// What the store held before.
    pub backup: Json,
    pub conflicts: Json,
    pub notes: Json,
}

/// Migration through a store: the previous state is preserved if anything fails (UT-059 / UT-073).
pub fn migrate_store(store: &mut dyn SettingsStore, now_ms: f64) -> Result<StoreMigration> {
    let backup = store.read();
    let next = migrate(&backup, now_ms)?;
    let document = store_document(&backup, &next);
    if let Err(detail) = store.write(&document) {
        return fail(
            "SETTINGS_INVALID",
            format!("migration aborted, previous settings are intact ({detail})"),
        );
    }
    Ok(StoreMigration {
        conflicts: next
            .get("conflicts")
            .cloned()
            .unwrap_or(Json::Arr(Vec::new())),
        notes: next.get("notes").cloned().unwrap_or(Json::Arr(Vec::new())),
        settings: next,
        backup,
    })
}

/// `exportSettings`: the settings as pretty JSON, refused when anything in it looks like a secret.
pub fn export_settings(settings: &Json) -> Result<String> {
    let mut doc: Obj = vec![("schema_version".to_string(), Json::int(SCHEMA_VERSION))];
    for key in ["providers", "scenarios", "budget"] {
        if let Some(v) = settings.get(key) {
            doc.push((key.to_string(), v.clone()));
        }
    }
    let text = to_pretty_string(&Json::Obj(doc));
    if secrets::scan(&text, "").blocked {
        return fail(
            "SETTINGS_INVALID",
            "export refused: it would contain secret-like data",
        );
    }
    Ok(text)
}

pub struct Imported {
    pub settings: Json,
    pub conflicts: Json,
    pub notes: Json,
}

/// `importSettings`: parse, migrate, merge with the defaults and validate; nothing is applied on failure.
pub fn import_settings(text: &str, now_ms: f64) -> Result<Imported> {
    let Ok(doc) = json::parse(text, Limits::DEFAULT) else {
        return fail("SETTINGS_INVALID", "settings file is not valid JSON");
    };
    let migrated = migrate(&doc, now_ms)?;
    let settings = match effective(&migrated) {
        Ok(s) => s,
        Err(field) => return fail("SETTINGS_INVALID", format!("invalid settings: {field}")),
    };
    let issues = validate(&settings);
    if !issues.is_empty() {
        let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        return fail(
            "SETTINGS_INVALID",
            format!("invalid settings: {}", paths.join(", ")),
        );
    }
    Ok(Imported {
        conflicts: migrated
            .get("conflicts")
            .cloned()
            .unwrap_or(Json::Arr(Vec::new())),
        notes: migrated
            .get("notes")
            .cloned()
            .unwrap_or(Json::Arr(Vec::new())),
        settings,
    })
}

pub struct RejectedPack {
    pub name: String,
    pub code: String,
    pub message: String,
}

pub struct UserRules {
    /// The built-in registry plus every accepted user pack; a failure never degrades the rules in use.
    pub registry: Registry,
    pub accepted: Vec<String>,
    pub rejected: Vec<RejectedPack>,
}

/// User rule packs (FR-045 / UT-058): validated as data in a fresh registry; a bad pack is reported and skipped.
/// `files` are `(name, text)`.
pub fn build_registry_with_user_rules(files: &[(String, String)]) -> Result<UserRules> {
    let mut registry = Registry::builtin()?;
    let (mut accepted, mut rejected) = (Vec::new(), Vec::new());
    for (name, text) in files {
        let outcome = if text.len() > PackLimits::PACK_BYTES {
            Err(PimeError::new(
                "RULEPACK_INVALID",
                "pack exceeds size limit",
            ))
        } else {
            match json::parse(
                text,
                Limits {
                    max_bytes: PackLimits::PACK_BYTES,
                    max_depth: 16,
                },
            ) {
                Ok(pack) => registry.add_user(&pack, text.len()),
                Err(_) => Err(PimeError::new("RULEPACK_INVALID", "not valid JSON")),
            }
        };
        match outcome {
            Ok(()) => accepted.push(name.clone()),
            Err(e) => rejected.push(RejectedPack {
                name: name.clone(),
                code: e.code.to_string(),
                message: e.message,
            }),
        }
    }
    Ok(UserRules {
        registry,
        accepted,
        rejected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(s: &str) -> Json {
        json::parse(s, Limits::DEFAULT).unwrap()
    }

    const NOW: f64 = 1_793_361_600_000.0; // 2026-10-30T12:00:00.000Z

    struct Memory {
        stored: Json,
        fail: Option<&'static str>,
        writes: usize,
    }

    impl SettingsStore for Memory {
        fn read(&mut self) -> Json {
            self.stored.clone()
        }
        fn write(&mut self, document: &Json) -> std::result::Result<(), String> {
            if let Some(why) = self.fail {
                return Err(why.to_string());
            }
            self.stored = document.clone();
            self.writes += 1;
            Ok(())
        }
    }

    #[test]
    fn ut_059_migration_keeps_the_previous_settings_when_the_write_fails() {
        let v0 = j(
            r#"{"provider":"openai-compatible","endpoint":"https://api.example.com/v1/chat/completions","model":"m1","effort":"high","effortCapability":"supported","userRules":[{"id":"mine"}],"theme":"x"}"#,
        );
        let m = migrate(&v0, NOW).unwrap();
        assert_eq!(
            m.get("scenarios")
                .unwrap()
                .get("f1Full")
                .unwrap()
                .get("provider")
                .and_then(Json::as_str),
            Some("openai-compatible")
        );
        assert_eq!(
            m.get("scenarios")
                .unwrap()
                .get("f1Insert")
                .unwrap()
                .get("effortCapability")
                .unwrap()
                .to_json_string(),
            r#"{"state":"supported","supported":["low","medium","high"],"checkedAt":"2026-10-30T12:00:00.000Z"}"#
        );
        let mut store = Memory {
            stored: v0.clone(),
            fail: Some("ENOSPC"),
            writes: 0,
        };
        let e = migrate_store(&mut store, NOW).err().unwrap();
        assert_eq!(e.code, "SETTINGS_INVALID");
        assert!(e.message.contains("previous settings are intact (ENOSPC)"));
        assert_eq!(
            store.stored, v0,
            "the old configuration is complete after the failed migration"
        );
        store.fail = None;
        let done = migrate_store(&mut store, NOW).unwrap();
        assert_eq!(store.writes, 1);
        assert_eq!(
            store.stored.get("schema_version").and_then(Json::as_i64),
            Some(1)
        );
        assert_eq!(store.stored.get("theme").and_then(Json::as_str), Some("x"));
        assert_eq!(
            store.stored.get("userRules").unwrap().to_json_string(),
            r#"[{"id":"mine"}]"#
        );
        assert!(
            store.stored.get("model").is_none(),
            "legacy flat keys are retired"
        );
        assert_eq!(done.backup.get("model").and_then(Json::as_str), Some("m1"));
        // an already migrated store is stable
        let before = store.stored.clone();
        migrate_store(&mut store, NOW).unwrap();
        assert_eq!(store.stored, before);
    }

    #[test]
    fn conflicts_are_reported_and_a_newer_schema_is_refused() {
        let m = migrate_v0(&j(r#"{"endpoint":"http://x","effort":"turbo"}"#), NOW).unwrap();
        let keys: Vec<&str> = m
            .get("conflicts")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .map(|c| c.get("key").and_then(Json::as_str).unwrap())
            .collect();
        assert_eq!(keys, ["endpoint", "effort"]);
        let e = migrate(&j(r#"{"schema_version":9}"#), NOW).err().unwrap();
        assert_eq!(e.code, "SETTINGS_INVALID");
        assert!(
            e.message
                .contains("schema_version 9 is newer than this build (1)")
        );
        // the effort level `max` is a level, not a conflict
        let max = migrate_v0(&j(r#"{"effort":"max"}"#), NOW).unwrap();
        assert_eq!(max.get("conflicts").unwrap().to_json_string(), "[]");
        assert_eq!(
            max.get("scenarios")
                .unwrap()
                .get("realtime")
                .unwrap()
                .get("effort")
                .and_then(Json::as_str),
            Some("max")
        );
        // an unusable clock only matters when a timestamp has to be written
        assert!(migrate_v0(&j(r#"{"effortCapability":"supported"}"#), f64::NAN).is_err());
        assert!(migrate_v0(&j("{}"), f64::NAN).is_ok());
    }

    #[test]
    fn a_stored_document_that_is_not_an_object_reads_as_a_fresh_install() {
        for text in ["[]", "5", "1.5", "\"s\"", "true"] {
            let m = migrate(&j(text), NOW).unwrap();
            assert_eq!(
                m.get("schema_version").and_then(Json::as_i64),
                Some(1),
                "{text}"
            );
        }
        for text in ["null", "false", "\"\""] {
            assert!(migrate(&j(text), NOW).is_err(), "{text}");
        }
        assert_eq!(
            migrate(&j("0"), NOW)
                .unwrap()
                .get("schema_version")
                .and_then(Json::as_i64),
            Some(1)
        );
    }

    #[test]
    fn export_refuses_secrets_and_import_validates_everything() {
        let defaults = crate::settings::defaults();
        let text = export_settings(&defaults).unwrap();
        assert!(text.starts_with("{\n  \"schema_version\": 1,\n  \"providers\": {"));
        let back = import_settings(&text, NOW).unwrap();
        assert_eq!(back.settings, defaults);
        assert_eq!(back.notes.to_json_string(), "[]");
        let leaky = j(
            r#"{"providers":{"p":{"api":"openai-chat","endpoint":"https://p.example/v1","note":"sk-proj-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH"}}}"#,
        );
        let e = export_settings(&effective(&leaky).unwrap()).err().unwrap();
        assert_eq!(e.code, "SETTINGS_INVALID");
        assert!(e.message.starts_with("export refused"));
        assert!(!e.message.contains("sk-proj"));
        for (bad, why) in [
            ("{bad", "not valid JSON"),
            ("", "not valid JSON"),
            ("{\"a\":1,\"a\":2}", "not valid JSON"),
            ("{\"schema_version\":3}", "newer than this build"),
            (
                r#"{"schema_version":1,"providers":{"X":{"api":"openai-chat","endpoint":"http://x"}}}"#,
                "providers.X.endpoint",
            ),
            (
                r#"{"schema_version":1,"scenarios":{"realtime":{"effort":"turbo","timeoutMs":1}}}"#,
                "scenarios.realtime.effort, scenarios.realtime.timeoutMs",
            ),
            (r#"{"schema_version":1,"providers":[]}"#, "providers"),
        ] {
            let e = import_settings(bad, NOW)
                .err()
                .unwrap_or_else(|| panic!("{bad}"));
            assert_eq!(e.code, "SETTINGS_INVALID");
            assert!(e.message.contains(why), "{bad}: {}", e.message);
        }
        let ok = import_settings(
            r#"{"schema_version":1,"scenarios":{"realtime":{"effort":"max"}}}"#,
            NOW,
        )
        .unwrap();
        assert_eq!(
            ok.settings
                .get("scenarios")
                .unwrap()
                .get("realtime")
                .unwrap()
                .get("effort")
                .and_then(Json::as_str),
            Some("max")
        );
    }

    #[test]
    fn ut_058_bad_user_packs_are_rejected_and_skipped_valid_ones_are_kept() {
        let good = r#"{"schema_version":1,"profile_id":"python","version":"1.0.0","triggers":[{"id":"my.log","pattern":"logd","candidates":[{"label":"debug log","text":"logging.debug(\"${msg}\")"}]}]}"#;
        let evil = good.replace(r#""text":"#, r#""shell":"calc.exe","text":"#);
        let files = vec![
            ("good.json".to_string(), good.to_string()),
            ("evil.json".to_string(), evil),
            (
                "huge.json".to_string(),
                format!("{{\"pad\":\"{}\"}}", "x".repeat(300_000)),
            ),
            ("bad.json".to_string(), "{bad".to_string()),
            ("null.json".to_string(), "null".to_string()),
            ("again.json".to_string(), good.to_string()),
        ];
        let built = build_registry_with_user_rules(&files).unwrap();
        assert_eq!(built.accepted, ["good.json"]);
        let rejected: Vec<(&str, &str, &str)> = built
            .rejected
            .iter()
            .map(|r| (r.name.as_str(), r.code.as_str(), r.message.as_str()))
            .collect();
        assert_eq!(
            rejected[1],
            ("huge.json", "RULEPACK_INVALID", "pack exceeds size limit")
        );
        assert_eq!(
            rejected[2],
            ("bad.json", "RULEPACK_INVALID", "not valid JSON")
        );
        assert_eq!(
            rejected[3],
            ("null.json", "RULEPACK_INVALID", "pack must be an object")
        );
        assert!(rejected[0].2.contains("unknown field \"shell\""));
        assert!(
            rejected[4]
                .2
                .contains("collides with an existing rule: my.log")
        );
        assert!(
            built
                .registry
                .triggers("python")
                .iter()
                .any(|t| t.id == "my.log"),
            "the accepted pack is active"
        );
        // the built-in registry itself is untouched by a failing set
        let none =
            build_registry_with_user_rules(&[("x.json".to_string(), "[]".to_string())]).unwrap();
        assert!(
            none.registry
                .triggers("python")
                .iter()
                .all(|t| t.id != "my.log")
        );
    }
}
