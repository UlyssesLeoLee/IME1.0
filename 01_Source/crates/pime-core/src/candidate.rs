//! Candidate model (FR-006, DD-02 section 4.2): closed enums with a strict string round trip, deterministic
//! ranking and de-duplication that keeps every contributing source (UT-006, UT-007, UT-008).

use crate::error::{PimeError, Result, fail};
use crate::json::Json;
use crate::pack::render_plain;
use crate::util::js_trim;
use std::collections::{HashMap, HashSet};

macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
            pub fn parse(s: &str) -> Option<$name> {
                match s { $($text => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

string_enum!(
    /// What a candidate is (UT-006: every kind round-trips, unknown kinds are rejected).
    CandidateKind {
        Keyword => "keyword",
        Syntax => "syntax",
        Prefix => "prefix",
        Identifier => "identifier",
        AiInline => "ai_inline",
        FullFilePatch => "full_file_patch",
        ByteHex => "byte_hex",
        Explain => "explain",
    }
);

string_enum!(
    CandidateSource {
        Rule => "rule",
        UserRule => "user_rule",
        Context => "context",
        Ai => "ai",
    }
);

string_enum!(
    Validation {
        Valid => "valid",
        Verified => "verified",
        Unchecked => "unchecked",
        Invalid => "invalid",
        ContextMissing => "context_missing",
    }
);

string_enum!(
    MatchKind {
        Exact => "exact",
        Prefix => "prefix",
        Keyword => "keyword",
        Fuzzy => "fuzzy",
        Context => "context",
    }
);

impl MatchKind {
    fn rank(m: Option<MatchKind>) -> u8 {
        match m {
            Some(MatchKind::Exact) => 0,
            Some(MatchKind::Prefix) => 1,
            Some(MatchKind::Keyword) => 2,
            Some(MatchKind::Fuzzy) => 3,
            Some(MatchKind::Context) => 4,
            None => 9,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub kind: CandidateKind,
    pub source: CandidateSource,
    /// Every source that produced this content (after de-duplication); always contains `source`.
    pub sources: Vec<CandidateSource>,
    pub profile_id: String,
    pub label: String,
    pub text: Option<String>,
    pub explain: String,
    pub validation: Validation,
    pub assumptions: Vec<String>,
    pub match_kind: Option<MatchKind>,
    pub weight: i64,
    pub ai_score: f64,
}

impl Candidate {
    /// Defaults of `makeCandidate`: syntax / rule / unchecked.
    pub fn new(id: &str) -> Candidate {
        Candidate {
            id: id.to_string(),
            kind: CandidateKind::Syntax,
            source: CandidateSource::Rule,
            sources: vec![CandidateSource::Rule],
            profile_id: String::new(),
            label: String::new(),
            text: None,
            explain: String::new(),
            validation: Validation::Unchecked,
            assumptions: Vec::new(),
            match_kind: None,
            weight: 0,
            ai_score: 0.0,
        }
    }

    pub fn with_source(mut self, source: CandidateSource) -> Candidate {
        self.source = source;
        self.sources = vec![source];
        self
    }

    /// Protocol 0.2 candidate object.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("id", Json::str(&self.id)),
            ("kind", Json::str(self.kind.as_str())),
            ("source", Json::str(self.source.as_str())),
            (
                "sources",
                Json::Arr(self.sources.iter().map(|s| Json::str(s.as_str())).collect()),
            ),
            ("profile_id", Json::str(&self.profile_id)),
            ("label", Json::str(&self.label)),
            ("text", self.text.as_deref().map_or(Json::Null, Json::str)),
            ("explain", Json::str(&self.explain)),
            ("validation", Json::str(self.validation.as_str())),
            ("assumptions", Json::strs(&self.assumptions)),
            (
                "match",
                self.match_kind
                    .map_or(Json::Null, |m| Json::str(m.as_str())),
            ),
        ])
    }

    /// Strict inverse of [`Candidate::to_json`]; an invalid kind, source, validation or match is rejected
    /// with `OUTPUT_INVALID` (UT-006).
    pub fn from_json(v: &Json) -> Result<Candidate> {
        let bad = |what: &str| -> PimeError { PimeError::new("OUTPUT_INVALID", what.to_string()) };
        let Some(id) = v.get("id").and_then(Json::as_str) else {
            return fail("OUTPUT_INVALID", "candidate shape");
        };
        let mut c = Candidate::new(id);
        if let Some(k) = v.get("kind") {
            c.kind = k
                .as_str()
                .and_then(CandidateKind::parse)
                .ok_or_else(|| bad("invalid candidate kind"))?;
        }
        if let Some(s) = v.get("source") {
            c.source = s
                .as_str()
                .and_then(CandidateSource::parse)
                .ok_or_else(|| bad("invalid candidate source"))?;
            c.sources = vec![c.source];
        }
        if let Some(Json::Arr(list)) = v.get("sources") {
            let mut sources = Vec::new();
            for s in list {
                sources.push(
                    s.as_str()
                        .and_then(CandidateSource::parse)
                        .ok_or_else(|| bad("invalid candidate source"))?,
                );
            }
            if !sources.is_empty() {
                c.sources = sources;
            }
        }
        if let Some(x) = v.get("validation") {
            c.validation = x
                .as_str()
                .and_then(Validation::parse)
                .ok_or_else(|| bad("invalid candidate validation"))?;
        }
        match v.get("match") {
            None | Some(Json::Null) => {}
            Some(m) => {
                c.match_kind = Some(
                    m.as_str()
                        .and_then(MatchKind::parse)
                        .ok_or_else(|| bad("invalid candidate match"))?,
                )
            }
        }
        match v.get("text") {
            None | Some(Json::Null) => {}
            Some(Json::Str(s)) => c.text = Some(s.clone()),
            Some(_) => return fail("OUTPUT_INVALID", "candidate text"),
        }
        for (key, slot) in [
            ("profile_id", &mut c.profile_id),
            ("label", &mut c.label),
            ("explain", &mut c.explain),
        ] {
            match v.get(key) {
                None => {}
                Some(Json::Str(s)) => *slot = s.clone(),
                Some(_) => return fail("OUTPUT_INVALID", "candidate shape"),
            }
        }
        match v.get("assumptions") {
            None => {}
            Some(Json::Arr(list)) => {
                for a in list {
                    c.assumptions.push(
                        a.as_str()
                            .ok_or_else(|| bad("candidate assumptions"))?
                            .to_string(),
                    );
                }
            }
            Some(_) => return fail("OUTPUT_INVALID", "candidate assumptions"),
        }
        Ok(c)
    }
}

pub fn candidate_to_json(c: &Candidate) -> String {
    c.to_json().to_json_string()
}

pub fn candidate_from_json(text: &str) -> Result<Candidate> {
    match crate::json::parse(
        text,
        crate::json::Limits {
            max_bytes: 1 << 20,
            max_depth: 16,
        },
    ) {
        Ok(v) => Candidate::from_json(&v),
        Err(_) => fail("OUTPUT_INVALID", "candidate is not JSON"),
    }
}

/// Inputs of the ranking that do not live on the candidate: explicit pins and local usage counts.
#[derive(Clone, Debug, Default)]
pub struct RankContext {
    pub pins: HashSet<String>,
    pub usage: HashMap<String, i64>,
}

fn validity(c: &Candidate) -> u8 {
    match c.validation {
        Validation::Valid | Validation::Verified => 2,
        Validation::Unchecked => 1,
        _ => 0,
    }
}

/// Candidate ordering (DD section 4.2): pin > explicit user rule > syntax-valid > match quality > frequency >
/// weight > AI score > id. The sort is stable and total, so repeated ranking gives identical results.
pub fn rank_candidates(list: &[Candidate], ctx: &RankContext) -> Vec<Candidate> {
    let mut out = list.to_vec();
    out.sort_by(|a, b| {
        ctx.pins
            .contains(&b.id)
            .cmp(&ctx.pins.contains(&a.id))
            .then(
                (b.source == CandidateSource::UserRule)
                    .cmp(&(a.source == CandidateSource::UserRule)),
            )
            .then(validity(b).cmp(&validity(a)))
            .then(MatchKind::rank(a.match_kind).cmp(&MatchKind::rank(b.match_kind)))
            .then(
                ctx.usage
                    .get(&b.id)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&ctx.usage.get(&a.id).copied().unwrap_or(0)),
            )
            .then(b.weight.cmp(&a.weight))
            .then(
                b.ai_score
                    .partial_cmp(&a.ai_score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.id.cmp(&b.id))
    });
    out
}

/// `fingerprint`: profile plus the normalised rendered text.
pub fn fingerprint(profile: &str, text: &str) -> String {
    let rendered = render_plain(text, "");
    let mut norm = String::new();
    let mut in_ws = false;
    for c in js_trim(&rendered).to_lowercase().chars() {
        if crate::util::is_js_space(c) {
            if !in_ws {
                norm.push(' ');
            }
            in_ws = true;
        } else {
            norm.push(c);
            in_ws = false;
        }
    }
    format!("{profile}|{norm}")
}

/// One row per normalised content; every contributing source stays visible in `sources` (UT-008).
pub fn dedupe(list: Vec<Candidate>, profile: &str) -> Vec<Candidate> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Candidate> = Vec::new();
    for c in list {
        let Some(text) = c.text.as_deref() else {
            out.push(c);
            continue;
        };
        let p = if profile.is_empty() {
            c.profile_id.as_str()
        } else {
            profile
        };
        let key = fingerprint(p, text);
        match seen.get(&key) {
            Some(&idx) => {
                for s in &c.sources {
                    if !out[idx].sources.contains(s) {
                        out[idx].sources.push(*s);
                    }
                }
            }
            None => {
                seen.insert(key, out.len());
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, f: impl FnOnce(&mut Candidate)) -> Candidate {
        let mut c = Candidate::new(id);
        c.validation = Validation::Valid;
        c.match_kind = Some(MatchKind::Prefix);
        c.weight = 50;
        f(&mut c);
        c
    }

    #[test]
    fn ut_006_all_kinds_sources_validations_round_trip() {
        for &kind in CandidateKind::ALL {
            for &source in CandidateSource::ALL {
                for &validation in Validation::ALL {
                    let mut c = Candidate::new("x#0").with_source(source);
                    c.kind = kind;
                    c.validation = validation;
                    c.text = Some("fn main() {}".into());
                    c.match_kind = Some(MatchKind::Exact);
                    c.assumptions = vec!["a".into()];
                    let back = candidate_from_json(&candidate_to_json(&c)).unwrap();
                    assert_eq!(back.kind, kind);
                    assert_eq!(back.source, source);
                    assert_eq!(back.validation, validation);
                    assert_eq!(back.text, c.text);
                    assert_eq!(back.match_kind, c.match_kind);
                }
            }
        }
        assert_eq!(CandidateKind::ALL.len(), 8);
        for k in [
            "keyword",
            "syntax",
            "ai_inline",
            "full_file_patch",
            "byte_hex",
        ] {
            assert!(CandidateKind::parse(k).is_some(), "{k}");
        }
    }

    #[test]
    fn ut_006_invalid_values_are_rejected() {
        for bad in [
            r#"{"id":"a","kind":"shell_command"}"#,
            r#"{"id":"a","source":"network"}"#,
            r#"{"id":"a","validation":"maybe"}"#,
            r#"{"id":"a","match":"vibes"}"#,
            r#"{"kind":"syntax"}"#,
            r#"{"id":5}"#,
            "not json",
        ] {
            assert_eq!(
                candidate_from_json(bad).unwrap_err().code,
                "OUTPUT_INVALID",
                "{bad}"
            );
        }
    }

    #[test]
    fn ut_007_pin_and_deterministic_rules_first_and_ranking_is_stable() {
        let list = vec![
            mk("b", |_| {}),
            mk("a", |_| {}),
            mk("ai1", |c| {
                c.source = CandidateSource::Ai;
                c.kind = CandidateKind::AiInline;
                c.validation = Validation::Unchecked;
                c.ai_score = 99.0;
            }),
            mk("pinned", |c| c.weight = 1),
            mk("user", |c| {
                c.source = CandidateSource::UserRule;
                c.weight = 1;
            }),
        ];
        let ctx = RankContext {
            pins: HashSet::from(["pinned".to_string()]),
            usage: HashMap::new(),
        };
        let first: Vec<String> = rank_candidates(&list, &ctx)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(first[0], "pinned", "an explicit pin beats everything");
        assert_eq!(first[1], "user", "an explicit user rule comes next");
        assert!(
            first.iter().position(|i| i == "a") < first.iter().position(|i| i == "b"),
            "equal scores fall back to the stable id"
        );
        assert!(
            first.iter().position(|i| i == "b") < first.iter().position(|i| i == "ai1"),
            "a syntax-valid rule beats an unchecked AI suggestion"
        );
        for _ in 0..10 {
            let again: Vec<String> = rank_candidates(&list, &ctx)
                .into_iter()
                .map(|c| c.id)
                .collect();
            assert_eq!(again, first);
            let mut reversed = list.clone();
            reversed.reverse();
            let from_reversed: Vec<String> = rank_candidates(&reversed, &ctx)
                .into_iter()
                .map(|c| c.id)
                .collect();
            assert_eq!(
                from_reversed, first,
                "input order must not matter for distinct ids"
            );
        }
    }

    #[test]
    fn ut_007_usage_breaks_ties_before_weight() {
        let list = vec![mk("a", |c| c.weight = 90), mk("b", |c| c.weight = 10)];
        let mut ctx = RankContext::default();
        ctx.usage.insert("b".into(), 3);
        assert_eq!(rank_candidates(&list, &ctx)[0].id, "b");
    }

    #[test]
    fn ut_008_dedupe_keeps_all_sources_and_one_row() {
        let rule = mk("r", |c| {
            c.text = Some("FN main()  {}".into());
            c.profile_id = "rust".into();
        });
        let ai = mk("a", |c| {
            c.source = CandidateSource::Ai;
            c.sources = vec![CandidateSource::Ai];
            c.kind = CandidateKind::AiInline;
            c.validation = Validation::Unchecked;
            c.text = Some("fn main() {}".into());
            c.profile_id = "rust".into();
        });
        let out = dedupe(vec![rule, ai], "rust");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "r");
        assert_eq!(
            out[0].sources,
            vec![CandidateSource::Rule, CandidateSource::Ai]
        );
    }

    #[test]
    fn ut_008_candidates_without_text_are_never_merged() {
        let a = mk("e1", |c| c.kind = CandidateKind::Explain);
        let b = mk("e2", |c| c.kind = CandidateKind::Explain);
        assert_eq!(dedupe(vec![a, b], "rust").len(), 2);
    }
}
