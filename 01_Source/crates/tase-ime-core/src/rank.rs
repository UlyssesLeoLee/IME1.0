//! The deterministic ranking of the candidates (DD-IME section 20.4, BD-IME section 18.2).
//!
//! The ranking is **deterministic**: the same candidates and the same request always give the same order. There is
//! no random and no clock in it - the caller passes the time it read into the request, and the ranking does not look
//! at it, so a test can compare two runs and a user always sees the same candidate first. What breaks a tie is the
//! `candidate_id`, so the order is total even when two candidates score the same.
//!
//! The score has four parts, and the total is their sum (the risk part is a penalty, so it is never positive):
//!
//! | part | what it measures |
//! |---|---|
//! | `language_match` | the language of the candidate against the language the user asked for |
//! | `keyword_match` | the words of the query against the label, the asset reference and the risk of the candidate |
//! | `applicability` | the segments of the asset id (`pattern`, `gof`, ...) against the field the user asked for |
//! | `risk_penalty` | what the candidate changes: a candidate that changes the whole project is pushed down |
//!
//! The user may ask the agent for the reasoning: [`RankScore::explain`] gives the parts, so that a candidate can be
//! told apart from another one of the same score.

use crate::candidate::Candidate;
use crate::context::InputContext;
use std::cmp::Ordering;

/// The score of a candidate whose language is the one the user asked for.
const LANGUAGE_MATCH: f64 = 1.0;

/// The score of a candidate whose language is another one.
const LANGUAGE_MISMATCH: f64 = -1.0;

/// The weight of the segments of the asset id in the applicability.
const APPLICABILITY_SEGMENT_WEIGHT: f64 = 0.6;

/// The weight of the kind of the asset (`pattern`, `algorithm`) in the applicability.
const APPLICABILITY_KIND_WEIGHT: f64 = 0.4;

/// The penalty of a candidate that changes the whole project (it goes to the IDE service for a preview).
const RISK_PENALTY_COMPLETE_PROJECT_EDIT: f64 = -0.5;

/// The penalty of a candidate that carries a note about its risk.
const RISK_PENALTY_NOTED: f64 = -0.1;

/// What the ranking is asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct RankRequest {
    /// What the user typed.
    pub query: String,
    /// The language the user asked for, when they named one.
    pub language: Option<String>,
    /// The context the user is typing in.
    pub context: InputContext,
    /// The time the caller read the clock at.
    ///
    /// The ranking does not use it: the order must not depend on the time, otherwise the same input would give
    /// different candidates from one keystroke to the next. The caller keeps the reading for its own bookkeeping.
    pub now_ms: u64,
}

impl RankRequest {
    /// A request without a reading of the clock (the caller adds one with [`RankRequest::with_now_ms`]).
    pub fn new(query: &str, language: Option<&str>, context: InputContext) -> RankRequest {
        RankRequest {
            query: query.to_string(),
            language: language.map(|l| l.to_string()),
            context,
            now_ms: 0,
        }
    }

    /// The request with the reading of the clock of the caller.
    pub fn with_now_ms(mut self, now_ms: u64) -> RankRequest {
        self.now_ms = now_ms;
        self
    }

    /// The reading of the clock of the caller.
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// The language the ranking compares against: the one of the request, or the hint of the context.
    fn language(&self) -> Option<&str> {
        self.language.as_deref().or_else(|| self.context.language())
    }
}

/// The score of a candidate, part by part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RankScore {
    /// The sum of the four parts.
    pub total: f64,
    /// The language of the candidate against the language of the request.
    pub language_match: f64,
    /// The words of the query against the candidate.
    pub keyword_match: f64,
    /// The segments of the asset id against the field of the query.
    pub applicability: f64,
    /// What the candidate changes (never positive).
    pub risk_penalty: f64,
}

impl RankScore {
    /// The parts of the score, with their names: the reasoning of the order.
    ///
    /// The sum of the parts is the total.
    pub fn explain(&self) -> Vec<(String, f64)> {
        vec![
            ("language_match".to_string(), self.language_match),
            ("keyword_match".to_string(), self.keyword_match),
            ("applicability".to_string(), self.applicability),
            ("risk_penalty".to_string(), self.risk_penalty),
        ]
    }
}

/// A candidate and its score.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedCandidate {
    /// The candidate.
    pub candidate: Candidate,
    /// Its score.
    pub score: RankScore,
}

/// The score of a candidate for a request.
pub fn score_candidate(candidate: &Candidate, request: &RankRequest) -> RankScore {
    let language_match = language_match(candidate, request);
    let keyword_match = keyword_match(candidate, request);
    let applicability = applicability(candidate, request);
    let risk_penalty = risk_penalty(candidate);
    RankScore {
        total: language_match + keyword_match + applicability + risk_penalty,
        language_match,
        keyword_match,
        applicability,
        risk_penalty,
    }
}

/// The candidates of a request, the best one first.
///
/// The order is total and stable: the same candidates and the same request always give the same order, and two
/// candidates that score the same are ordered by their `candidate_id`.
pub fn rank(candidates: &[Candidate], request: &RankRequest) -> Vec<RankedCandidate> {
    let mut ranked: Vec<RankedCandidate> = candidates
        .iter()
        .map(|candidate| RankedCandidate {
            candidate: candidate.clone(),
            score: score_candidate(candidate, request),
        })
        .collect();
    ranked.sort_by(compare);
    ranked
}

/// The order of two candidates: the best one comes first.
fn compare(a: &RankedCandidate, b: &RankedCandidate) -> Ordering {
    let total = compare_scores(b.score.total, a.score.total);
    if total != Ordering::Equal {
        return total;
    }
    // the same total: the parts decide, one after the other, so that the order never depends on the order of the
    // candidates in the list
    for (x, y) in [
        (a.score.language_match, b.score.language_match),
        (a.score.keyword_match, b.score.keyword_match),
        (a.score.applicability, b.score.applicability),
        (a.score.risk_penalty, b.score.risk_penalty),
    ] {
        let part = compare_scores(y, x);
        if part != Ordering::Equal {
            return part;
        }
    }
    // the same score: the id decides
    a.candidate.candidate_id.cmp(&b.candidate.candidate_id)
}

/// The order of two scores, rounded to the resolution of the comparison.
fn compare_scores(a: f64, b: f64) -> Ordering {
    round9(a).partial_cmp(&round9(b)).unwrap_or(Ordering::Equal)
}

/// A score rounded to the resolution of the comparison, so that two scores that only differ by a rounding error
/// are the same score.
fn round9(value: f64) -> f64 {
    (value * 1e9).round() / 1e9
}

/// The words of a text: the runs of alphanumeric characters, in lower case.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect()
}

/// The language of the candidate against the language of the request.
///
/// The candidate of the language the user asked for gets the full score, a candidate of another language is pushed
/// down hard (the implementation of another language cannot be inserted into a file of this one), and a request
/// without a language says nothing about the candidates.
fn language_match(candidate: &Candidate, request: &RankRequest) -> f64 {
    let Some(language) = request.language() else {
        return 0.0;
    };
    let language = language.trim();
    let candidate_language = candidate.language.trim();
    if language.is_empty() || candidate_language.is_empty() {
        return 0.0;
    }
    if candidate_language.eq_ignore_ascii_case(language) {
        LANGUAGE_MATCH
    } else {
        LANGUAGE_MISMATCH
    }
}

/// The words of the query against the label, the asset reference and the risk of the candidate.
///
/// The ratio is the part of the words of the query that the candidate answers to. The version of the asset
/// reference is not part of the haystack: the numbers of a version are not a word the user typed.
fn keyword_match(candidate: &Candidate, request: &RankRequest) -> f64 {
    let query = tokenize(&request.query);
    if query.is_empty() {
        return 0.0;
    }
    let mut haystack = tokenize(&candidate.label);
    haystack.extend(tokenize(candidate.asset_ref.id()));
    if let Some(risk) = candidate.risk.as_deref() {
        haystack.extend(tokenize(risk));
    }
    let matched = query
        .iter()
        .filter(|word| haystack.iter().any(|answer| answer.starts_with(word.as_str())))
        .count();
    matched as f64 / query.len() as f64
}

/// The segments of the asset id against the field of the query.
///
/// The id of an asset names it (`pattern.gof.behavioral.strategy`): the segments that the query names are the ones
/// that tell that the candidate belongs to the field the user asked for. The kind of the asset (`pattern`,
/// `algorithm`) counts more than a single segment, because the user asks for a pattern or an algorithm.
fn applicability(candidate: &Candidate, request: &RankRequest) -> f64 {
    let domain = tokenize(&request.query);
    if domain.is_empty() {
        return 0.0;
    }
    let segments = candidate.asset_ref.segments();
    if segments.is_empty() {
        return 0.0;
    }
    let hits = segments
        .iter()
        .filter(|segment| {
            let segment = *segment;
            domain
                .iter()
                .any(|word| segment.starts_with(word.as_str()) || word.starts_with(segment))
        })
        .count();
    let ratio = hits as f64 / segments.len() as f64;
    let kind_hit = match candidate.asset_ref.kind() {
        Some(kind) => domain
            .iter()
            .any(|word| word.starts_with(kind) || kind.starts_with(word.as_str())),
        None => false,
    };
    APPLICABILITY_SEGMENT_WEIGHT * ratio
        + if kind_hit {
            APPLICABILITY_KIND_WEIGHT
        } else {
            0.0
        }
}

/// What the candidate changes.
///
/// A candidate that changes the whole project is not inserted into the text of the user: it goes to the IDE service
/// for a preview and an approval (BD-IME section 18.2), so it is pushed down. A candidate that carries a note about
/// its risk is pushed down a little.
fn risk_penalty(candidate: &Candidate) -> f64 {
    let mut penalty = 0.0;
    if candidate.complete_project_edit {
        penalty += RISK_PENALTY_COMPLETE_PROJECT_EDIT;
    }
    if candidate
        .risk
        .as_deref()
        .is_some_and(|risk| !risk.trim().is_empty())
    {
        penalty += RISK_PENALTY_NOTED;
    }
    penalty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::{AssetRef, CandidateStatus};
    use crate::context::ContextKind;

    fn candidate(
        candidate_id: &str,
        asset: &str,
        language: &str,
        label: &str,
    ) -> Candidate {
        let mut candidate = Candidate::new(
            candidate_id,
            AssetRef::parse(asset).expect("the asset of the test"),
        );
        candidate.language = language.to_string();
        candidate.label = label.to_string();
        candidate.status = CandidateStatus::ImplementationAvailable;
        candidate
    }

    fn request(query: &str, language: Option<&str>) -> RankRequest {
        RankRequest::new(
            query,
            language,
            InputContext::for_code(Some("rust"), 1),
        )
    }

    fn ids(ranked: &[RankedCandidate]) -> Vec<String> {
        ranked
            .iter()
            .map(|r| r.candidate.candidate_id.clone())
            .collect()
    }

    fn sample() -> Vec<Candidate> {
        let mut strategy = candidate(
            "cand-104",
            "pattern.gof.behavioral.strategy@1.0.0",
            "rust",
            "Strategy: trait + implementations",
        );
        strategy.risk = Some("新 trait 带来抽象与维护成本".to_string());
        let mut python_strategy = strategy.clone();
        python_strategy.candidate_id = "cand-204".to_string();
        python_strategy.language = "python".to_string();
        let mut project = strategy.clone();
        project.candidate_id = "cand-304".to_string();
        project.complete_project_edit = true;
        let dijkstra = candidate(
            "cand-404",
            "algorithm.graph.dijkstra@1.0.0",
            "rust",
            "Dijkstra shortest path",
        );
        vec![python_strategy, dijkstra, project, strategy]
    }

    #[test]
    fn dd_0204_the_order_of_the_candidates_is_the_expected_one() {
        let ranked = rank(&sample(), &request("strategy", Some("rust")));
        assert_eq!(
            ids(&ranked),
            vec![
                "cand-104",  // the language, the word and the field fit
                "cand-304",  // the same, but it changes the whole project
                "cand-404",  // the language fits, the word does not
                "cand-204",  // the language does not fit
            ]
        );
    }

    #[test]
    fn dd_0204_equal_scores_are_broken_by_the_candidate_id() {
        let one = candidate("cand-b", "pattern.gof.behavioral.strategy@1.0.0", "rust", "Strategy");
        let two = candidate("cand-a", "pattern.gof.behavioral.strategy@1.0.0", "rust", "Strategy");
        let ranked = rank(&[one.clone(), two.clone()], &request("strategy", Some("rust")));
        assert_eq!(ids(&ranked), vec!["cand-a", "cand-b"]);
        let ranked = rank(&[two, one], &request("strategy", Some("rust")));
        assert_eq!(ids(&ranked), vec!["cand-a", "cand-b"]);
    }

    #[test]
    fn the_order_does_not_depend_on_the_order_of_the_candidates() {
        let mut list = sample();
        let expected = ids(&rank(&list, &request("strategy", Some("rust"))));
        list.reverse();
        assert_eq!(ids(&rank(&list, &request("strategy", Some("rust")))), expected);
        let mut other = list.clone();
        other.swap(0, 2);
        assert_eq!(ids(&rank(&other, &request("strategy", Some("rust")))), expected);
    }

    #[test]
    fn the_clock_of_the_caller_does_not_change_the_order() {
        let list = sample();
        let first = rank(&list, &request("strategy", Some("rust")).with_now_ms(0));
        let second = rank(&list, &request("strategy", Some("rust")).with_now_ms(999_999));
        assert_eq!(ids(&first), ids(&second));
        assert_eq!(first, second);
        assert_eq!(second[0].score, first[0].score);
    }

    #[test]
    fn the_language_of_the_request_decides() {
        let list = sample();
        let rust = rank(&list, &request("strategy", Some("rust")));
        let python = rank(&list, &request("strategy", Some("python")));
        assert_eq!(rust[0].candidate.candidate_id, "cand-104");
        assert_eq!(python[0].candidate.candidate_id, "cand-204");
        assert_eq!(rust[0].score.language_match, LANGUAGE_MATCH);
        assert_eq!(python[0].score.language_match, LANGUAGE_MATCH);
        let rust_score = rust
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-204")
            .expect("the candidate is there")
            .score;
        assert_eq!(rust_score.language_match, LANGUAGE_MISMATCH);
    }

    #[test]
    fn a_candidate_that_changes_the_whole_project_is_pushed_down() {
        let list = sample();
        let ranked = rank(&list, &request("strategy", Some("rust")));
        let whole = ranked
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-304")
            .expect("the candidate is there");
        let card = ranked
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-104")
            .expect("the candidate is there");
        assert_eq!(whole.score.risk_penalty, RISK_PENALTY_COMPLETE_PROJECT_EDIT + RISK_PENALTY_NOTED);
        assert_eq!(card.score.risk_penalty, RISK_PENALTY_NOTED);
        assert!(whole.score.total < card.score.total);
    }

    #[test]
    fn the_explanation_adds_up_to_the_total() {
        for ranked in rank(&sample(), &request("strategy", Some("rust"))) {
            let parts = ranked.score.explain();
            assert_eq!(parts.len(), 4);
            let sum: f64 = parts.iter().map(|(_, value)| value).sum();
            assert!(
                (sum - ranked.score.total).abs() < 1e-9,
                "the parts {parts:?} do not add up to {}",
                ranked.score.total
            );
            let names: Vec<&str> = parts.iter().map(|(name, _)| name.as_str()).collect();
            assert_eq!(
                names,
                vec![
                    "language_match",
                    "keyword_match",
                    "applicability",
                    "risk_penalty"
                ]
            );
        }
    }

    #[test]
    fn the_keyword_match_counts_the_words_of_the_query() {
        let list = sample();
        let ranked = rank(&list, &request("strategy", Some("rust")));
        let card = ranked
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-104")
            .expect("the candidate is there");
        assert_eq!(card.score.keyword_match, 1.0);
        let dijkstra = ranked
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-404")
            .expect("the candidate is there");
        assert_eq!(dijkstra.score.keyword_match, 0.0);
        let half = rank(&list, &request("strategy dijkstra", Some("rust")));
        let card = half
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-104")
            .expect("the candidate is there");
        assert_eq!(card.score.keyword_match, 0.5);
    }

    #[test]
    fn the_applicability_follows_the_segments_of_the_asset_id() {
        let list = sample();
        let ranked = rank(&list, &request("strategy", Some("rust")));
        let card = ranked
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-104")
            .expect("the candidate is there");
        // one of the four segments of `pattern.gof.behavioral.strategy` is named by the query
        assert_eq!(card.score.applicability, APPLICABILITY_SEGMENT_WEIGHT * 0.25);
        let pattern = rank(&list, &request("pattern", Some("rust")));
        let card = pattern
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-104")
            .expect("the candidate is there");
        // the kind of the asset is named by the query
        assert_eq!(
            card.score.applicability,
            APPLICABILITY_SEGMENT_WEIGHT * 0.25 + APPLICABILITY_KIND_WEIGHT
        );
    }

    #[test]
    fn a_request_without_a_word_or_a_language_says_nothing() {
        let list = sample();
        let silent = RankRequest::new("", None, InputContext::safe(ContextKind::Code, None, 1));
        for entry in rank(&list, &silent) {
            assert_eq!(entry.score.keyword_match, 0.0);
            assert_eq!(entry.score.language_match, 0.0);
            assert_eq!(entry.score.applicability, 0.0);
        }
        // the hint of the context is used when the request names no language
        let with_context = RankRequest::new("", None, InputContext::for_code(Some("rust"), 1));
        let ranked = rank(&list, &with_context);
        let card = ranked
            .iter()
            .find(|r| r.candidate.candidate_id == "cand-104")
            .expect("the candidate is there");
        assert_eq!(card.score.language_match, LANGUAGE_MATCH);
    }

    #[test]
    fn a_ranking_of_no_candidate_is_empty() {
        assert!(rank(&[], &request("strategy", Some("rust"))).is_empty());
    }

    #[test]
    fn the_context_does_not_change_the_order() {
        let list = sample();
        let code = rank(&list, &request("strategy", Some("rust")));
        let plain = rank(
            &list,
            &RankRequest::new(
                "strategy",
                Some("rust"),
                InputContext::safe(ContextKind::PlainText, Some("rust"), 1),
            ),
        );
        assert_eq!(ids(&code), ids(&plain));
    }
}
