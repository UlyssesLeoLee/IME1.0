//! Effective settings, validation and scenario resolution (port of the pure parts of `core/settings.js`:
//! `effective`, `validateSettings`, `resolveScenario`). Settings never contain keys.
//!
//! The settings value is plain JSON with the exact shape the JS engine uses, so the bridge can keep reading the
//! result: `{providers: {id: {api, endpoint, ...}}, scenarios: {realtime|f1Full|f1Insert: {...}}, budget: {...}}`.
//! Object key order follows JavaScript object-spread order (including the "integer-like keys first" rule), because
//! the golden fixtures compare serialised output.
//!
//! Known differences from the JS reference (all in input that VS Code's typed settings never produce): array and
//! string values where an object is expected are ignored instead of being spread into numeric keys.

use crate::error::{Result, fail};
use crate::json::Json;

pub const SCHEMA_VERSION: i64 = 1;
pub const SCENARIOS: [&str; 3] = ["realtime", "f1Full", "f1Insert"];
pub const API_FAMILIES: [&str; 3] = ["openai-responses", "openai-chat", "anthropic-messages"];
pub const EFFORT_LEVELS: [&str; 8] = [
    "auto", "none", "minimal", "low", "medium", "high", "xhigh", "extended",
];
pub const MAX_TOKENS_PARAMS: [&str; 2] = ["max_tokens", "max_completion_tokens"];

type Obj = Vec<(String, Json)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: &'static str,
}

fn s(v: &str) -> Json {
    Json::str(v)
}

fn default_providers() -> Obj {
    let p = |api: &str, endpoint: &str| Json::obj(vec![("api", s(api)), ("endpoint", s(endpoint))]);
    vec![
        (
            "openai".to_string(),
            p("openai-responses", "https://api.openai.com/v1/responses"),
        ),
        (
            "openai-compatible".to_string(),
            p("openai-chat", "https://api.example.com/v1/chat/completions"),
        ),
        (
            "anthropic".to_string(),
            p(
                "anthropic-messages",
                "https://api.anthropic.com/v1/messages",
            ),
        ),
    ]
}

fn default_scenario(name: &str) -> Obj {
    let (max_tokens, timeout) = match name {
        "realtime" => (250, 8000),
        "f1Full" => (12000, 60000),
        _ => (1500, 30000),
    };
    vec![
        ("provider".to_string(), s("openai")),
        ("model".to_string(), s("")),
        ("effort".to_string(), s("auto")),
        (
            "effortCapability".to_string(),
            Json::obj(vec![("state", s("unknown"))]),
        ),
        ("maxOutputTokens".to_string(), Json::int(max_tokens)),
        ("timeoutMs".to_string(), Json::int(timeout)),
    ]
}

fn default_budget() -> Obj {
    vec![
        ("maxRequests".to_string(), Json::int(200)),
        ("maxTokens".to_string(), Json::int(400_000)),
        ("pricePer1kTokens".to_string(), Json::int(0)),
    ]
}

/// `{...base, ...over}`: overridden keys keep their position, new keys are appended.
fn spread(mut base: Obj, over: &[(String, Json)]) -> Obj {
    for (k, v) in over {
        match base.iter_mut().find(|(bk, _)| bk == k) {
            Some(slot) => slot.1 = v.clone(),
            None => base.push((k.clone(), v.clone())),
        }
    }
    js_order(base)
}

/// The array-index value of a canonical numeric key (`"0"`, `"17"`), if it is one.
fn array_index(key: &str) -> Option<u64> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u64>().ok().filter(|n| *n < 4_294_967_295)
}

/// JavaScript property order: integer-like keys ascending first, then the other keys in insertion order.
fn js_order(obj: Obj) -> Obj {
    if !obj.iter().any(|(k, _)| array_index(k).is_some()) {
        return obj;
    }
    let (mut ints, rest): (Vec<_>, Vec<_>) =
        obj.into_iter().partition(|(k, _)| array_index(k).is_some());
    ints.sort_by_key(|(k, _)| array_index(k));
    ints.extend(rest);
    ints
}

/// `/^[\w.-]{1,40}$/` (JavaScript `\w` is ASCII only).
fn valid_provider_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

enum Field<'a> {
    Missing,
    Object(&'a [(String, Json)]),
}

/// `values[key]` for a field that must be an object (`null` counts as missing).
fn object_field<'a>(values: &'a Json, key: &str) -> std::result::Result<Field<'a>, String> {
    match values.get(key) {
        None | Some(Json::Null) => Ok(Field::Missing),
        Some(Json::Obj(o)) => Ok(Field::Object(o)),
        Some(_) => Err(key.to_string()),
    }
}

/// Field-by-field merge of user values over the defaults; unknown scenarios and invalid provider ids are dropped.
/// `Err(field)` names the first value that has the wrong type.
pub fn effective(values: &Json) -> std::result::Result<Json, String> {
    if !matches!(values, Json::Obj(_) | Json::Null) {
        return Err("values".to_string());
    }
    let user_providers = object_field(values, "providers")?;
    let user_scenarios = object_field(values, "scenarios")?;
    let user_budget = object_field(values, "budget")?;

    let defaults = default_providers();
    let merged = match user_providers {
        Field::Object(o) => spread(defaults.clone(), o),
        Field::Missing => defaults.clone(),
    };
    let mut providers: Obj = Vec::new();
    for (id, def) in merged {
        if !valid_provider_id(&id) {
            continue;
        }
        let Json::Obj(def) = def else { continue };
        let base = defaults
            .iter()
            .find(|(k, _)| *k == id)
            .and_then(|(_, v)| v.as_object().cloned())
            .unwrap_or_default();
        providers.push((id, Json::Obj(spread(base, &def))));
    }

    let mut scenarios: Obj = Vec::new();
    for name in SCENARIOS {
        let over: &[(String, Json)] = match &user_scenarios {
            Field::Object(o) => match o.iter().find(|(k, _)| k == name) {
                Some((_, Json::Obj(v))) => v,
                _ => &[],
            },
            Field::Missing => &[],
        };
        scenarios.push((
            name.to_string(),
            Json::Obj(spread(default_scenario(name), over)),
        ));
    }

    let budget = match user_budget {
        Field::Object(o) => spread(default_budget(), o),
        Field::Missing => default_budget(),
    };
    Ok(Json::obj(vec![
        ("providers", Json::Obj(js_order(providers))),
        ("scenarios", Json::Obj(scenarios)),
        ("budget", Json::Obj(budget)),
    ]))
}

/// The settings every session starts with (no user values).
pub fn defaults() -> Json {
    effective(&Json::Null).unwrap_or(Json::Null)
}

fn entries(v: Option<&Json>) -> &[(String, Json)] {
    match v {
        Some(Json::Obj(o)) => o,
        _ => &[],
    }
}

fn is_int_in(v: Option<&Json>, lo: i64, hi: i64) -> bool {
    matches!(v.and_then(Json::as_i64), Some(n) if (lo..=hi).contains(&n))
}

pub fn validate(settings: &Json) -> Vec<Issue> {
    let mut issues = Vec::new();
    let providers = entries(settings.get("providers"));
    for (id, p) in providers {
        let api = p.get("api").and_then(Json::as_str).unwrap_or("");
        if !API_FAMILIES.contains(&api) {
            issues.push(Issue {
                path: format!("providers.{id}.api"),
                message: "unknown API family",
            });
        }
        // optional provider fields: `maxTokensParam` (openai-chat) and `apiVersion` (anthropic-messages)
        if let Some(v) = p.get("maxTokensParam")
            && !v.as_str().is_some_and(|x| MAX_TOKENS_PARAMS.contains(&x))
        {
            issues.push(Issue {
                path: format!("providers.{id}.maxTokensParam"),
                message: "must be max_tokens or max_completion_tokens",
            });
        }
        if let Some(v) = p.get("apiVersion")
            && !v.as_str().is_some_and(is_api_version)
        {
            issues.push(Issue {
                path: format!("providers.{id}.apiVersion"),
                message: "expected YYYY-MM-DD",
            });
        }
        if !p
            .get("endpoint")
            .and_then(Json::as_str)
            .is_some_and(|e| check_endpoint(e).is_ok())
        {
            issues.push(Issue {
                path: format!("providers.{id}.endpoint"),
                message: "HTTPS endpoint required",
            });
        }
    }
    for (name, sc) in entries(settings.get("scenarios")) {
        let provider = sc.get("provider").and_then(Json::as_str);
        if !provider.is_some_and(|p| providers.iter().any(|(id, _)| id == p)) {
            issues.push(Issue {
                path: format!("scenarios.{name}.provider"),
                message: "undefined provider",
            });
        }
        if !sc
            .get("effort")
            .and_then(Json::as_str)
            .is_some_and(|e| EFFORT_LEVELS.contains(&e))
        {
            issues.push(Issue {
                path: format!("scenarios.{name}.effort"),
                message: "invalid effort level",
            });
        }
        if !is_int_in(sc.get("maxOutputTokens"), 1, 64_000) {
            issues.push(Issue {
                path: format!("scenarios.{name}.maxOutputTokens"),
                message: "out of range",
            });
        }
        if !is_int_in(sc.get("timeoutMs"), 1000, 300_000) {
            issues.push(Issue {
                path: format!("scenarios.{name}.timeoutMs"),
                message: "out of range (1 s - 300 s)",
            });
        }
    }
    issues
}

/// `/^\d{4}-\d{2}-\d{2}$/`
fn is_api_version(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// The concrete request settings for one scenario (scenarios are fully independent, FR-021).
pub fn resolve_scenario(settings: &Json, name: &str) -> Result<Json> {
    let Some(sc) = entries(settings.get("scenarios"))
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
    else {
        return fail("SETTINGS_INVALID", format!("unknown scenario {name}"));
    };
    let provider_id = sc.get("provider").and_then(Json::as_str).unwrap_or("");
    let Some(p) = entries(settings.get("providers"))
        .iter()
        .find(|(k, _)| k == provider_id)
        .map(|(_, v)| v)
    else {
        return fail(
            "MODEL_UNSUPPORTED",
            format!("provider \"{provider_id}\" is not defined"),
        );
    };
    let mut out: Obj = vec![
        ("scenario".to_string(), s(name)),
        ("providerId".to_string(), s(provider_id)),
    ];
    for (key, source, field) in [
        ("family", p, "api"),
        ("endpoint", p, "endpoint"),
        ("apiVersion", p, "apiVersion"),
        ("maxTokensParam", p, "maxTokensParam"),
        ("model", sc, "model"),
        ("effort", sc, "effort"),
        ("capability", sc, "effortCapability"),
        ("maxOutputTokens", sc, "maxOutputTokens"),
        ("timeoutMs", sc, "timeoutMs"),
    ] {
        if let Some(v) = source.get(field) {
            out.push((key.to_string(), v.clone()));
        }
    }
    Ok(Json::Obj(out))
}

/// Percent-decoding of a URL host; a malformed escape stays as it is.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("00");
            out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Port of `ai.checkEndpoint` (WHATWG `new URL` as far as it matters here): an absolute `https:` URL with a
/// non-empty host, a valid port and no embedded credentials. The full AI module arrives in step 1.2.
#[allow(clippy::result_unit_err)] // the only information is "valid or not"
pub fn check_endpoint(endpoint: &str) -> std::result::Result<(), ()> {
    // the URL parser trims C0 controls and spaces at both ends and drops tab / newline everywhere
    let cleaned: String = endpoint
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let Some(colon) = cleaned.find(':') else {
        return Err(());
    };
    let scheme = &cleaned[..colon];
    let mut sc = scheme.chars();
    if !sc.next().is_some_and(|c| c.is_ascii_alphabetic())
        || !sc.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        || !scheme.eq_ignore_ascii_case("https")
    {
        return Err(());
    }
    // a special scheme ignores any number of slashes and backslashes before the authority (`https:host` works)
    let rest = cleaned[colon + 1..].trim_start_matches(['/', '\\']);
    let authority_end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let (credentials, host_port) = match authority.rfind('@') {
        Some(at) => (&authority[..at], &authority[at + 1..]),
        None => ("", authority),
    };
    if credentials.chars().any(|c| c != ':') {
        return Err(()); // a user name or password
    }
    let (host, port) = if host_port.starts_with('[') {
        let close = host_port.find(']').ok_or(())?;
        let inner = &host_port[1..close];
        if inner.is_empty()
            || !inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
        {
            return Err(());
        }
        let after = &host_port[close + 1..];
        // the bracketed address was validated above; "v6" stands in for it in the host checks below
        match after.strip_prefix(':') {
            Some(p) => ("v6", p),
            None if after.is_empty() => ("v6", ""),
            None => return Err(()),
        }
    } else {
        match host_port.find(':') {
            Some(i) => (&host_port[..i], &host_port[i + 1..]),
            None => (host_port, ""),
        }
    };
    if host.is_empty() {
        return Err(());
    }
    // the host is percent-decoded before the forbidden code points are checked (so `%6D` is fine, `%25` is not)
    if percent_decode(host).chars().any(|c| {
        c <= ' '
            || c == '\u{7f}'
            || matches!(
                c,
                '#' | '%' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|'
            )
    }) {
        return Err(());
    }
    if !port.is_empty() {
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(());
        }
        match port.parse::<u64>() {
            Ok(n) if n <= 65_535 => {}
            _ => return Err(()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Limits, parse};

    fn j(s: &str) -> Json {
        parse(s, Limits::DEFAULT).unwrap()
    }

    #[test]
    fn defaults_have_the_js_shape_and_order() {
        let d = defaults();
        assert_eq!(
            d.to_json_string(),
            concat!(
                r#"{"providers":{"openai":{"api":"openai-responses","endpoint":"https://api.openai.com/v1/responses"},"#,
                r#""openai-compatible":{"api":"openai-chat","endpoint":"https://api.example.com/v1/chat/completions"},"#,
                r#""anthropic":{"api":"anthropic-messages","endpoint":"https://api.anthropic.com/v1/messages"}},"#,
                r#""scenarios":{"realtime":{"provider":"openai","model":"","effort":"auto","effortCapability":{"state":"unknown"},"maxOutputTokens":250,"timeoutMs":8000},"#,
                r#""f1Full":{"provider":"openai","model":"","effort":"auto","effortCapability":{"state":"unknown"},"maxOutputTokens":12000,"timeoutMs":60000},"#,
                r#""f1Insert":{"provider":"openai","model":"","effort":"auto","effortCapability":{"state":"unknown"},"maxOutputTokens":1500,"timeoutMs":30000}},"#,
                r#""budget":{"maxRequests":200,"maxTokens":400000,"pricePer1kTokens":0}}"#
            )
        );
        assert!(validate(&d).is_empty());
    }

    #[test]
    fn user_values_override_field_by_field() {
        let e = effective(&j(
            r#"{"providers":{"mockA":{"api":"openai-responses","endpoint":"https://127.0.0.1:9/x"},
                "openai":{"endpoint":"https://proxy.example/v1/responses"},"bad id":{"api":"x"},"nul":null},
                "scenarios":{"f1Full":{"provider":"mockA","model":"m","effort":"high"},"other":{"model":"z"}},
                "budget":{"maxRequests":3,"extra":1}}"#,
        ))
        .unwrap();
        let providers = e.get("providers").unwrap();
        assert_eq!(
            providers
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>(),
            ["openai", "openai-compatible", "anthropic", "mockA"],
            "defaults first, invalid ids and null definitions dropped"
        );
        // a partial override keeps the default of the other field
        assert_eq!(
            providers.get("openai").unwrap().to_json_string(),
            r#"{"api":"openai-responses","endpoint":"https://proxy.example/v1/responses"}"#
        );
        let full = e.get("scenarios").unwrap().get("f1Full").unwrap();
        assert_eq!(full.get("provider").and_then(Json::as_str), Some("mockA"));
        assert_eq!(
            full.get("maxOutputTokens").and_then(Json::as_i64),
            Some(12000)
        );
        assert!(e.get("scenarios").unwrap().get("other").is_none());
        assert_eq!(
            e.get("budget").unwrap().to_json_string(),
            r#"{"maxRequests":3,"maxTokens":400000,"pricePer1kTokens":0,"extra":1}"#
        );
    }

    #[test]
    fn wrong_types_are_reported_by_field_name() {
        assert_eq!(
            effective(&j(r#"{"providers":[]}"#)),
            Err("providers".into())
        );
        assert_eq!(effective(&j(r#"{"budget":5}"#)), Err("budget".into()));
        assert_eq!(effective(&j("[1]")), Err("values".into()));
        assert!(effective(&j("{}")).is_ok());
    }

    #[test]
    fn integer_like_provider_ids_come_first_as_in_javascript() {
        let e = effective(&j(
            r#"{"providers":{"b":{"api":"openai-chat","endpoint":"https://b.example/"},"10":{"api":"openai-chat","endpoint":"https://x.example/"},"2":{"api":"openai-chat","endpoint":"https://y.example/"}}}"#,
        ))
        .unwrap();
        let ids: Vec<&str> = e
            .get("providers")
            .unwrap()
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            ids,
            ["2", "10", "openai", "openai-compatible", "anthropic", "b"]
        );
    }

    #[test]
    fn validation_reports_each_problem_with_its_path() {
        let e = effective(&j(
            r#"{"providers":{"p":{"api":"nope","endpoint":"http://insecure.example"}},
                "scenarios":{"realtime":{"provider":"ghost","effort":"turbo","maxOutputTokens":0,"timeoutMs":999},
                             "f1Full":{"maxOutputTokens":64001,"timeoutMs":300001},
                             "f1Insert":{"maxOutputTokens":1.5}}}"#,
        ))
        .unwrap();
        let got: Vec<(String, &str)> = validate(&e)
            .into_iter()
            .map(|i| (i.path, i.message))
            .collect();
        let want = [
            ("providers.p.api", "unknown API family"),
            ("providers.p.endpoint", "HTTPS endpoint required"),
            ("scenarios.realtime.provider", "undefined provider"),
            ("scenarios.realtime.effort", "invalid effort level"),
            ("scenarios.realtime.maxOutputTokens", "out of range"),
            ("scenarios.realtime.timeoutMs", "out of range (1 s - 300 s)"),
            ("scenarios.f1Full.maxOutputTokens", "out of range"),
            ("scenarios.f1Full.timeoutMs", "out of range (1 s - 300 s)"),
            ("scenarios.f1Insert.maxOutputTokens", "out of range"),
        ];
        assert_eq!(
            got,
            want.iter()
                .map(|(p, m)| (p.to_string(), *m))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn provider_profile_extras_pass_through_and_are_validated() {
        let e = effective(&j(
            r#"{"providers":{"oa":{"api":"openai-chat","endpoint":"https://a.example/v1","maxTokensParam":"max_completion_tokens"},
                              "an":{"api":"anthropic-messages","endpoint":"https://b.example/v1","apiVersion":"2026-10-21"},
                              "bad1":{"api":"openai-chat","endpoint":"https://c.example/v1","maxTokensParam":"max_new_tokens"},
                              "bad2":{"api":"anthropic-messages","endpoint":"https://d.example/v1","apiVersion":"latest"},
                              "bad3":{"api":"anthropic-messages","endpoint":"https://d.example/v1","apiVersion":20261021}},
                "scenarios":{"f1Full":{"provider":"oa","effort":"xhigh"},"f1Insert":{"provider":"an","effort":"none"},
                             "realtime":{"provider":"oa","effort":"minimal"}}}"#,
        ))
        .unwrap();
        let got: Vec<(String, &str)> = validate(&e)
            .into_iter()
            .map(|i| (i.path, i.message))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "providers.bad1.maxTokensParam".to_string(),
                    "must be max_tokens or max_completion_tokens"
                ),
                (
                    "providers.bad2.apiVersion".to_string(),
                    "expected YYYY-MM-DD"
                ),
                (
                    "providers.bad3.apiVersion".to_string(),
                    "expected YYYY-MM-DD"
                ),
            ],
            "the new effort levels are accepted"
        );
        let r = resolve_scenario(&e, "f1Full").unwrap();
        assert_eq!(
            r.get("maxTokensParam").and_then(Json::as_str),
            Some("max_completion_tokens")
        );
        assert!(
            r.get("apiVersion").is_none(),
            "absent fields are omitted like undefined"
        );
        let r = resolve_scenario(&e, "f1Insert").unwrap();
        assert_eq!(
            r.get("apiVersion").and_then(Json::as_str),
            Some("2026-10-21")
        );
        // field order of the resolved scenario follows the JS object literal
        assert_eq!(
            r.as_object()
                .unwrap()
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>(),
            [
                "scenario",
                "providerId",
                "family",
                "endpoint",
                "apiVersion",
                "model",
                "effort",
                "capability",
                "maxOutputTokens",
                "timeoutMs"
            ]
        );
    }

    #[test]
    fn scenarios_resolve_independently() {
        let e = effective(&j(
            r#"{"providers":{"mockB":{"api":"openai-chat","endpoint":"https://b.example/chat"}},
                "scenarios":{"f1Full":{"provider":"mockB","model":"big","effort":"high"},"f1Insert":{"provider":"ghost"}}}"#,
        ))
        .unwrap();
        let r = resolve_scenario(&e, "f1Full").unwrap();
        assert_eq!(
            r.to_json_string(),
            r#"{"scenario":"f1Full","providerId":"mockB","family":"openai-chat","endpoint":"https://b.example/chat","model":"big","effort":"high","capability":{"state":"unknown"},"maxOutputTokens":12000,"timeoutMs":60000}"#
        );
        assert_eq!(
            resolve_scenario(&e, "f1Insert").unwrap_err().code,
            "MODEL_UNSUPPORTED"
        );
        assert_eq!(
            resolve_scenario(&e, "nope").unwrap_err().code,
            "SETTINGS_INVALID"
        );
    }

    #[test]
    fn endpoint_rules_follow_whatwg_url_parsing() {
        for good in [
            "https://api.openai.com/v1/responses",
            "HTTPS://Example.COM/x",
            "https:example.com",
            "https:\\\\example.com\\x",
            "https:///example.com",
            "https://127.0.0.1:8443/v1",
            "https://[::1]:8443/v1",
            "https://@example.com",
            "https://:@example.com",
            "https://example.com:/x",
            "  https://example.com/x \n",
            "https://exa%6Dple.com/",
            "ht\ttps://example.com",
        ] {
            assert!(check_endpoint(good).is_ok(), "should accept {good:?}");
        }
        for bad in [
            "",
            "example.com",
            "http://example.com",
            "ftp://example.com",
            "https://",
            "https:///",
            "https://user@example.com",
            "https://user:pw@example.com",
            "https://:pw@example.com",
            "https://example.com:99999",
            "https://example.com:80a",
            "https://exa mple.com",
            "https://exa<mple.com",
            "https://exa%25mple.com",
            "https://exa%zzmple.com",
            "https://a@",
            "//example.com",
        ] {
            assert!(check_endpoint(bad).is_err(), "should reject {bad:?}");
        }
    }
}
