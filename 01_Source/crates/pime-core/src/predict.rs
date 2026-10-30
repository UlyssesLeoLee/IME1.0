//! Context-aware deterministic candidate engine (FR-006, FR-025/026, DD-03): port of `predict` from `languages.js`.
//! No network, no execution: candidates are text only.

use crate::analysis::{classify_context, enclosing_block, nearby_identifiers};
use crate::candidate::{
    dedupe, rank_candidates, Candidate, CandidateKind, CandidateSource, MatchKind, RankContext,
    Validation,
};
use crate::error::Result;
use crate::pack::{dialect_matches, render_plain, Context, Registry, LANGUAGES};
use crate::util::{is_js_space, js_trim, utf16_len};
use crate::validators::{validate_source, Dialect};
use crate::x86;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

pub const MAX_PREFIX: usize = 128;
pub const DEFAULT_LIMIT: usize = 6;

#[derive(Clone, Debug)]
pub struct PredictOptions {
    pub profile: String,
    pub prefix: String,
    pub before: String,
    pub after: String,
    pub dialect: Dialect,
    pub limit: usize,
    pub pins: HashSet<String>,
    pub usage: HashMap<String, i64>,
}

impl PredictOptions {
    pub fn new(profile: &str, prefix: &str) -> PredictOptions {
        PredictOptions {
            profile: profile.to_string(),
            prefix: prefix.to_string(),
            before: String::new(),
            after: String::new(),
            dialect: Dialect::new(),
            limit: DEFAULT_LIMIT,
            pins: HashSet::new(),
            usage: HashMap::new(),
        }
    }

    pub fn before(mut self, before: &str) -> Self {
        self.before = before.to_string();
        self
    }

    pub fn after(mut self, after: &str) -> Self {
        self.after = after.to_string();
        self
    }

    pub fn dialect(mut self, axis: &str, value: &str) -> Self {
        self.dialect.insert(axis.to_string(), value.to_string());
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// The engine: a registry plus a template-validation cache (like the JS `validationCache`).
pub struct Engine {
    registry: Registry,
    cache: Mutex<HashMap<String, bool>>,
}

/// Process-wide engine over the built-in packs.
pub fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::new(Registry::builtin().expect("built-in language packs are valid"))
    })
}

fn match_pattern(pattern: &str, key: &str, label: &str) -> Option<MatchKind> {
    let p = pattern.to_lowercase();
    if p == key {
        return Some(MatchKind::Exact);
    }
    if p.starts_with(key) {
        return Some(MatchKind::Prefix);
    }
    let key_len = utf16_len(key);
    if key_len >= 2 {
        let lower = label.to_lowercase();
        if lower
            .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
            .any(|w| !w.is_empty() && w.starts_with(key))
        {
            return Some(MatchKind::Keyword);
        }
        if key_len < p.chars().count() && key.chars().next() == p.chars().next() {
            let k: Vec<char> = key.chars().collect();
            let mut i = 0;
            for ch in p.chars() {
                if i < k.len() && ch == k[i] {
                    i += 1;
                }
            }
            if i == k.len() {
                return Some(MatchKind::Fuzzy);
            }
        }
    }
    None
}

fn explain_candidate(
    id: &str,
    label: String,
    explain: &str,
    validation: Validation,
    profile: &str,
    assumptions: Vec<String>,
) -> Candidate {
    let mut c = Candidate::new(id);
    c.kind = CandidateKind::Explain;
    c.label = label;
    c.explain = explain.to_string();
    c.validation = validation;
    c.profile_id = profile.to_string();
    c.match_kind = Some(MatchKind::Exact);
    c.assumptions = assumptions;
    c
}

impl Engine {
    pub fn new(registry: Registry) -> Engine {
        Engine {
            registry,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    fn validate_template(&self, profile: &str, text: &str, dialect: &Dialect) -> Validation {
        let dialect_key: String = dialect.iter().map(|(k, v)| format!("{k}={v};")).collect();
        let key = format!("{profile}|{dialect_key}|{text}");
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .copied();
        let ok = match cached {
            Some(ok) => ok,
            None => {
                let body = if profile == "python" { "pass" } else { "" };
                let ok = validate_source(profile, &render_plain(text, body), dialect).ok;
                let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() > 5000 {
                    cache.clear();
                }
                cache.insert(key, ok);
                ok
            }
        };
        if ok {
            Validation::Valid
        } else {
            Validation::Invalid
        }
    }

    fn predict_machine(&self, prefix: &str, dialect: &Dialect, limit: usize) -> Vec<Candidate> {
        let key = x86::normalize(prefix);
        if key.is_empty() {
            return Vec::new();
        }
        let compact = dialect.get("format").is_some_and(|f| f == "compact");
        let mut out: Vec<Candidate> = Vec::new();
        for e in x86::list_encodable() {
            if !e.asm.starts_with(&key) {
                continue;
            }
            let mut c = Candidate::new(&format!("mc.{}", e.asm));
            c.kind = CandidateKind::ByteHex;
            c.label = format!("{}  \u{2192}  {}", e.asm, e.hex);
            c.text = Some(if compact {
                e.hex.replace(' ', "")
            } else {
                e.hex.clone()
            });
            c.explain =
                "x86-64 long mode; verified against the encoding table; text only, never executed"
                    .into();
            c.validation = Validation::Verified;
            c.profile_id = "machine-code".into();
            c.match_kind = Some(if e.asm == key {
                MatchKind::Exact
            } else {
                MatchKind::Prefix
            });
            c.weight = 100 - 90.min(e.asm.chars().count() as i64);
            c.assumptions = vec!["x86-64 long mode".into()];
            out.push(c);
            if out.len() >= 60 {
                break;
            }
        }
        let is_hex_bytes = is_hex_pairs(&key);
        if out.is_empty() && !is_hex_bytes {
            if let x86::EncodeResult::Err { code, reason } = x86::encode(&key) {
                out.push(explain_candidate(
                    &format!("mc.unresolved:{key}"),
                    format!("{key}: {code}"),
                    reason,
                    Validation::ContextMissing,
                    "machine-code",
                    vec![reason.to_string()],
                ));
            }
        }
        if is_hex_bytes {
            if let Some(asm) = x86::disassemble_known(&key) {
                out.push(explain_candidate(
                    &format!("mc.disasm:{key}"),
                    format!("{}  =  {}", key.to_uppercase(), asm),
                    "Known-form disassembly preview",
                    Validation::Verified,
                    "machine-code",
                    Vec::new(),
                ));
            }
        }
        out.truncate(limit);
        out
    }

    /// Ranked, de-duplicated candidates for `opts` (never more than `opts.limit`).
    pub fn predict(&self, opts: &PredictOptions) -> Result<Vec<Candidate>> {
        let reg = &self.registry;
        let profile = opts.profile.as_str();
        let key = js_trim(&opts.prefix).to_lowercase();
        if key.is_empty() || utf16_len(&key) > MAX_PREFIX || !LANGUAGES.contains(&profile) {
            return Ok(Vec::new());
        }
        let (dialect, unsupported) = reg.resolve_dialect(profile, &opts.dialect);
        if let Some((axis, value)) = unsupported.first() {
            // unknown ISA/dialect: say so instead of guessing bytes or syntax
            return Ok(vec![explain_candidate(
                &format!("{profile}.dialect"),
                format!("{axis} \"{value}\" is not supported"),
                "Pick a supported value (for example isa x86-64) to get candidates",
                Validation::ContextMissing,
                profile,
                vec!["dialect context missing".into()],
            )]);
        }
        let rank = RankContext {
            pins: opts.pins.clone(),
            usage: opts.usage.clone(),
        };
        if profile == "machine-code" {
            let mut list = rank_candidates(
                &self.predict_machine(&opts.prefix, &dialect, opts.limit.saturating_mul(2)),
                &rank,
            );
            list.truncate(opts.limit);
            return Ok(list);
        }
        let ctx = classify_context(profile, &opts.before);
        if ctx == Context::Literal {
            return Ok(Vec::new()); // inside a string or comment nothing is proposed
        }
        let block = enclosing_block(profile, &opts.before);
        let mut out: Vec<Candidate> = Vec::new();
        for t in reg.triggers(profile) {
            if !dialect_matches(&t.when, &dialect) {
                continue;
            }
            if t.contexts.as_ref().is_some_and(|c| !c.contains(&ctx)) {
                continue;
            }
            if t.in_blocks
                .as_ref()
                .is_some_and(|b| !b.iter().any(|x| x == block))
            {
                continue;
            }
            if t.not_in
                .as_ref()
                .is_some_and(|b| b.iter().any(|x| x == block))
            {
                continue;
            }
            for (i, c) in t.candidates.iter().enumerate() {
                if !dialect_matches(&c.when, &dialect) {
                    continue;
                }
                let Some(matched) = match_pattern(&t.pattern, &key, &c.label) else {
                    continue;
                };
                let mut text = c.text.clone();
                let mut assumptions = c.assumptions.clone();
                if c.stmt_semicolon && ctx != Context::Statement && text.ends_with(';') {
                    text.pop();
                    assumptions.push(if ctx == Context::Unknown {
                        "No semicolon added: statement position not provable".to_string()
                    } else {
                        "No semicolon: expression position".to_string()
                    });
                }
                let mut cand = Candidate::new(&format!("{}#{}", t.id, i)).with_source(if t.user {
                    CandidateSource::UserRule
                } else {
                    CandidateSource::Rule
                });
                cand.kind = t.kind;
                cand.profile_id = profile.to_string();
                cand.label = c.label.clone();
                cand.explain = c.explain.clone();
                cand.match_kind = Some(matched);
                cand.weight = t.weight.unwrap_or(50);
                cand.assumptions = assumptions;
                cand.validation = self.validate_template(profile, &text, &dialect);
                cand.text = Some(text);
                out.push(cand);
            }
        }
        for kw in reg.keywords(profile) {
            let lower = kw.to_lowercase();
            if !lower.starts_with(&key) {
                continue;
            }
            let mut c = Candidate::new(&format!("{profile}.kw.{kw}"));
            c.kind = CandidateKind::Keyword;
            c.profile_id = profile.to_string();
            c.label = kw.to_string();
            c.text = Some(kw.to_string());
            c.match_kind = Some(if lower == key {
                MatchKind::Exact
            } else {
                MatchKind::Prefix
            });
            c.weight = 20;
            c.validation = Validation::Valid;
            out.push(c);
        }
        if !opts.before.is_empty() || !opts.after.is_empty() {
            let joined = format!("{}{}", opts.before, opts.after);
            for (i, word) in nearby_identifiers(reg, profile, &joined, js_trim(&opts.prefix), 3)
                .into_iter()
                .enumerate()
            {
                let mut c = Candidate::new(&format!("{profile}.id.{word}"))
                    .with_source(CandidateSource::Context);
                c.kind = CandidateKind::Identifier;
                c.profile_id = profile.to_string();
                c.label = word.clone();
                c.text = Some(word);
                c.explain = "Seen in nearby code".into();
                c.match_kind = Some(MatchKind::Context);
                c.weight = 30 - i as i64;
                c.validation = Validation::Unchecked;
                c.assumptions = vec!["Only nearby text was read; no project symbol table".into()];
                out.push(c);
            }
        }
        out.retain(|c| c.validation != Validation::Invalid);
        let mut list = dedupe(rank_candidates(&out, &rank), profile);
        list.truncate(opts.limit);
        Ok(list)
    }
}

/// `/^(?:[0-9a-f]{2}\s*)+$/i`
fn is_hex_pairs(key: &str) -> bool {
    let c: Vec<char> = key.chars().collect();
    let mut i = 0;
    let mut groups = 0;
    while i < c.len() {
        if i + 1 < c.len() && c[i].is_ascii_hexdigit() && c[i + 1].is_ascii_hexdigit() {
            i += 2;
            groups += 1;
            while i < c.len() && is_js_space(c[i]) {
                i += 1;
            }
        } else {
            return false;
        }
    }
    groups > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(profile: &str, prefix: &str) -> Vec<String> {
        engine()
            .predict(&PredictOptions::new(profile, prefix))
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    #[test]
    fn rust_prefix_candidates() {
        assert_eq!(ids("rust", "fnm"), vec!["rust.main#0"]);
        assert_eq!(ids("rust", "f")[0], "rust.main#0");
        assert!(ids("rust", "").is_empty());
        assert!(ids("unknown", "fn").is_empty());
    }

    #[test]
    fn literal_positions_yield_nothing() {
        let o = PredictOptions::new("rust", "fn").before("let s = \"abc ");
        assert!(engine().predict(&o).unwrap().is_empty());
    }

    #[test]
    fn unsupported_dialect_is_explained_not_guessed() {
        let o = PredictOptions::new("machine-code", "nop").dialect("isa", "arm64");
        let r = engine().predict(&o).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].id.as_str(), r[0].validation, r[0].text.clone()),
            ("machine-code.dialect", Validation::ContextMissing, None)
        );
    }

    #[test]
    fn machine_code_is_verified_or_explained() {
        let r = engine()
            .predict(&PredictOptions::new("machine-code", "mov rax, rbx"))
            .unwrap();
        assert_eq!(r[0].text.as_deref(), Some("48 89 D8"));
        assert_eq!(r[0].validation, Validation::Verified);
        let r = engine()
            .predict(&PredictOptions::new("machine-code", "jmp foo"))
            .unwrap();
        assert_eq!(
            (r[0].kind, r[0].text.clone(), r[0].validation),
            (CandidateKind::Explain, None, Validation::ContextMissing)
        );
    }

    #[test]
    fn identifiers_come_from_nearby_text() {
        let o = PredictOptions::new("rust", "cou")
            .before("let counter = 1;\nlet count_total = counter + 2;\n");
        let r = engine().predict(&o).unwrap();
        assert!(r
            .iter()
            .any(|c| c.id == "rust.id.counter" && c.source == CandidateSource::Context));
    }
}
