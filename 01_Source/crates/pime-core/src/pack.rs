//! Declarative language packs (DD-03): strict validation (port of `validatePack`, UT-011/UT-058) and the registry.
//! Packs are DATA, never scripts: unknown keys are rejected, sizes are bounded, `${` may only open a
//! well-formed placeholder, and user packs may only add triggers/keywords (never replace built-in behaviour).

use crate::candidate::CandidateKind;
use crate::error::{fail, PimeError, Result};
use crate::json::{self, Json, Limits};
use crate::util::utf16_len;
use crate::validators::Dialect;
use std::collections::{HashMap, HashSet};

pub const LANGUAGES: [&str; 7] = [
    "rust",
    "python",
    "typescript",
    "powershell",
    "c",
    "assembly",
    "machine-code",
];

const PACK_KEYS: [&str; 11] = [
    "schema_version",
    "profile_id",
    "version",
    "display_name",
    "language_ids",
    "accepted_extensions",
    "validator",
    "dialects",
    "keywords",
    "triggers",
    "generated",
];
const USER_PACK_KEYS: [&str; 5] = [
    "schema_version",
    "profile_id",
    "version",
    "keywords",
    "triggers",
];
const TRIGGER_KEYS: [&str; 9] = [
    "id",
    "pattern",
    "kind",
    "weight",
    "when",
    "contexts",
    "in",
    "not_in",
    "candidates",
];
const CANDIDATE_KEYS: [&str; 6] = [
    "label",
    "explain",
    "text",
    "stmt_semicolon",
    "assumptions",
    "when",
];

pub struct PackLimits;

impl PackLimits {
    pub const PACK_BYTES: usize = 262_144;
    pub const TRIGGERS: usize = 2000;
    pub const CANDIDATES: usize = 8;
    pub const TEXT: usize = 4000;
    pub const LABEL: usize = 200;
    pub const KEYWORDS: usize = 2000;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    Statement,
    Expression,
    Type,
    Unknown,
    Literal,
}

impl Context {
    pub fn parse(s: &str) -> Option<Context> {
        Some(match s {
            "statement" => Context::Statement,
            "expression" => Context::Expression,
            "type" => Context::Type,
            "unknown" => Context::Unknown,
            "literal" => Context::Literal,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Context::Statement => "statement",
            Context::Expression => "expression",
            Context::Type => "type",
            Context::Unknown => "unknown",
            Context::Literal => "literal",
        }
    }
}

pub type When = Vec<(String, Vec<String>)>;

#[derive(Clone, Debug)]
pub struct DialectAxis {
    pub name: String,
    pub values: Vec<String>,
    pub default: String,
}

#[derive(Clone, Debug)]
pub struct Template {
    pub label: String,
    pub explain: String,
    pub text: String,
    pub stmt_semicolon: bool,
    pub assumptions: Vec<String>,
    pub when: When,
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub id: String,
    pub pattern: String,
    pub kind: CandidateKind,
    pub weight: Option<i64>,
    pub when: When,
    pub contexts: Option<Vec<Context>>,
    pub in_blocks: Option<Vec<String>>,
    pub not_in: Option<Vec<String>>,
    pub candidates: Vec<Template>,
    pub user: bool,
}

#[derive(Clone, Debug)]
pub struct Pack {
    pub profile_id: String,
    pub version: String,
    pub display_name: String,
    pub language_ids: Vec<String>,
    pub accepted_extensions: Vec<String>,
    pub dialects: Vec<DialectAxis>,
    pub keywords: Vec<String>,
    pub triggers: Vec<Trigger>,
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    fail("RULEPACK_INVALID", message)
}

fn check_keys(obj: &[(String, Json)], allowed: &[&str], wher: &str) -> Result<()> {
    for (k, _) in obj {
        if !allowed.contains(&k.as_str()) {
            return invalid(format!(
                "{wher}: unknown field \"{k}\" (rule packs are declarative data, never scripts)"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- placeholders

/// `${name}` at `i`: returns (end, name). `name` is `[A-Za-z_][A-Za-z0-9_.-]*`.
fn placeholder_at(t: &[char], i: usize) -> Option<(usize, String)> {
    if t.get(i) != Some(&'$') || t.get(i + 1) != Some(&'{') {
        return None;
    }
    let s = i + 2;
    if !t
        .get(s)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
    {
        return None;
    }
    let mut e = s + 1;
    while e < t.len() && (t[e].is_ascii_alphanumeric() || matches!(t[e], '_' | '.' | '-')) {
        e += 1;
    }
    if t.get(e) == Some(&'}') {
        Some((e + 1, t[s..e].iter().collect()))
    } else {
        None
    }
}

/// `text.replace(PLACEHOLDER, name => name === 'cursor' ? cursorText : name)`.
pub fn render_plain(text: &str, cursor_text: &str) -> String {
    let t: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        if let Some((end, name)) = placeholder_at(&t, i) {
            out.push_str(if name == "cursor" { cursor_text } else { &name });
            i = end;
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out
}

/// Text with every well-formed placeholder removed (a remaining `${` is malformed).
fn strip_placeholders(text: &str) -> String {
    let t: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        if let Some((end, _)) = placeholder_at(&t, i) {
            i = end;
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------- field checks

fn in_class(s: &str, min: usize, max: usize, class: impl Fn(char) -> bool) -> bool {
    let n = s.chars().count();
    n >= min && n <= max && s.chars().all(class)
}

fn is_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn string_list(v: &Json) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_string))
        .collect()
}

fn parse_when(v: &Json, id: &str) -> Result<When> {
    let Some(obj) = v.as_object() else {
        return invalid(format!("invalid when in {id}"));
    };
    let mut out = Vec::new();
    for (axis, allowed) in obj {
        let Some(list) = string_list(allowed) else {
            return invalid(format!("invalid when in {id}"));
        };
        out.push((axis.clone(), list));
    }
    Ok(out)
}

fn parse_template(c: &Json, id: &str) -> Result<Template> {
    let Some(obj) = c.as_object() else {
        return invalid("candidate must be an object");
    };
    check_keys(obj, &CANDIDATE_KEYS, "candidate")?;
    let text = match c.get("text") {
        Some(Json::Str(t)) if !t.is_empty() && utf16_len(t) <= PackLimits::TEXT => t.clone(),
        _ => return invalid(format!("invalid text in {id}")),
    };
    let label = match c.get("label") {
        Some(Json::Str(l)) if utf16_len(l) <= PackLimits::LABEL => l.clone(),
        _ => return invalid(format!("invalid label in {id}")),
    };
    // "${" may only open a well-formed placeholder: no template/command substitution syntax is accepted.
    if strip_placeholders(&text).contains("${") {
        return invalid(format!("malformed placeholder in {id}"));
    }
    let assumptions = match c.get("assumptions") {
        None => Vec::new(),
        Some(a) => string_list(a).ok_or_else(|| {
            PimeError::new("RULEPACK_INVALID", format!("invalid assumptions in {id}"))
        })?,
    };
    // stricter than the reference engine: these optional fields are type-checked as well
    let explain = match c.get("explain") {
        None => String::new(),
        Some(Json::Str(s)) => s.clone(),
        Some(_) => return invalid(format!("invalid explain in {id}")),
    };
    let stmt_semicolon = match c.get("stmt_semicolon") {
        None => false,
        Some(Json::Bool(b)) => *b,
        Some(_) => return invalid(format!("invalid stmt_semicolon in {id}")),
    };
    let when = match c.get("when") {
        None => Vec::new(),
        Some(w) => parse_when(w, id)?,
    };
    Ok(Template {
        label,
        explain,
        text,
        stmt_semicolon,
        assumptions,
        when,
    })
}

fn parse_trigger(t: &Json, ids: &mut HashSet<String>, user: bool) -> Result<Trigger> {
    let Some(obj) = t.as_object() else {
        return invalid("trigger must be an object");
    };
    check_keys(obj, &TRIGGER_KEYS, "trigger")?;
    let id = match t.get("id") {
        Some(Json::Str(s))
            if in_class(s, 1, 80, |c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
            }) && !ids.contains(s) =>
        {
            s.clone()
        }
        other => {
            return invalid(format!(
                "trigger id missing or duplicate: {}",
                other.and_then(Json::as_str).unwrap_or("")
            ))
        }
    };
    ids.insert(id.clone());
    let pattern = match t.get("pattern") {
        Some(Json::Str(p))
            if in_class(p, 1, 64, |c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '#' | '.' | '-')
            }) =>
        {
            p.clone()
        }
        _ => return invalid(format!("invalid pattern in {id}")),
    };
    // `t.kind || 'syntax'`: a falsy kind means the default
    let kind = match t.get("kind") {
        None | Some(Json::Null) | Some(Json::Bool(false)) => CandidateKind::Syntax,
        Some(Json::Str(s)) if s.is_empty() => CandidateKind::Syntax,
        Some(Json::Str(s)) if s == "syntax" => CandidateKind::Syntax,
        Some(Json::Str(s)) if s == "prefix" => CandidateKind::Prefix,
        _ => return invalid(format!("invalid kind in {id}")),
    };
    let weight = match t.get("weight") {
        None => None,
        Some(w) => match w.as_i64() {
            Some(n) if (0..=100).contains(&n) => Some(n),
            _ => return invalid(format!("invalid weight in {id}")),
        },
    };
    let contexts = match t.get("contexts") {
        None => None,
        Some(c) => {
            let list = string_list(c).and_then(|l| {
                l.iter()
                    .map(|x| Context::parse(x))
                    .collect::<Option<Vec<_>>>()
            });
            Some(list.ok_or_else(|| {
                PimeError::new("RULEPACK_INVALID", format!("invalid contexts in {id}"))
            })?)
        }
    };
    let blocks = |key: &str| -> Result<Option<Vec<String>>> {
        match t.get(key) {
            None => Ok(None),
            Some(v) => string_list(v).map(Some).ok_or_else(|| {
                PimeError::new("RULEPACK_INVALID", format!("invalid {key} in {id}"))
            }),
        }
    };
    let in_blocks = blocks("in")?;
    let not_in = blocks("not_in")?;
    let when = match t.get("when") {
        None => Vec::new(),
        Some(w) => parse_when(w, &id)?,
    };
    let cands = match t.get("candidates").and_then(Json::as_array) {
        Some(c) if !c.is_empty() && c.len() <= PackLimits::CANDIDATES => c,
        _ => return invalid(format!("candidates of {id} out of bounds")),
    };
    let candidates = cands
        .iter()
        .map(|c| parse_template(c, &id))
        .collect::<Result<Vec<_>>>()?;
    Ok(Trigger {
        id,
        pattern,
        kind,
        weight,
        when,
        contexts,
        in_blocks,
        not_in,
        candidates,
        user,
    })
}

/// Port of `validatePack`: validates and converts. `bytes` is the raw size of the pack file.
pub fn parse_pack(pack: &Json, builtin: bool, bytes: usize) -> Result<Pack> {
    let Some(obj) = pack.as_object() else {
        return invalid("pack must be an object");
    };
    if bytes > PackLimits::PACK_BYTES {
        return invalid("pack exceeds size limit");
    }
    check_keys(
        obj,
        if builtin { &PACK_KEYS } else { &USER_PACK_KEYS },
        "pack",
    )?;
    if pack.get("schema_version").and_then(Json::as_f64) != Some(1.0) {
        return invalid("unsupported schema_version (migration required)");
    }
    let profile_id = match pack.get("profile_id").and_then(Json::as_str) {
        Some(p) if LANGUAGES.contains(&p) => p.to_string(),
        _ => return invalid("unknown profile_id"),
    };
    let version = match pack.get("version").and_then(Json::as_str) {
        Some(v) if is_semver(v) => v.to_string(),
        _ => return invalid("version must be semver"),
    };
    let trigger_list: &[Json] = match pack.get("triggers") {
        None | Some(Json::Null) => &[],
        Some(Json::Arr(a)) if a.len() <= PackLimits::TRIGGERS => a,
        _ => return invalid("too many triggers"),
    };
    let keyword_list: &[Json] = match pack.get("keywords") {
        None | Some(Json::Null) => &[],
        Some(Json::Arr(a)) if a.len() <= PackLimits::KEYWORDS => a,
        _ => return invalid("invalid keywords"),
    };
    let mut keywords = Vec::new();
    for k in keyword_list {
        match k.as_str() {
            Some(s)
                if in_class(s, 1, 64, |c| {
                    c.is_ascii_alphanumeric() || matches!(c, '_' | '#' | '.' | '$' | '-')
                }) =>
            {
                keywords.push(s.to_string())
            }
            _ => return invalid("invalid keywords"),
        }
    }
    let mut ids = HashSet::new();
    let triggers = trigger_list
        .iter()
        .map(|t| parse_trigger(t, &mut ids, !builtin))
        .collect::<Result<Vec<_>>>()?;
    let mut out = Pack {
        profile_id,
        version,
        display_name: pack
            .get("display_name")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
        language_ids: Vec::new(),
        accepted_extensions: Vec::new(),
        dialects: Vec::new(),
        keywords,
        triggers,
    };
    if builtin {
        let (Some(ids), Some(exts), Some(dialects)) = (
            pack.get("language_ids").and_then(string_list),
            pack.get("accepted_extensions").and_then(string_list),
            pack.get("dialects").and_then(Json::as_object),
        ) else {
            return invalid("incomplete built-in pack");
        };
        out.language_ids = ids;
        out.accepted_extensions = exts;
        for (axis, def) in dialects {
            let values = def.get("values").and_then(string_list);
            let default = def.get("default").and_then(Json::as_str);
            match (values, default) {
                (Some(values), Some(default)) if values.iter().any(|v| v == default) => {
                    out.dialects.push(DialectAxis {
                        name: axis.clone(),
                        values,
                        default: default.to_string(),
                    });
                }
                _ => {
                    return invalid(format!(
                        "dialect axis {axis} needs a default among its values"
                    ))
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- registry

const BUILTIN_SOURCES: [(&str, &str); 7] = [
    ("rust", include_str!("../../../language-packs/rust.json")),
    (
        "python",
        include_str!("../../../language-packs/python.json"),
    ),
    (
        "typescript",
        include_str!("../../../language-packs/typescript.json"),
    ),
    (
        "powershell",
        include_str!("../../../language-packs/powershell.json"),
    ),
    ("c", include_str!("../../../language-packs/c.json")),
    (
        "assembly",
        include_str!("../../../language-packs/assembly.json"),
    ),
    (
        "machine-code",
        include_str!("../../../language-packs/machine-code.json"),
    ),
];

#[derive(Clone, Debug, Default)]
pub struct Registry {
    packs: Vec<Pack>,
    user_triggers: HashMap<String, Vec<Trigger>>,
    user_keywords: HashMap<String, Vec<String>>,
    by_language_id: HashMap<String, String>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// The seven built-in packs compiled into the binary.
    pub fn builtin() -> Result<Registry> {
        let mut reg = Registry::new();
        for (id, raw) in BUILTIN_SOURCES {
            let parsed = json::parse(
                raw,
                Limits {
                    max_bytes: PackLimits::PACK_BYTES,
                    max_depth: 16,
                },
            );
            let Ok(value) = parsed else {
                return invalid(format!("{id}.json is not valid JSON"));
            };
            if value.get("profile_id").and_then(Json::as_str) != Some(id) {
                return invalid(format!("{id}.json declares another profile"));
            }
            reg.add(&value, raw.len())?;
        }
        Ok(reg)
    }

    pub fn add(&mut self, pack: &Json, bytes: usize) -> Result<()> {
        let pack = parse_pack(pack, true, bytes)?;
        if self.get(&pack.profile_id).is_some() {
            return invalid(format!("duplicate profile {}", pack.profile_id));
        }
        for id in &pack.language_ids {
            self.by_language_id
                .insert(id.clone(), pack.profile_id.clone());
        }
        self.packs.push(pack);
        Ok(())
    }

    /// User packs only add triggers/keywords; they cannot replace or remove built-in behaviour.
    pub fn add_user(&mut self, pack: &Json, bytes: usize) -> Result<()> {
        let pack = parse_pack(pack, false, bytes)?;
        let Some(base) = self.get(&pack.profile_id) else {
            return invalid("user pack targets an unloaded profile");
        };
        let mut taken: HashSet<&str> = base.triggers.iter().map(|t| t.id.as_str()).collect();
        if let Some(list) = self.user_triggers.get(&pack.profile_id) {
            taken.extend(list.iter().map(|t| t.id.as_str()));
        }
        for t in &pack.triggers {
            if taken.contains(t.id.as_str()) {
                return invalid(format!(
                    "trigger id collides with an existing rule: {}",
                    t.id
                ));
            }
        }
        self.user_triggers
            .entry(pack.profile_id.clone())
            .or_default()
            .extend(pack.triggers);
        self.user_keywords
            .entry(pack.profile_id)
            .or_default()
            .extend(pack.keywords);
        Ok(())
    }

    pub fn clear_user(&mut self) {
        self.user_triggers.clear();
        self.user_keywords.clear();
    }

    pub fn get(&self, profile: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.profile_id == profile)
    }

    pub fn language_for_id(&self, language_id: &str) -> Option<&str> {
        self.by_language_id.get(language_id).map(String::as_str)
    }

    pub fn profiles(&self) -> Vec<&str> {
        self.packs.iter().map(|p| p.profile_id.as_str()).collect()
    }

    /// Built-in triggers followed by user triggers.
    pub fn triggers(&self, profile: &str) -> Vec<&Trigger> {
        let mut out: Vec<&Trigger> = self
            .get(profile)
            .map(|p| p.triggers.iter().collect())
            .unwrap_or_default();
        if let Some(u) = self.user_triggers.get(profile) {
            out.extend(u.iter());
        }
        out
    }

    pub fn keywords(&self, profile: &str) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .get(profile)
            .map(|p| p.keywords.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if let Some(u) = self.user_keywords.get(profile) {
            out.extend(u.iter().map(String::as_str));
        }
        out
    }

    /// `resolveDialect`: every axis gets a value; unsupported requests fall back to the default and are reported.
    pub fn resolve_dialect(
        &self,
        profile: &str,
        requested: &Dialect,
    ) -> (Dialect, Vec<(String, String)>) {
        let mut values = Dialect::new();
        let mut unsupported = Vec::new();
        if let Some(pack) = self.get(profile) {
            for axis in &pack.dialects {
                match requested.get(&axis.name) {
                    None => {
                        values.insert(axis.name.clone(), axis.default.clone());
                    }
                    Some(want) if want.is_empty() => {
                        values.insert(axis.name.clone(), axis.default.clone());
                    }
                    Some(want) if axis.values.contains(want) => {
                        values.insert(axis.name.clone(), want.clone());
                    }
                    Some(want) => {
                        values.insert(axis.name.clone(), axis.default.clone());
                        unsupported.push((axis.name.clone(), want.clone()));
                    }
                }
            }
        }
        (values, unsupported)
    }

    pub fn default_dialect(&self, profile: &str) -> Dialect {
        self.resolve_dialect(profile, &Dialect::new()).0
    }
}

pub fn dialect_matches(when: &When, dialect: &Dialect) -> bool {
    when.iter()
        .all(|(axis, allowed)| dialect.get(axis).is_some_and(|v| allowed.contains(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Json {
        json::parse(s, Limits::DEFAULT).unwrap()
    }

    fn user_pack(extra: &str) -> Json {
        parse(&format!(
            r#"{{"schema_version":1,"profile_id":"rust","version":"1.0.0"{extra}}}"#
        ))
    }

    #[test]
    fn builtin_packs_load_and_cover_seven_languages() {
        let reg = Registry::builtin().unwrap();
        assert_eq!(reg.profiles(), LANGUAGES.to_vec());
        assert_eq!(reg.language_for_id("typescriptreact"), Some("typescript"));
        assert_eq!(reg.language_for_id("nasm"), Some("assembly"));
        assert!(!reg.triggers("rust").is_empty());
        assert!(reg.keywords("python").contains(&"def"));
    }

    #[test]
    fn unknown_keys_are_rejected_in_every_position() {
        let reg_err = |s: &str| parse_pack(&parse(s), true, 0).unwrap_err().code;
        assert_eq!(
            reg_err(r#"{"schema_version":1,"profile_id":"rust","version":"1.0.0","script":"x"}"#),
            "RULEPACK_INVALID"
        );
        let t = r#"{"id":"a","pattern":"a","exec":"calc","candidates":[{"label":"l","text":"t"}]}"#;
        assert!(parse_pack(&user_pack(&format!(r#","triggers":[{t}]"#)), false, 0).is_err());
        let c = r#"{"id":"a","pattern":"a","candidates":[{"label":"l","text":"t","run":"x"}]}"#;
        assert!(parse_pack(&user_pack(&format!(r#","triggers":[{c}]"#)), false, 0).is_err());
    }

    #[test]
    fn limits_and_placeholders_are_enforced() {
        let big = "x".repeat(4001);
        let long = format!(
            r#","triggers":[{{"id":"a","pattern":"a","candidates":[{{"label":"l","text":"{big}"}}]}}]"#
        );
        assert!(parse_pack(&user_pack(&long), false, 0).is_err());
        let bad_placeholder = r#","triggers":[{"id":"a","pattern":"a","candidates":[{"label":"l","text":"run ${rm -rf} ${cursor}"}]}]"#;
        assert!(parse_pack(&user_pack(bad_placeholder), false, 0).is_err());
        let ok = r#","triggers":[{"id":"a","pattern":"a","weight":5,"candidates":[{"label":"l","text":"x ${name} ${cursor}"}]}]"#;
        assert!(parse_pack(&user_pack(ok), false, 0).is_ok());
        assert!(
            parse_pack(&user_pack(ok), false, 300_000).is_err(),
            "size limit"
        );
        let weight = r#","triggers":[{"id":"a","pattern":"a","weight":101,"candidates":[{"label":"l","text":"x"}]}]"#;
        assert!(parse_pack(&user_pack(weight), false, 0).is_err());
        let pattern =
            r#","triggers":[{"id":"a","pattern":"a b","candidates":[{"label":"l","text":"x"}]}]"#;
        assert!(parse_pack(&user_pack(pattern), false, 0).is_err());
    }

    #[test]
    fn user_packs_cannot_use_builtin_only_fields_or_collide() {
        assert!(parse_pack(&user_pack(r#","dialects":{}"#), false, 0).is_err());
        assert!(parse_pack(&user_pack(r#","display_name":"x""#), false, 0).is_err());
        let mut reg = Registry::builtin().unwrap();
        let collide = r#","triggers":[{"id":"rust.main","pattern":"zz","candidates":[{"label":"l","text":"x"}]}]"#;
        assert!(reg.add_user(&user_pack(collide), 0).is_err());
        let fresh = r#","keywords":["mykw"],"triggers":[{"id":"user.zz","pattern":"zz","candidates":[{"label":"l","text":"x"}]}]"#;
        reg.add_user(&user_pack(fresh), 0).unwrap();
        assert!(reg.triggers("rust").last().unwrap().user);
        assert!(reg.keywords("rust").contains(&"mykw"));
        reg.clear_user();
        assert!(!reg.keywords("rust").contains(&"mykw"));
        let unloaded = parse(r#"{"schema_version":1,"profile_id":"rust","version":"1.0.0"}"#);
        assert!(Registry::new().add_user(&unloaded, 0).is_err());
    }

    #[test]
    fn schema_version_profile_and_semver_checks() {
        for bad in [
            r#"{"schema_version":7,"profile_id":"rust","version":"1.0.0"}"#,
            r#"{"schema_version":1,"profile_id":"cobol","version":"1.0.0"}"#,
            r#"{"schema_version":1,"profile_id":"rust","version":"1.0"}"#,
            r#"{"profile_id":"rust","version":"1.0.0"}"#,
            "[]",
        ] {
            assert_eq!(
                parse_pack(&parse(bad), false, 0).unwrap_err().code,
                "RULEPACK_INVALID",
                "{bad}"
            );
        }
        let mut reg = Registry::builtin().unwrap();
        let rust = parse(BUILTIN_SOURCES[0].1);
        assert!(reg.add(&rust, 10).is_err(), "duplicate profile");
    }

    #[test]
    fn dialect_resolution_defaults_and_reports_unsupported() {
        let reg = Registry::builtin().unwrap();
        let mut req = Dialect::new();
        req.insert("edition".into(), "2018".into());
        let (values, unsupported) = reg.resolve_dialect("rust", &req);
        assert_eq!(values["edition"], "2024");
        assert_eq!(
            unsupported,
            vec![("edition".to_string(), "2018".to_string())]
        );
        req.insert("edition".into(), "2021".into());
        assert_eq!(reg.resolve_dialect("rust", &req).0["edition"], "2021");
        assert_eq!(render_plain("a ${name} ${cursor}", "X"), "a name X");
    }
}
