//! Plugin languages: a plugin pack adds a whole new language profile that the user switches on and off.
//!
//! The three shipped packs (Cypher, PL/SQL, T-SQL) are compiled into the core like the built-in packs; the
//! enabled set is a runtime property of the engine. T-SQL shares the `.sql` extension with PL/SQL, so it is
//! off by default.

use pime_core::json::{Json, Limits, parse};
use pime_core::pack::{
    DEFAULT_PLUGIN_LANGUAGES, LANGUAGES, PLUGIN_LANGUAGES, Pack, PackKind, Registry,
    is_plugin_profile, parse_pack,
};
use pime_core::predict::{Engine, PredictOptions};
use pime_core::protocol::handle_line;
use pime_core::resolve::{ResolveContext, resolve_language};
use pime_core::validators::{Dialect, validate_source};

fn ids(engine: &Engine, profile: &str, prefix: &str) -> Vec<String> {
    engine
        .predict(&PredictOptions::new(profile, prefix))
        .expect("predict")
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// An engine with its own registry (the tests never share the process-wide engine).
fn engine_with(ids: &[&str]) -> Engine {
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    Engine::new(Registry::builtin_with_plugins(&ids).expect("the shipped packs are valid"))
}

fn ctx<'a>(language_id: &'a str, file_name: &'a str) -> ResolveContext<'a> {
    ResolveContext {
        language_id: Some(language_id),
        file_name: Some(file_name),
        text: None,
    }
}

/// One pack document.
fn pack_doc(s: &str) -> Json {
    parse(s, Limits::DEFAULT).expect("valid JSON")
}

fn codes(profile: &str, text: &str, dialect: &[(&str, &str)]) -> Vec<&'static str> {
    let d: Dialect = dialect
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    validate_source(profile, text, &d)
        .issues
        .iter()
        .map(|i| i.code)
        .collect()
}

#[test]
fn the_three_shipped_plugin_packs_load_and_predict() {
    let engine = engine_with(&["cypher", "plsql", "tsql"]);
    let reg = engine.registry();
    assert_eq!(reg.available_plugins(), PLUGIN_LANGUAGES.to_vec());
    assert_eq!(reg.enabled_plugins(), PLUGIN_LANGUAGES.to_vec());
    for id in PLUGIN_LANGUAGES {
        let pack: &Pack = reg.get(id).expect("the pack is loaded");
        assert_eq!(pack.profile_id, id);
        assert_eq!(pack.validator, "sql", "{id} asks for the SQL family");
        assert!(!pack.triggers.is_empty(), "{id} has triggers");
        assert!(!pack.keywords.is_empty(), "{id} has keywords");
    }
    // a plugin profile predicts like a built-in one
    assert!(
        ids(&engine, "cypher", "match")
            .iter()
            .any(|i| i.starts_with("cypher.")),
        "{:?}",
        ids(&engine, "cypher", "match")
    );
    assert!(
        ids(&engine, "plsql", "select")
            .iter()
            .any(|i| i.starts_with("plsql.")),
        "{:?}",
        ids(&engine, "plsql", "select")
    );
    assert!(
        ids(&engine, "tsql", "select")
            .iter()
            .any(|i| i.starts_with("tsql.")),
        "{:?}",
        ids(&engine, "tsql", "select")
    );
    // the candidate of a plugin profile is valid text, never a crash
    let c = engine
        .predict(&PredictOptions::new("cypher", "match"))
        .expect("predict")
        .into_iter()
        .next()
        .expect("a candidate");
    assert!(c.text.is_some_and(|t| t.starts_with("MATCH")));
}

#[test]
fn the_default_plugin_set_is_cypher_and_plsql() {
    let engine = engine_with(&[]);
    let reg = engine.registry();
    // the shipped packs are known even when they are off
    assert_eq!(reg.available_plugins(), PLUGIN_LANGUAGES.to_vec());
    assert_eq!(reg.enabled_plugins(), Vec::<String>::new());
    assert!(
        reg.has_profile("rust"),
        "the built-in profiles are always active"
    );
    assert!(!reg.has_profile("cypher"), "an off plugin is not active");
    assert!(
        reg.get("cypher").is_none(),
        "an off plugin contributes nothing"
    );
    assert!(reg.triggers("cypher").is_empty());
    assert!(reg.keywords("cypher").is_empty());
    assert!(reg.language_for_id("cypher").is_none());
    // the default set of the product
    let engine = engine_with(&DEFAULT_PLUGIN_LANGUAGES);
    assert_eq!(
        engine.registry().enabled_plugins(),
        vec!["cypher".to_string(), "plsql".to_string()]
    );
    assert!(engine.registry().has_profile("cypher"));
    assert!(engine.registry().has_profile("plsql"));
    assert!(
        !engine.registry().has_profile("tsql"),
        "T-SQL is off by default"
    );
}

#[test]
fn a_disabled_plugin_profile_predicts_nothing_and_is_not_resolvable() {
    let engine = engine_with(&["cypher", "plsql"]);
    assert!(ids(&engine, "tsql", "select").is_empty(), "no candidates");
    let reg = engine.registry();
    let d = resolve_language(&reg, &ctx("plaintext", "x.tsql"), "unknown", None);
    assert_ne!(d.language, "tsql", "the extension does not resolve to it");
    // a manual lock of a disabled profile is rejected
    let d = resolve_language(&reg, &ctx("plaintext", "x.txt"), "unknown", Some("tsql"));
    assert!(!d.locked, "a disabled profile cannot be locked");
    assert_eq!(d.reason, "kept");
}

#[test]
fn switching_the_plugin_set_changes_what_predicts() {
    let engine = engine_with(&["cypher"]);
    assert!(ids(&engine, "tsql", "select").is_empty(), "tsql is off");
    engine.set_plugin_languages(&["plsql".to_string()]);
    assert_eq!(engine.plugin_languages(), vec!["plsql".to_string()]);
    assert!(ids(&engine, "tsql", "select").is_empty(), "tsql is off now");
    assert!(
        ids(&engine, "cypher", "match").is_empty(),
        "cypher is off now"
    );
    assert!(
        !ids(&engine, "plsql", "select").is_empty(),
        "plsql is on now"
    );
    // unknown ids are ignored, the known ones stay
    engine.set_plugin_languages(&["cobol".to_string(), "plsql".to_string()]);
    assert_eq!(engine.plugin_languages(), vec!["plsql".to_string()]);
}

#[test]
fn the_sql_extension_is_ambiguous_when_two_plugins_own_it() {
    let engine = engine_with(&["cypher", "plsql"]);
    let reg = engine.registry();
    assert_eq!(reg.extension_owners(".sql"), vec!["plsql"]);
    let d = resolve_language(&reg, &ctx("plaintext", "x.sql"), "unknown", None);
    assert_eq!(
        (d.language.as_str(), d.reason, d.confidence, d.uncertain),
        ("plsql", "extension", 85, false)
    );
    // T-SQL owns `.sql` as well: the extension becomes ambiguous and the language is kept
    let engine = engine_with(&["cypher", "plsql", "tsql"]);
    let reg = engine.registry();
    assert_eq!(reg.extension_owners(".sql"), vec!["plsql", "tsql"]);
    let d = resolve_language(&reg, &ctx("plaintext", "x.sql"), "rust", None);
    assert_eq!(
        (
            d.language.as_str(),
            d.reason,
            d.uncertain,
            d.suggestion.as_deref()
        ),
        ("rust", "ambiguous-extension", true, Some("plsql"))
    );
    // only T-SQL: it owns `.sql` alone
    let engine = engine_with(&["tsql"]);
    let reg = engine.registry();
    let d = resolve_language(&reg, &ctx("plaintext", "x.sql"), "unknown", None);
    assert_eq!(
        (d.language.as_str(), d.reason, d.confidence),
        ("tsql", "extension", 85)
    );
}

#[test]
fn the_plugin_extensions_resolve_to_their_profile() {
    let engine = engine_with(&["cypher", "plsql", "tsql"]);
    let reg = engine.registry();
    for (file, want) in [
        ("x.cypher", "cypher"),
        ("x.cql", "cypher"),
        ("x.pls", "plsql"),
        ("x.plsql", "plsql"),
        ("x.pkb", "plsql"),
        ("x.pks", "plsql"),
        ("x.tsql", "tsql"),
    ] {
        let d = resolve_language(&reg, &ctx("plaintext", file), "unknown", None);
        assert_eq!(d.language.as_str(), want, "{file}");
        assert_eq!(d.reason, "extension", "{file}");
    }
    // the language id of the bridge wins over the extension
    let d = resolve_language(&reg, &ctx("plsql", "x.cypher"), "unknown", None);
    assert_eq!(d.language.as_str(), "plsql");
    assert_eq!(d.reason, "bridge-languageId");
}

#[test]
fn a_built_in_profile_behaves_exactly_as_before() {
    let engine = engine_with(&["cypher", "plsql", "tsql"]);
    assert_eq!(ids(&engine, "rust", "fnm"), vec!["rust.main#0"]);
    assert!(ids(&engine, "rust", "").is_empty());
    assert!(ids(&engine, "unknown", "fn").is_empty());
    let reg = engine.registry();
    // the hardcoded tables win over the registry
    let d = resolve_language(&reg, &ctx("plaintext", "x.rs"), "unknown", None);
    assert_eq!(
        (d.language.as_str(), d.reason, d.confidence),
        ("rust", "extension", 85)
    );
    let d = resolve_language(&reg, &ctx("plaintext", "a.h"), "rust", None);
    assert_eq!(
        (
            d.language.as_str(),
            d.reason,
            d.uncertain,
            d.suggestion.as_deref()
        ),
        ("rust", "ambiguous-extension", true, Some("c"))
    );
    // the validators of the built-in profiles are untouched
    assert!(codes("rust", "fn main() {\n    println!(\"}\");\n}\n", &[]).is_empty());
    assert_eq!(
        codes("rust", "fn main() {\n", &[]),
        vec!["BRACKET_UNCLOSED"]
    );
    assert_eq!(
        codes("powershell", "$a ?? 1", &[("version", "5.1")]),
        vec!["PS7_NULL_COALESCING"]
    );
    assert_eq!(codes("machine-code", "GG", &[]), vec!["INVALID_HEX"]);
    assert_eq!(
        codes("cobol", "x", &[]),
        vec!["NO_VALIDATOR"],
        "an unknown profile has no validator"
    );
}

#[test]
fn the_sql_validator_masks_comments_and_literals() {
    // a comment and a string hide their brackets
    assert!(
        codes(
            "cypher",
            "-- a ( comment\nMATCH (n) RETURN n;",
            &[("version", "5")]
        )
        .is_empty()
    );
    assert!(
        codes(
            "cypher",
            "/* a ) comment */ MATCH (n) RETURN n;",
            &[("version", "5")]
        )
        .is_empty()
    );
    assert!(
        codes(
            "cypher",
            "MATCH (n) WHERE n.name = 'a ) b' RETURN n;",
            &[("version", "5")]
        )
        .is_empty()
    );
    // a doubled quote does not end the literal
    assert!(
        codes(
            "cypher",
            "MATCH (n) WHERE n.name = 'a '' b' RETURN n;",
            &[("version", "5")]
        )
        .is_empty()
    );
    // a double quoted identifier
    assert!(codes("plsql", "SELECT \"A B\" FROM emp;", &[("version", "21c")]).is_empty());
    // the brackets of the code are checked
    assert_eq!(
        codes(
            "plsql",
            "SELECT * FROM emp WHERE (a = 1",
            &[("version", "21c")]
        ),
        vec!["BRACKET_UNCLOSED"]
    );
    assert_eq!(
        codes(
            "plsql",
            "SELECT * FROM emp WHERE a) = 1",
            &[("version", "21c")]
        ),
        vec!["BRACKET_UNEXPECTED"]
    );
    // an unterminated string is reported with the line of the opening quote
    assert_eq!(
        codes(
            "cypher",
            "MATCH (n)\nWHERE n.name = 'abc\n",
            &[("version", "5")]
        ),
        vec!["SQL_UNTERMINATED_STRING"]
    );
    let d: Dialect = [("version".to_string(), "5".to_string())]
        .into_iter()
        .collect();
    let r = validate_source("cypher", "MATCH (n)\nWHERE n.name = 'abc\n", &d);
    assert_eq!(r.issues[0].line, 2, "the line of the opening quote");
    assert!(!r.ok);
    assert_eq!(r.level, "invalid");
}

#[test]
fn the_sql_validator_reports_the_dialect_gated_features() {
    // Cypher 5 features in a Cypher 4 file
    assert_eq!(
        codes(
            "cypher",
            "MATCH (n) WHERE EXISTS { MATCH (m) } RETURN n;",
            &[("version", "4")]
        ),
        vec!["CYPHER5_EXISTS_SUBQUERY"]
    );
    assert_eq!(
        codes(
            "cypher",
            "MATCH (n) IN TRANSACTIONS RETURN n;",
            &[("version", "4")]
        ),
        vec!["CYPHER5_IN_TRANSACTIONS"]
    );
    // the same text is fine in Cypher 5
    assert!(
        codes(
            "cypher",
            "MATCH (n) WHERE EXISTS { MATCH (m) } RETURN n;",
            &[("version", "5")]
        )
        .is_empty()
    );
    // PL/SQL: LISTAGG needs 19c
    assert_eq!(
        codes(
            "plsql",
            "SELECT LISTAGG(name) FROM emp;",
            &[("version", "18c")]
        ),
        vec!["PLSQL19C_LISTAGG"]
    );
    assert!(
        codes(
            "plsql",
            "SELECT LISTAGG(name) FROM emp;",
            &[("version", "19c")]
        )
        .is_empty()
    );
    // T-SQL: CREATE OR ALTER needs 2016, STRING_AGG needs 2017
    assert_eq!(
        codes(
            "tsql",
            "CREATE OR ALTER PROCEDURE p AS BEGIN END;",
            &[("version", "2012")]
        ),
        vec!["TSQL2016_CREATE_OR_ALTER"]
    );
    assert_eq!(
        codes(
            "tsql",
            "SELECT STRING_AGG(name) FROM emp;",
            &[("version", "2016")]
        ),
        vec!["TSQL2017_STRING_AGG"]
    );
    assert!(
        codes(
            "tsql",
            "SELECT STRING_AGG(name) FROM emp;",
            &[("version", "2017")]
        )
        .is_empty()
    );
    // a commented out feature does not fire
    assert!(
        codes(
            "tsql",
            "-- CREATE OR ALTER PROCEDURE p AS BEGIN END;",
            &[("version", "2012")]
        )
        .is_empty()
    );
    // without a version axis there is no dialect check at all
    assert!(codes("tsql", "CREATE OR ALTER PROCEDURE p AS BEGIN END;", &[]).is_empty());
}

#[test]
fn well_formed_sql_of_every_dialect_is_valid() {
    for (profile, text, dialect) in [
        (
            "cypher",
            "MATCH (n)\nWHERE n.name = 'a'\nRETURN n;",
            &[("version", "5")][..],
        ),
        (
            "plsql",
            "SELECT name FROM emp WHERE id = 1;",
            &[("version", "21c")][..],
        ),
        (
            "tsql",
            "SELECT name FROM emp WHERE id = 1;",
            &[("version", "2022")][..],
        ),
    ] {
        let r = validate_source(
            profile,
            text,
            &dialect
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        assert!(r.ok, "{profile}: {:?}", r.issues);
        assert_eq!(r.level, "valid");
        assert!(r.issues.is_empty());
    }
}

#[test]
fn a_plugin_pack_may_not_claim_what_a_built_in_pack_owns() {
    let mut reg = Registry::builtin_with_plugins(&[]).expect("packs");
    // the extension of a built-in profile
    let pack = pack_doc(
        r#"{"schema_version":1,"profile_id":"cobol","version":"1.0.0","display_name":"Cobol",
            "language_ids":["cobol"],"accepted_extensions":[".rs"],"validator":"sql",
            "dialects":{},"keywords":[],"triggers":[]}"#,
    );
    assert_eq!(
        reg.add_plugin(&pack, 0).unwrap_err().code,
        "RULEPACK_INVALID"
    );
    // the language id of a built-in profile
    let pack = pack_doc(
        r#"{"schema_version":1,"profile_id":"cobol","version":"1.0.0","display_name":"Cobol",
            "language_ids":["rust"],"accepted_extensions":[".cob"],"validator":"sql",
            "dialects":{},"keywords":[],"triggers":[]}"#,
    );
    assert_eq!(
        reg.add_plugin(&pack, 0).unwrap_err().code,
        "RULEPACK_INVALID"
    );
    // a free extension is fine
    let pack = pack_doc(
        r#"{"schema_version":1,"profile_id":"cobol","version":"1.0.0","display_name":"Cobol",
            "language_ids":["cobol"],"accepted_extensions":[".cob"],"validator":"sql",
            "dialects":{},"keywords":[],"triggers":[]}"#,
    );
    reg.add_plugin(&pack, 0)
        .expect("a free extension is allowed");
    assert!(reg.available_plugins().contains(&"cobol".to_string()));
}

#[test]
fn the_profile_id_rules_of_a_plugin_pack() {
    assert!(is_plugin_profile("cobol"));
    assert!(is_plugin_profile("my-lang-2"));
    assert!(
        !is_plugin_profile("rust"),
        "a built-in profile is not a plugin"
    );
    assert!(!is_plugin_profile("Cobol"), "the id starts lower case");
    assert!(!is_plugin_profile("1cobol"), "the id starts with a letter");
    assert!(!is_plugin_profile("cobol_2"), "the id has no underscore");
    assert!(!is_plugin_profile(""), "the id is not empty");
    assert!(
        !is_plugin_profile(&"a".repeat(33)),
        "the id is at most 32 characters"
    );
    // the built-in kind only accepts the seven built-in profiles
    let pack = pack_doc(
        r#"{"schema_version":1,"profile_id":"cobol","version":"1.0.0","display_name":"Cobol",
            "language_ids":["cobol"],"accepted_extensions":[".cob"],"validator":"sql",
            "dialects":{},"keywords":[],"triggers":[]}"#,
    );
    assert!(parse_pack(&pack, PackKind::Builtin, 0).is_err());
    assert!(parse_pack(&pack, PackKind::Plugin, 0).is_ok());
    // a plugin pack has the same shape as a built-in one
    let pack = pack_doc(r#"{"schema_version":1,"profile_id":"cobol","version":"1.0.0"}"#);
    assert!(
        parse_pack(&pack, PackKind::Plugin, 0).is_err(),
        "the shape is complete"
    );
}

#[test]
fn the_protocol_switches_the_plugin_set() {
    let call = |line: &str| parse(&handle_line(line), Limits::DEFAULT).expect("a JSON answer");
    let list = |v: &Json, key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Json::as_array)
            .expect("an array")
            .iter()
            .map(|x| x.as_str().unwrap_or_default().to_string())
            .collect()
    };
    // the default set of the product
    let h = call(r#"{"op":"health"}"#);
    assert_eq!(list(&h, "languages"), LANGUAGES.to_vec());
    assert_eq!(list(&h, "pluginLanguages"), vec!["cypher", "plsql"]);
    assert_eq!(
        list(&h, "availablePluginLanguages"),
        PLUGIN_LANGUAGES.to_vec()
    );
    let mut want = LANGUAGES.to_vec();
    want.extend(["cypher", "plsql"]);
    assert_eq!(list(&h, "activeProfiles"), want);

    // T-SQL is switched on
    let r = call(r#"{"op":"setPluginLanguages","languages":["cypher","plsql","tsql"]}"#);
    assert_eq!(r.get("ok"), Some(&Json::Bool(true)));
    let h = call(r#"{"op":"health"}"#);
    assert_eq!(list(&h, "pluginLanguages"), PLUGIN_LANGUAGES.to_vec());
    let mut want = LANGUAGES.to_vec();
    want.extend(PLUGIN_LANGUAGES);
    assert_eq!(list(&h, "activeProfiles"), want);
    // and it predicts
    let p = call(r#"{"op":"predict","language":"tsql","prefix":"select"}"#);
    assert!(
        !p.get("candidates")
            .and_then(Json::as_array)
            .expect("candidates")
            .is_empty()
    );

    // unknown ids are ignored, they are not an error
    let r = call(r#"{"op":"setPluginLanguages","languages":["cobol"]}"#);
    assert_eq!(r.get("ok"), Some(&Json::Bool(true)));
    let h = call(r#"{"op":"health"}"#);
    assert!(list(&h, "pluginLanguages").is_empty());

    // a value that is not an array of strings is a protocol error
    for bad in [
        r#"{"op":"setPluginLanguages","languages":"cypher"}"#,
        r#"{"op":"setPluginLanguages","languages":[1]}"#,
        r#"{"op":"setPluginLanguages","languages":{}}"#,
        r#"{"op":"setPluginLanguages"}"#,
    ] {
        let r = call(bad);
        assert_eq!(
            r.get("error").and_then(Json::as_str),
            Some("INVALID_REQUEST"),
            "{bad}"
        );
        assert_eq!(
            r.get("field").and_then(Json::as_str),
            Some("languages"),
            "{bad}"
        );
    }
    // an empty list switches every plugin off
    let r = call(r#"{"op":"setPluginLanguages","languages":[]}"#);
    assert_eq!(r.get("ok"), Some(&Json::Bool(true)));
    let h = call(r#"{"op":"health"}"#);
    assert!(list(&h, "pluginLanguages").is_empty());

    // back to the default set of the product
    call(r#"{"op":"setPluginLanguages","languages":["cypher","plsql"]}"#);
    let h = call(r#"{"op":"health"}"#);
    assert_eq!(list(&h, "pluginLanguages"), vec!["cypher", "plsql"]);
}

#[test]
fn the_protocol_validates_a_plugin_profile() {
    let call = |line: &str| parse(&handle_line(line), Limits::DEFAULT).expect("a JSON answer");
    let v = call(r#"{"op":"validate","language":"plsql","text":"SELECT name FROM emp;"}"#);
    let res = v.get("result").expect("a result");
    assert_eq!(res.get("ok").and_then(Json::as_bool), Some(true));
    assert_eq!(res.get("level").and_then(Json::as_str), Some("valid"));
    let v = call(
        r#"{"op":"validate","language":"plsql","text":"SELECT LISTAGG(name) FROM emp;","dialect":{"version":"18c"}}"#,
    );
    let res = v.get("result").expect("a result");
    assert_eq!(res.get("level").and_then(Json::as_str), Some("invalid"));
    assert_eq!(
        res.get("issues").and_then(Json::as_array).expect("issues")[0]
            .get("code")
            .and_then(Json::as_str),
        Some("PLSQL19C_LISTAGG")
    );
}

/// The protocol schema must document the new operation: a contract that is not in the schema is a contract nobody
/// can check.
#[test]
fn the_schema_documents_the_plugin_operations() {
    let path = format!(
        "{}/../../schemas/core-protocol-v0.3.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(path).expect("schema file");
    parse(&raw, Limits::DEFAULT).expect("the schema is valid JSON");
    assert!(
        raw.contains("\"const\": \"setPluginLanguages\""),
        "the schema lacks the request of setPluginLanguages"
    );
    assert!(
        raw.contains("\"title\": \"setPluginLanguages\""),
        "the schema lacks the answer of setPluginLanguages"
    );
    // the language of a lock may be a plugin profile: the schema must not close the list at the seven built-ins
    assert!(
        !raw.contains("\"language\": {\"enum\":"),
        "the schema still closes the profile ids at the built-in ones"
    );
}
