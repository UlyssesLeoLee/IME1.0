//! The selection and the ranking of the assets (detailed design §19.2 and §19.3, basic design §17.4,
//! requirements §14.4).
//!
//! # What the answer holds
//!
//! A recommendation is never a bare list of names. Every candidate carries the fields the contract of detailed
//! design §19.2 asks for - `asset_id`, `implementation_id`, `version`, `reason`, `complexity`, `requirements` and
//! `proof_evidence` - and more:
//!
//! * the **score** and the **breakdown** of the score: the four parts and what each of them contributed;
//! * the **counterexamples** of the card and the **risks** the card declares (requirements §14.4: "反例、许可和风险
//!   必须随机器结果返回，防止 Agent 为了减少 Token 无条件套用不适合的算法");
//! * the **license** of the card and the **alternatives** the card names;
//! * the flag `instantiable` and the reasons: an asset of this repository is a knowledge card, and a knowledge card
//!   may not be instantiated.
//!
//! The candidates that were not recommended are kept apart, with the reason of the refusal
//! (`rejected=[{asset_id,reason}]`, detailed design §19.2).
//!
//! # The score
//!
//! The score is the sum of four parts, and the sum of the parts is the score - always, with no rounding:
//!
//! | the part | the weight | what it measures |
//! |---|---|---|
//! | `intent` | 60 | the share of the intent the card covers |
//! | `constraints` | 15 | the properties the card declares in the caller's favour |
//! | `complexity` | 10 | the declared cost of the card against the budget of the caller |
//! | `implementation` | 15 | the state of the implementation of the card in the requested language |
//!
//! The intent is split into terms, and every term is worth at most 12 points: a term that is found in the name
//! of the card is worth 12, in an alias 10, in the id 8, in the intent of the card 6, in a condition or in a tag 4.
//! The part of the score is the weight of the terms the card covers, divided by the weight of all the terms of the
//! intent - so a card that covers the whole intent is worth the 60 points, and a card that covers a part of it is
//! worth that part.
//!
//! The parts are integers, the score is an integer, and the order of the candidates is the score and then the id:
//! the answer of the catalog is the same for the same request, every time (no clock, no randomness, no hash order).
//!
//! # The gate
//!
//! A candidate that does not answer the request is **not** recommended, and the refusal says why (detailed design
//! §19.2: `rejected=[{asset_id,reason}]`). The rules are:
//!
//! 1. an implementation that is `REVOKED` in the requested language is not recommended any more;
//! 2. the license of the card must be on the list of the licenses the caller allows, when the caller states one;
//! 3. a card that declares a global mutable state is refused when the caller does not allow it;
//! 4. a card that declares that it is not thread safe is refused when the caller asks for thread safety;
//! 5. a card that declares that it may need negative inputs is refused when the caller does not allow them;
//! 6. a card that declares that its order is not stable is refused when the caller asks for a stable order;
//! 7. a card whose declared cost is above the budget of the caller is refused;
//! 8. when the caller asks for an instantiable asset, a card without an approved implementation in the requested
//!    language is refused.
//!
//! A card that says **nothing** about a property is neither refused nor rewarded on it: the risk is reported
//! instead. The catalog never invents a property that the manifest does not declare.

use tase_contract::TaseError;

use crate::assets::{AssetView, Layer};
use crate::governance::{ImplementationStatus, license_allows};
use crate::implementation::ImplementationRegistry;
use crate::index::CatalogIndex;

/// The number of the candidates an answer holds when the caller does not ask for a number.
pub const DEFAULT_LIMIT: usize = 10;

/// The largest number of the candidates an answer holds.
///
/// The MCP tool of detailed design §19.6 caps the limit at 50; the library accepts more, for a caller that asks
/// for the whole catalog at once.
pub const MAX_LIMIT: usize = 200;

/// The weight of the part of the score that measures the match of the intent.
pub const WEIGHT_INTENT: i32 = 60;
/// The weight of the part of the score that measures the fit of the declared conditions.
pub const WEIGHT_CONSTRAINTS: i32 = 15;
/// The weight of the part of the score that measures the declared cost.
pub const WEIGHT_COMPLEXITY: i32 = 10;
/// The weight of the part of the score that measures the state of the implementation.
pub const WEIGHT_IMPLEMENTATION: i32 = 15;
/// The largest score a candidate may reach.
pub const MAX_SCORE: i32 =
    WEIGHT_INTENT + WEIGHT_CONSTRAINTS + WEIGHT_COMPLEXITY + WEIGHT_IMPLEMENTATION;

/// The points a term of the intent is worth when it is found in the name of the card: this is the weight of a term
/// that is fully covered.
const POINTS_TERM_MAX: i32 = 12;
/// The points a term of the intent gives when it is found in the name of the card.
const POINTS_NAME: i32 = 12;
/// The points a term of the intent gives when it is found in an alias of the card.
const POINTS_ALIAS: i32 = 10;
/// The points a term of the intent gives when it is found in the id of the card.
const POINTS_ID: i32 = 8;
/// The points a term of the intent gives when it is found in the intent of the card.
const POINTS_INTENT: i32 = 6;
/// The points a term of the intent gives when it is found in a condition of the card.
const POINTS_CONDITION: i32 = 4;
/// The points a term of the intent gives when it is found in a tag of the card.
const POINTS_TAG: i32 = 4;

/// The points a card gets when it declares that it is thread safe and the caller asks for thread safety.
const POINTS_THREAD_SAFE: i32 = 6;
/// The points a card gets when it declares that its order is stable and the caller asks for a stable order.
const POINTS_STABLE_ORDER: i32 = 4;
/// The points a card gets when it declares that it does not need negative inputs and the caller forbids them.
const POINTS_NONNEGATIVE: i32 = 5;

/// The cost of a card, as the card declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Complexity {
    /// The time the card declares.
    pub time: String,
    /// The memory the card declares, on top of the input.
    pub extra_space: String,
}

/// The budget of the caller: the cost it accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComplexityBudget {
    /// At most O(log n).
    Logarithmic,
    /// At most O(n).
    Linear,
    /// At most O(n log n).
    Linearithmic,
    /// At most O(n^2).
    Quadratic,
    /// No budget: the cost only scores, it never refuses.
    Unbounded,
}

impl ComplexityBudget {
    /// The name of the budget on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ComplexityBudget::Logarithmic => "LOGARITHMIC",
            ComplexityBudget::Linear => "LINEAR",
            ComplexityBudget::Linearithmic => "LINEARITHMIC",
            ComplexityBudget::Quadratic => "QUADRATIC",
            ComplexityBudget::Unbounded => "UNBOUNDED",
        }
    }

    /// The largest cost the budget accepts; `None` when the budget accepts anything.
    pub fn max_order(self) -> Option<u8> {
        match self {
            ComplexityBudget::Logarithmic => Some(2),
            ComplexityBudget::Linear => Some(3),
            ComplexityBudget::Linearithmic => Some(4),
            ComplexityBudget::Quadratic => Some(5),
            ComplexityBudget::Unbounded => None,
        }
    }
}

/// The rank of a declared cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComplexityRank {
    /// O(1).
    Constant,
    /// O(log n).
    Logarithmic,
    /// O(n).
    Linear,
    /// O(n log n).
    Linearithmic,
    /// O(n^2).
    Quadratic,
    /// O(2^n) or worse.
    Exponential,
    /// The cost is declared in prose that cannot be ordered.
    Unknown,
    /// The card declares no cost at all ("n/a").
    NotApplicable,
}

impl ComplexityRank {
    /// The name of the rank on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ComplexityRank::Constant => "CONSTANT",
            ComplexityRank::Logarithmic => "LOGARITHMIC",
            ComplexityRank::Linear => "LINEAR",
            ComplexityRank::Linearithmic => "LINEARITHMIC",
            ComplexityRank::Quadratic => "QUADRATIC",
            ComplexityRank::Exponential => "EXPONENTIAL",
            ComplexityRank::Unknown => "UNKNOWN",
            ComplexityRank::NotApplicable => "NOT_APPLICABLE",
        }
    }

    /// The place of the rank in the order of the costs; `None` when the cost cannot be ordered.
    pub fn order(self) -> Option<u8> {
        match self {
            ComplexityRank::Constant => Some(1),
            ComplexityRank::Logarithmic => Some(2),
            ComplexityRank::Linear => Some(3),
            ComplexityRank::Linearithmic => Some(4),
            ComplexityRank::Quadratic => Some(5),
            ComplexityRank::Exponential => Some(6),
            ComplexityRank::Unknown | ComplexityRank::NotApplicable => None,
        }
    }

    /// Whether the cost is above the budget.
    ///
    /// A cost that cannot be ordered is never above a budget: the card is not refused for a cost the catalog does
    /// not understand.
    pub fn exceeds(self, budget: ComplexityBudget) -> bool {
        match (self.order(), budget.max_order()) {
            (Some(cost), Some(limit)) => cost > limit,
            _ => false,
        }
    }

    /// The points the cost is worth.
    pub fn points(self) -> i32 {
        match self {
            ComplexityRank::Constant | ComplexityRank::Logarithmic => 10,
            ComplexityRank::Linear => 8,
            ComplexityRank::Linearithmic => 5,
            ComplexityRank::Quadratic => 3,
            ComplexityRank::Exponential => 0,
            // a cost that is not declared, or that cannot be ordered, is worth the middle: it is neither a bonus
            // nor a penalty.
            ComplexityRank::Unknown | ComplexityRank::NotApplicable => 5,
        }
    }
}

/// The rank of a declared cost, read from the text of the card.
///
/// The cost of a card is prose ("O(n log n) 平均 / O(n²) 最坏"): the reading is a coarse order, not a proof. The
/// rule is fixed and documented:
///
/// * a text that is empty or "n/a" declares no cost at all;
/// * every "O(...)" of the text is read and the worst of them wins;
/// * inside one "O(...)": "2^" or "n!" or "v^3" is exponential; "n²", "v²", "n*m" or "v*e" is quadratic; a body
///   that starts with "log" is logarithmic; a body that holds "log" somewhere else is n log n; a body that is only
///   sizes and operators is linear; anything else is a cost that cannot be ordered.
pub fn rank_of(text: &str) -> ComplexityRank {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() || lower == "n/a" || lower == "na" || lower == "-" {
        return ComplexityRank::NotApplicable;
    }
    let mut worst = ComplexityRank::Unknown;
    for body in big_o_bodies(&lower) {
        let rank = rank_of_body(&body);
        if rank.order().unwrap_or(0) > worst.order().unwrap_or(0) {
            worst = rank;
        }
    }
    worst
}

/// The bodies of the "O(...)" of a text.
fn big_o_bodies(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut bodies: Vec<String> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == 'o' && chars.get(index + 1) == Some(&'(') {
            let start = index + 2;
            let mut depth = 1;
            let mut cursor = start;
            while cursor < chars.len() && depth > 0 {
                match chars[cursor] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    cursor += 1;
                }
            }
            let end = cursor.min(chars.len());
            bodies.push(chars[start..end].iter().collect());
            index = end + 1;
            continue;
        }
        index += 1;
    }
    bodies
}

/// The rank of one body of an "O(...)".
fn rank_of_body(body: &str) -> ComplexityRank {
    let compact: String = body
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    if compact.is_empty() || compact == "1" {
        return ComplexityRank::Constant;
    }
    if compact.contains("2^")
        || compact.contains("n!")
        || compact.contains("v^3")
        || compact.contains("n^3")
    {
        return ComplexityRank::Exponential;
    }
    if compact.contains("n²")
        || compact.contains("n^2")
        || compact.contains("v²")
        || compact.contains("v^2")
        || compact.contains("n*m")
        || compact.contains("v*e")
        || compact.contains("n*w")
        || compact.contains("v*v")
    {
        return ComplexityRank::Quadratic;
    }
    if compact.contains("log") {
        if compact.starts_with("log") {
            return ComplexityRank::Logarithmic;
        }
        return ComplexityRank::Linearithmic;
    }
    if is_size_expression(&compact) {
        return ComplexityRank::Linear;
    }
    ComplexityRank::Unknown
}

/// Whether a body holds nothing but sizes and operators.
fn is_size_expression(body: &str) -> bool {
    let mut seen_size = false;
    for ch in body.chars() {
        if ch.is_ascii_digit() {
            continue;
        }
        if "nmvketdabpqrsuijxyz".contains(ch) {
            seen_size = true;
            continue;
        }
        if "+-*^()".contains(ch) {
            continue;
        }
        return false;
    }
    seen_size
}

/// The worst of two ranks.
fn max_rank(left: ComplexityRank, right: ComplexityRank) -> ComplexityRank {
    match (left.order(), right.order()) {
        (None, None) => {
            if left == ComplexityRank::Unknown || right == ComplexityRank::Unknown {
                ComplexityRank::Unknown
            } else {
                ComplexityRank::NotApplicable
            }
        }
        (None, Some(_)) => right,
        (Some(_), None) => left,
        (Some(x), Some(y)) => {
            if x >= y {
                left
            } else {
                right
            }
        }
    }
}

/// The conditions of the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constraints {
    /// The cost the caller accepts; `None` when the caller states no budget.
    pub max_complexity: Option<ComplexityBudget>,
    /// Whether the caller accepts a card that keeps a global mutable state.
    pub allow_global_mutable_state: bool,
    /// Whether the caller asks for an asset that may be used from several threads at once.
    pub require_thread_safety: bool,
    /// Whether the caller accepts a card that may need negative inputs.
    pub allow_negative_inputs: bool,
    /// Whether the caller asks for an asset whose order is stable.
    pub require_stable_order: bool,
    /// Whether the caller asks for an asset that may be instantiated.
    pub require_instantiable: bool,
    /// The licenses the caller allows; an empty list states no restriction at the gate of the recommendation.
    pub allowed_licenses: Vec<String>,
}

impl Default for Constraints {
    fn default() -> Self {
        Constraints {
            max_complexity: None,
            allow_global_mutable_state: true,
            require_thread_safety: false,
            allow_negative_inputs: true,
            require_stable_order: false,
            require_instantiable: false,
            allowed_licenses: Vec::new(),
        }
    }
}

/// The request of a recommendation (detailed design §19.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecommendRequest {
    /// The intent of the caller, in words.
    pub intent: String,
    /// The language of the implementation the caller wants.
    pub language: String,
    /// The conditions of the caller.
    pub constraints: Constraints,
    /// Only the assets of this layer.
    pub layer: Option<Layer>,
    /// Only the assets of this category.
    pub category: Option<String>,
    /// How many candidates to return at most (0 means the default).
    pub limit: usize,
}

impl Default for RecommendRequest {
    fn default() -> Self {
        RecommendRequest {
            intent: String::new(),
            language: String::new(),
            constraints: Constraints::default(),
            layer: None,
            category: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// The breakdown of the score of a candidate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScoreBreakdown {
    /// The points of the match of the intent.
    pub intent: i32,
    /// The points of the fit of the declared conditions.
    pub constraints: i32,
    /// The points of the declared cost.
    pub complexity: i32,
    /// The points of the state of the implementation.
    pub implementation: i32,
}

impl ScoreBreakdown {
    /// The score: the sum of the parts.
    pub fn total(&self) -> i32 {
        self.intent + self.constraints + self.complexity + self.implementation
    }

    /// The parts of the score, with the weight of each of them.
    pub fn components(&self) -> Vec<ScoreComponent> {
        vec![
            ScoreComponent {
                name: "intent",
                points: self.intent,
                weight: WEIGHT_INTENT,
            },
            ScoreComponent {
                name: "constraints",
                points: self.constraints,
                weight: WEIGHT_CONSTRAINTS,
            },
            ScoreComponent {
                name: "complexity",
                points: self.complexity,
                weight: WEIGHT_COMPLEXITY,
            },
            ScoreComponent {
                name: "implementation",
                points: self.implementation,
                weight: WEIGHT_IMPLEMENTATION,
            },
        ]
    }
}

/// One part of the score of a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreComponent {
    /// The name of the part.
    pub name: &'static str,
    /// The points of the part.
    pub points: i32,
    /// The weight of the part.
    pub weight: i32,
}

/// Why a candidate was not recommended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectionReason {
    /// The implementation of the asset in the requested language is revoked.
    Revoked,
    /// The license of the card is not on the list of the licenses the caller allows.
    LicenseNotAllowed,
    /// The card declares a global mutable state and the caller does not allow it.
    GlobalMutableStateNotAllowed,
    /// The card declares that it is not thread safe and the caller asks for thread safety.
    NotThreadSafe,
    /// The card declares that it may need negative inputs and the caller does not allow them.
    NegativeInputsNotAllowed,
    /// The card declares that its order is not stable and the caller asks for a stable order.
    UnstableOrderNotAllowed,
    /// The declared cost of the card is above the budget of the caller.
    ComplexityAboveBudget,
    /// The asset has no implementation in the requested language.
    NoImplementationInLanguage,
    /// The asset has an implementation in the requested language, but it is not approved.
    NoApprovedImplementation,
}

impl RejectionReason {
    /// The reason on the wire (detailed design §19.2: `reason:'NEGATIVE_EDGES_UNSUPPORTED'`).
    pub fn as_str(self) -> &'static str {
        match self {
            RejectionReason::Revoked => "REVOKED",
            RejectionReason::LicenseNotAllowed => "LICENSE_NOT_ALLOWED",
            RejectionReason::GlobalMutableStateNotAllowed => "GLOBAL_MUTABLE_STATE_NOT_ALLOWED",
            RejectionReason::NotThreadSafe => "NOT_THREAD_SAFE",
            RejectionReason::NegativeInputsNotAllowed => "NEGATIVE_INPUTS_NOT_ALLOWED",
            RejectionReason::UnstableOrderNotAllowed => "UNSTABLE_ORDER_NOT_ALLOWED",
            RejectionReason::ComplexityAboveBudget => "COMPLEXITY_ABOVE_BUDGET",
            RejectionReason::NoImplementationInLanguage => "NO_IMPLEMENTATION_IN_LANGUAGE",
            RejectionReason::NoApprovedImplementation => "NO_APPROVED_IMPLEMENTATION",
        }
    }
}

/// A candidate that was not recommended, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectedCandidate {
    /// The id of the asset.
    pub asset_id: String,
    /// The reason of the refusal.
    pub reason: RejectionReason,
    /// The refusal, in one line.
    pub detail: String,
}

impl RejectedCandidate {
    /// A refusal.
    fn new(asset_id: &str, reason: RejectionReason, detail: impl Into<String>) -> RejectedCandidate {
        RejectedCandidate {
            asset_id: asset_id.to_string(),
            reason,
            detail: detail.into(),
        }
    }
}

/// Where the confidence of a score comes from (basic design §17.4).
///
/// The confidence of a score is never the confidence of a model: it says where the score comes from. The catalog
/// scores by **rule**, so every score of this crate is a rule score. `VERIFIED_BENCHMARK` is the state of a score
/// that a benchmark of the repository has confirmed - the catalog of this repository holds no benchmark, so the
/// state is never reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfidenceSource {
    /// The score comes from the rules of the catalog.
    Rule,
    /// The score comes from a benchmark that was run and kept.
    VerifiedBenchmark,
}

impl ConfidenceSource {
    /// The name of the source on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ConfidenceSource::Rule => "rule",
            ConfidenceSource::VerifiedBenchmark => "verified_benchmark",
        }
    }
}

/// One candidate of the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recommendation {
    /// The id of the asset.
    pub asset_id: String,
    /// The version of the asset.
    pub version: String,
    /// The layer of the asset.
    pub layer: Layer,
    /// The score of the candidate.
    pub score: i32,
    /// The breakdown of the score.
    pub breakdown: ScoreBreakdown,
    /// Where the confidence of the score comes from: never the confidence of a model.
    pub confidence_source: ConfidenceSource,
    /// Whether the asset may be instantiated in the requested language.
    pub instantiable: bool,
    /// The id of the approved implementation in the requested language, when there is one.
    pub implementation_id: Option<String>,
    /// The conditions the card declares (the `constraints` of the manifest).
    pub requirements: Vec<(String, String)>,
    /// Where the evidence of the implementation lives, when there is an approved implementation.
    pub proof_evidence: Option<String>,
    /// Why the asset may (or may not) be instantiated.
    pub reasons: Vec<String>,
    /// The cases the card does not solve (requirements §14.4).
    pub counterexamples: Vec<String>,
    /// The risks the card declares.
    pub risks: Vec<String>,
    /// The explanation of the score.
    pub notes: Vec<String>,
    /// The license of the card.
    pub license: Option<String>,
    /// The declared cost of the card.
    pub complexity: Option<Complexity>,
    /// The assets the card names as its alternatives.
    pub alternatives: Vec<String>,
}

/// The answer of a recommendation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecommendationResult {
    /// The intent of the request.
    pub intent: String,
    /// The language of the request.
    pub language: String,
    /// The candidates, the best one first.
    pub recommended: Vec<Recommendation>,
    /// The candidates that were not recommended, and why.
    pub rejected: Vec<RejectedCandidate>,
    /// The number of the candidates that did not fit in the limit.
    pub truncated: usize,
}

/// The recommendation of a request.
///
/// The answer is a pure function of the index, of the registry of the implementations and of the request: the same
/// request always gives the same answer, in the same order.
pub fn recommend(
    index: &CatalogIndex,
    implementations: &ImplementationRegistry,
    request: &RecommendRequest,
) -> Result<RecommendationResult, TaseError> {
    let intent = request.intent.trim();
    if intent.is_empty() {
        return Err(TaseError::invalid_argument(
            "the intent must not be empty: the catalog cannot guess which asset is meant".to_string(),
        ));
    }
    let language = request.language.trim();
    if language.is_empty() {
        return Err(TaseError::invalid_argument(
            "the target language must not be empty".to_string(),
        ));
    }
    let limit = if request.limit == 0 {
        DEFAULT_LIMIT
    } else {
        request.limit
    };
    if limit > MAX_LIMIT {
        return Err(TaseError::invalid_argument(format!(
            "the limit {limit} is larger than the largest answer {MAX_LIMIT}"
        )));
    }

    let terms = tokenize(intent);
    let mut recommended: Vec<Recommendation> = Vec::new();
    let mut rejected: Vec<RejectedCandidate> = Vec::new();

    for view in index.views() {
        if let Some(layer) = request.layer
            && view.layer != layer
        {
            continue;
        }
        if let Some(category) = &request.category {
            let category = category.trim();
            if !category.is_empty() && !view.entry.category.eq_ignore_ascii_case(category) {
                continue;
            }
        }
        let matched = intent_match(view, &terms);
        if !matched.answered {
            // the asset does not answer the intent at all: it is not a candidate
            continue;
        }
        if let Err(refusal) = gate(view, implementations, request) {
            rejected.push(refusal);
            continue;
        }
        recommended.push(build(view, implementations, request, matched.points));
    }

    recommended.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.asset_id.cmp(&right.asset_id))
    });
    rejected.sort_by(|left, right| left.asset_id.cmp(&right.asset_id));

    let truncated = recommended.len().saturating_sub(limit);
    recommended.truncate(limit);

    Ok(RecommendationResult {
        intent: intent.to_string(),
        language: language.to_string(),
        recommended,
        rejected,
        truncated,
    })
}

/// The gate: the rules that refuse a candidate.
fn gate(
    view: &AssetView,
    implementations: &ImplementationRegistry,
    request: &RecommendRequest,
) -> Result<(), RejectedCandidate> {
    let constraints = &request.constraints;
    let language = request.language.trim();
    let in_language = implementations.for_asset_and_language(view.id(), language);

    // 1. a revoked implementation is not recommended any more
    if in_language
        .iter()
        .any(|implementation| implementation.status == ImplementationStatus::Revoked)
    {
        return Err(RejectedCandidate::new(
            view.id(),
            RejectionReason::Revoked,
            format!("the implementation of the asset in {language} is revoked"),
        ));
    }

    // 2. the license of the card
    if !constraints.allowed_licenses.is_empty()
        && !license_allows(view.entry.license.as_deref(), &constraints.allowed_licenses)
    {
        return Err(RejectedCandidate::new(
            view.id(),
            RejectionReason::LicenseNotAllowed,
            format!(
                "the license of the card is not on the list of the allowed licenses: {:?}",
                view.entry.license
            ),
        ));
    }

    let traits = view.traits();

    // 3. the global mutable state
    if !constraints.allow_global_mutable_state && traits.global_mutable_state {
        return Err(RejectedCandidate::new(
            view.id(),
            RejectionReason::GlobalMutableStateNotAllowed,
            "the card declares that the asset keeps a global mutable state".to_string(),
        ));
    }

    // 4. the thread safety
    if constraints.require_thread_safety && traits.not_thread_safe {
        return Err(RejectedCandidate::new(
            view.id(),
            RejectionReason::NotThreadSafe,
            "the card declares that the asset is not thread safe".to_string(),
        ));
    }

    // 5. the negative inputs
    if !constraints.allow_negative_inputs && traits.may_need_negative_inputs {
        return Err(RejectedCandidate::new(
            view.id(),
            RejectionReason::NegativeInputsNotAllowed,
            "the card declares that the asset may need negative inputs".to_string(),
        ));
    }

    // 6. the stable order
    if constraints.require_stable_order && traits.stable_order == Some(false) {
        return Err(RejectedCandidate::new(
            view.id(),
            RejectionReason::UnstableOrderNotAllowed,
            "the card declares that the order it produces is not stable".to_string(),
        ));
    }

    // 7. the cost
    if let Some(budget) = constraints.max_complexity {
        let rank = complexity_rank(view);
        if rank.exceeds(budget) {
            return Err(RejectedCandidate::new(
                view.id(),
                RejectionReason::ComplexityAboveBudget,
                format!(
                    "the declared cost of the card ({}) is above the budget of the caller ({})",
                    rank.as_str(),
                    budget.as_str()
                ),
            ));
        }
    }

    // 8. the instantiability
    if constraints.require_instantiable && implementations.approved_for(view.id(), language).is_none() {
        let reason = if in_language.is_empty() {
            RejectionReason::NoImplementationInLanguage
        } else {
            RejectionReason::NoApprovedImplementation
        };
        return Err(RejectedCandidate::new(
            view.id(),
            reason,
            format!("the asset has no approved implementation in {language}"),
        ));
    }

    Ok(())
}

/// The rank of the cost of a card.
fn complexity_rank(view: &AssetView) -> ComplexityRank {
    let time = rank_of(view.entry.complexity_time.as_deref().unwrap_or_default());
    let space = rank_of(view.entry.complexity_space.as_deref().unwrap_or_default());
    max_rank(time, space)
}

/// The candidate of the answer.
fn build(
    view: &AssetView,
    implementations: &ImplementationRegistry,
    request: &RecommendRequest,
    intent_points: i32,
) -> Recommendation {
    let constraints_points = constraints_points(view, &request.constraints);
    let rank = complexity_rank(view);
    let implementation_points = if implementations
        .approved_for(view.id(), request.language.trim())
        .is_some()
    {
        WEIGHT_IMPLEMENTATION
    } else {
        0
    };
    let breakdown = ScoreBreakdown {
        intent: intent_points,
        constraints: constraints_points,
        complexity: rank.points(),
        implementation: implementation_points,
    };
    let (instantiable, reasons) = instantiability(view, implementations, request);
    let approved = implementations.approved_for(view.id(), request.language.trim());
    Recommendation {
        asset_id: view.entry.id.clone(),
        version: view.entry.version.clone(),
        layer: view.layer,
        score: breakdown.total(),
        breakdown,
        confidence_source: ConfidenceSource::Rule,
        instantiable,
        implementation_id: approved.map(|implementation| implementation.id.clone()),
        requirements: view.entry.constraints.clone(),
        proof_evidence: approved.and_then(|implementation| implementation.tests_manifest.clone()),
        reasons,
        counterexamples: view.entry.counterexamples.clone(),
        risks: risks_of(view, implementations, request),
        notes: breakdown
            .components()
            .iter()
            .map(|component| {
                format!(
                    "{}: {} of {}",
                    component.name, component.points, component.weight
                )
            })
            .collect(),
        license: view.entry.license.clone(),
        complexity: match (&view.entry.complexity_time, &view.entry.complexity_space) {
            (Some(time), Some(space)) => Some(Complexity {
                time: time.clone(),
                extra_space: space.clone(),
            }),
            _ => None,
        },
        alternatives: view.entry.alternatives.clone(),
    }
}

/// The points of the declared conditions.
fn constraints_points(view: &AssetView, constraints: &Constraints) -> i32 {
    let traits = view.traits();
    let mut points = 0;
    if constraints.require_thread_safety && traits.declares_thread_safe && !traits.not_thread_safe {
        points += POINTS_THREAD_SAFE;
    }
    if constraints.require_stable_order && traits.stable_order == Some(true) {
        points += POINTS_STABLE_ORDER;
    }
    if !constraints.allow_negative_inputs && traits.declares_nonnegative_inputs {
        points += POINTS_NONNEGATIVE;
    }
    points.min(WEIGHT_CONSTRAINTS)
}

/// Whether the asset may be instantiated, and why.
fn instantiability(
    view: &AssetView,
    implementations: &ImplementationRegistry,
    request: &RecommendRequest,
) -> (bool, Vec<String>) {
    let language = request.language.trim();
    let in_language = implementations.for_asset_and_language(view.id(), language);
    let mut reasons: Vec<String> = Vec::new();

    let approved = implementations.approved_for(view.id(), language);
    if approved.is_none() {
        if in_language.is_empty() {
            reasons.push(format!(
                "the asset has no implementation in {language}: it is a knowledge card"
            ));
        } else {
            let states: Vec<&str> = in_language
                .iter()
                .map(|implementation| implementation.status.as_str())
                .collect();
            reasons.push(format!(
                "the implementation of the asset in {language} is {} and not APPROVED",
                states.join(", ")
            ));
        }
    }

    let allowed = &request.constraints.allowed_licenses;
    if allowed.is_empty() {
        reasons.push(
            "the caller states no list of allowed licenses: the license of the card is not verified"
                .to_string(),
        );
    } else if !license_allows(view.entry.license.as_deref(), allowed) {
        reasons.push(format!(
            "the license of the card ({:?}) is not on the list of the allowed licenses",
            view.entry.license
        ));
    }

    let instantiable = approved.is_some()
        && !allowed.is_empty()
        && license_allows(view.entry.license.as_deref(), allowed);
    if instantiable {
        reasons.push(format!(
            "the implementation {} of the asset in {language} is APPROVED and its license is allowed",
            approved.map(|implementation| implementation.id.as_str()).unwrap_or_default()
        ));
    }
    (instantiable, reasons)
}

/// The risks the card declares.
fn risks_of(
    view: &AssetView,
    implementations: &ImplementationRegistry,
    request: &RecommendRequest,
) -> Vec<String> {
    let traits = view.traits();
    let language = request.language.trim();
    let mut risks: Vec<String> = Vec::new();
    if traits.global_mutable_state {
        risks.push("the card declares that the asset keeps a global mutable state".to_string());
    }
    if traits.not_thread_safe {
        risks.push("the card declares that the asset is not thread safe".to_string());
    }
    if !traits.declares_thread_safe {
        risks.push("the card does not declare whether the asset is thread safe".to_string());
    }
    if traits.may_need_negative_inputs {
        risks.push("the card declares that the asset may need negative inputs".to_string());
    }
    if traits.stable_order == Some(false) {
        risks.push("the card declares that the order it produces is not stable".to_string());
    }
    if view.entry.license.is_none() {
        risks.push("the card declares no license".to_string());
    }
    let in_language = implementations.for_asset_and_language(view.id(), language);
    if in_language.is_empty() {
        risks.push(format!("there is no implementation of the asset in {language}"));
    } else if !in_language
        .iter()
        .any(|implementation| implementation.status.is_instantiable())
    {
        risks.push(format!(
            "the implementation of the asset in {language} is not APPROVED"
        ));
    }
    if view.entry.counterexamples.is_empty() {
        risks.push("the card declares no counterexample".to_string());
    }
    risks
}

/// One term of the intent.
struct Term {
    /// The windows of the term that are matched against the fields of the cards.
    windows: Vec<String>,
}

impl Term {
    /// The windows of a term.
    ///
    /// A term in Chinese is matched with its bigrams as well: the intent "最短路" then finds the card whose name
    /// is "最短路径".
    fn new(text: &str, is_cjk: bool) -> Term {
        let mut windows = vec![text.to_string()];
        if is_cjk {
            let chars: Vec<char> = text.chars().collect();
            for window in chars.windows(2) {
                let candidate: String = window.iter().collect();
                if !windows.contains(&candidate) {
                    windows.push(candidate);
                }
            }
        }
        Term { windows }
    }
}

/// Whether a character is a character of Chinese.
fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF
    )
}

/// The terms of an intent.
fn tokenize(intent: &str) -> Vec<Term> {
    let mut terms: Vec<Term> = Vec::new();
    let mut buffer = String::new();
    let mut buffer_is_cjk = false;
    for ch in intent.chars() {
        let is_cjk = is_cjk_char(ch);
        let is_word = ch.is_alphanumeric() || is_cjk;
        if !is_word {
            push_term(&mut terms, &mut buffer, &mut buffer_is_cjk);
            continue;
        }
        if !buffer.is_empty() && is_cjk != buffer_is_cjk {
            push_term(&mut terms, &mut buffer, &mut buffer_is_cjk);
        }
        buffer_is_cjk = is_cjk;
        buffer.push(ch);
    }
    push_term(&mut terms, &mut buffer, &mut buffer_is_cjk);
    terms
}

/// The terms that are ready.
fn push_term(terms: &mut Vec<Term>, buffer: &mut String, buffer_is_cjk: &mut bool) {
    if buffer.is_empty() {
        return;
    }
    let text = std::mem::take(buffer);
    let is_cjk = *buffer_is_cjk;
    *buffer_is_cjk = false;
    terms.push(Term::new(&text, is_cjk));
}

/// The fields of a card, in lower case.
struct LowerFields {
    names: Vec<String>,
    aliases: Vec<String>,
    id: String,
    intent: String,
    conditions: Vec<String>,
    tags: Vec<String>,
}

impl LowerFields {
    /// The fields of a card, in lower case.
    fn of(view: &AssetView) -> LowerFields {
        LowerFields {
            names: view
                .entry
                .names
                .values()
                .map(|name| name.to_lowercase())
                .collect(),
            aliases: view
                .aliases
                .iter()
                .map(|alias| alias.to_lowercase())
                .collect(),
            id: view.entry.id.to_lowercase(),
            intent: view.entry.intent.to_lowercase(),
            conditions: view
                .entry
                .constraints
                .iter()
                .map(|(_, value)| value.to_lowercase())
                .collect(),
            tags: view.tags.clone(),
        }
    }
}

/// The match of the intent with a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IntentMatch {
    /// The points of the part of the score that measures the intent.
    points: i32,
    /// Whether the card answers the intent at all.
    answered: bool,
}

/// The match of the intent with a card.
///
/// The intent is split into terms, and every term is worth at most [`POINTS_TERM_MAX`] points: a term that is
/// found in the name of the card is worth the most, a term that is found in a condition is worth the least. The
/// part of the score is the share of the intent the card covers - the weight of the terms the card covers, divided
/// by the weight of all the terms of the intent. The division is rounded up, so a card that covers one term of a
/// long intent is still worth something.
fn intent_match(view: &AssetView, terms: &[Term]) -> IntentMatch {
    let fields = LowerFields::of(view);
    let mut covered: usize = 0;
    let mut answered = false;
    for term in terms {
        let mut best = 0;
        for window in &term.windows {
            let needle = window.to_lowercase();
            if needle.is_empty() {
                continue;
            }
            let mut points = 0;
            for name in &fields.names {
                if name.contains(&needle) {
                    points = points.max(POINTS_NAME);
                }
            }
            for alias in &fields.aliases {
                if alias.contains(&needle) {
                    points = points.max(POINTS_ALIAS);
                }
            }
            if fields.id.contains(&needle) {
                points = points.max(POINTS_ID);
            }
            if fields.intent.contains(&needle) {
                points = points.max(POINTS_INTENT);
            }
            for condition in &fields.conditions {
                if condition.contains(&needle) {
                    points = points.max(POINTS_CONDITION);
                }
            }
            for tag in &fields.tags {
                if tag.contains(&needle) {
                    points = points.max(POINTS_TAG);
                }
            }
            best = best.max(points);
        }
        if best > 0 {
            answered = true;
        }
        covered += best as usize;
    }
    if !answered || terms.is_empty() {
        return IntentMatch {
            points: 0,
            answered: false,
        };
    }
    let denominator = POINTS_TERM_MAX as usize * terms.len();
    let points = (covered * WEIGHT_INTENT as usize).div_ceil(denominator);
    IntentMatch {
        points: points as i32,
        answered: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Layer;
    use crate::implementation::Implementation;
    use std::path::Path;

    /// The catalog of the repository.
    fn real_index() -> CatalogIndex {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = tase_registry::Registry::load(&dir).expect("the catalog is valid");
        CatalogIndex::build(&registry).expect("the index is built")
    }

    /// The registry of the implementations of the repository: it holds none.
    fn real_implementations() -> ImplementationRegistry {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        let registry = tase_registry::Registry::load(&dir).expect("the catalog is valid");
        ImplementationRegistry::from_registry(&registry).expect("the registry is read")
    }

    /// A request with the given intent.
    fn request(intent: &str) -> RecommendRequest {
        RecommendRequest {
            intent: intent.to_string(),
            language: "rust".to_string(),
            ..RecommendRequest::default()
        }
    }

    /// The first candidate of the answer.
    fn first(result: &RecommendationResult) -> &Recommendation {
        &result.recommended[0]
    }

    #[test]
    fn the_answer_is_deterministic() {
        let index = real_index();
        let implementations = real_implementations();
        let request = request("单例");
        let one = recommend(&index, &implementations, &request).expect("the answer is there");
        let two = recommend(&index, &implementations, &request).expect("the answer is there");
        assert_eq!(one, two);
        // and the order of the candidates is stable
        let ids: Vec<&str> = one.recommended.iter().map(|c| c.asset_id.as_str()).collect();
        let ids_again: Vec<&str> = two.recommended.iter().map(|c| c.asset_id.as_str()).collect();
        assert_eq!(ids, ids_again);
    }

    #[test]
    fn the_sum_of_the_breakdown_is_the_score() {        let index = real_index();
        let implementations = real_implementations();
        let result = recommend(&index, &implementations, &request("最短路径")).expect("the answer is there");
        assert!(!result.recommended.is_empty());
        for candidate in &result.recommended {
            assert_eq!(candidate.breakdown.total(), candidate.score);
            let parts: i32 = candidate.breakdown.components().iter().map(|c| c.points).sum();
            assert_eq!(parts, candidate.score);
            // the confidence of a score is never the confidence of a model
            assert_eq!(candidate.confidence_source, ConfidenceSource::Rule);
            for component in candidate.breakdown.components() {
                assert!(component.points >= 0, "{component:?}");
                assert!(component.points <= component.weight, "{component:?}");
            }
        }
    }

    #[test]
    fn the_intent_finds_the_asset_that_is_asked_for() {
        let index = real_index();
        let implementations = real_implementations();
        let result = recommend(&index, &implementations, &request("单例")).expect("the answer is there");
        assert_eq!(first(&result).asset_id, "pattern.gof.creational.singleton");
        let result = recommend(&index, &implementations, &request("最小生成树")).expect("the answer is there");
        assert_eq!(first(&result).asset_id, "algorithm.graph.kruskal");
        let result = recommend(&index, &implementations, &request("MST")).expect("the answer is there");
        assert_eq!(first(&result).asset_id, "algorithm.graph.kruskal");
        let result = recommend(&index, &implementations, &request("短路")).expect("the answer is there");
        assert_eq!(first(&result).asset_id, "algorithm.graph.dijkstra");
    }

    #[test]
    fn the_answer_carries_the_counterexamples_and_the_risks() {
        let index = real_index();
        let implementations = real_implementations();
        let result = recommend(&index, &implementations, &request("单例")).expect("the answer is there");
        let candidate = first(&result);
        assert!(!candidate.counterexamples.is_empty());
        assert!(!candidate.risks.is_empty());
        // the risk of the global mutable state is there
        assert!(
            candidate
                .risks
                .iter()
                .any(|risk| risk.contains("global mutable state")),
            "{:?}",
            candidate.risks
        );
        // the license travels with the result
        assert_eq!(candidate.license.as_deref(), Some("MIT"));
        // the alternatives travel with the result
        assert!(candidate.alternatives.contains(&"pattern.arch.dependency_injection".to_string()));
    }

    #[test]
    fn every_asset_of_the_repository_is_a_knowledge_card_and_is_not_instantiable() {
        let index = real_index();
        let implementations = real_implementations();
        // not one asset of the repository has an implementation at all
        assert!(implementations.is_empty());
        for view in index.views() {
            let mut request = request(&view.entry.names.values().next().cloned().unwrap_or_default());
            request.limit = MAX_LIMIT;
            let result = recommend(&index, &implementations, &request).expect("the answer is there");
            let candidate = result
                .recommended
                .iter()
                .find(|candidate| candidate.asset_id == view.id())
                .unwrap_or_else(|| panic!("{} is not in the answer", view.id()));
            assert!(!candidate.instantiable, "{}", view.id());
            assert!(
                candidate
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("knowledge card")),
                "{}: {:?}",
                view.id(),
                candidate.reasons
            );
            assert_eq!(candidate.breakdown.implementation, 0);
        }
    }

    #[test]
    fn a_constraint_that_is_not_met_refuses_the_candidate() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("单例");
        request.constraints.require_thread_safety = true;
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        assert!(
            !result
                .recommended
                .iter()
                .any(|candidate| candidate.asset_id == "pattern.gof.creational.singleton"),
            "the singleton must not be recommended: it is not thread safe"
        );
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "pattern.gof.creational.singleton")
            .expect("the refusal is there");
        assert_eq!(refusal.reason, RejectionReason::NotThreadSafe);
        assert_eq!(refusal.reason.as_str(), "NOT_THREAD_SAFE");
    }

    #[test]
    fn a_global_mutable_state_is_refused_when_the_caller_forbids_it() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("单例");
        request.constraints.allow_global_mutable_state = false;
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "pattern.gof.creational.singleton")
            .expect("the refusal is there");
        assert_eq!(
            refusal.reason,
            RejectionReason::GlobalMutableStateNotAllowed
        );
    }

    #[test]
    fn a_negative_input_is_refused_when_the_caller_forbids_it() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("最短路径");
        request.constraints.allow_negative_inputs = false;
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "algorithm.graph.bellman_ford")
            .expect("the refusal is there");
        assert_eq!(refusal.reason, RejectionReason::NegativeInputsNotAllowed);
        // the card that declares that it does not need them is not refused
        assert!(
            !result
                .rejected
                .iter()
                .any(|refusal| refusal.asset_id == "algorithm.graph.dijkstra")
        );
    }

    #[test]
    fn an_unstable_order_is_refused_when_the_caller_asks_for_a_stable_one() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("排序");
        request.constraints.require_stable_order = true;
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "algorithm.sorting.heap_sort")
            .expect("the refusal is there");
        assert_eq!(refusal.reason, RejectionReason::UnstableOrderNotAllowed);
        // the stable one is not refused, and it earns the points of the property
        let stable = result
            .recommended
            .iter()
            .find(|candidate| candidate.asset_id == "algorithm.sorting.merge_sort")
            .expect("the card is there");
        assert_eq!(stable.breakdown.constraints, POINTS_STABLE_ORDER);
    }

    #[test]
    fn a_license_that_is_not_allowed_refuses_the_candidate() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("单例");
        request.constraints.allowed_licenses = vec!["Apache-2.0".to_string()];
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "pattern.gof.creational.singleton")
            .expect("the refusal is there");
        assert_eq!(refusal.reason, RejectionReason::LicenseNotAllowed);
        // the license of the repository is allowed: the candidate comes back
        request.constraints.allowed_licenses = vec!["MIT".to_string()];
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        assert!(
            result
                .recommended
                .iter()
                .any(|candidate| candidate.asset_id == "pattern.gof.creational.singleton")
        );
    }

    #[test]
    fn a_cost_above_the_budget_refuses_the_candidate() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("排序");
        request.constraints.max_complexity = Some(ComplexityBudget::Linear);
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "algorithm.sorting.merge_sort")
            .expect("the refusal is there");
        assert_eq!(refusal.reason, RejectionReason::ComplexityAboveBudget);
        // a linear sort is inside the budget
        assert!(
            !result
                .rejected
                .iter()
                .any(|refusal| refusal.asset_id == "algorithm.searching.binary_search")
        );
    }

    #[test]
    fn the_cost_of_a_card_is_read_from_its_declaration() {
        assert_eq!(rank_of("n/a"), ComplexityRank::NotApplicable);
        assert_eq!(rank_of(""), ComplexityRank::NotApplicable);
        assert_eq!(rank_of("O(1)"), ComplexityRank::Constant);
        assert_eq!(rank_of("O(log n)"), ComplexityRank::Logarithmic);
        assert_eq!(rank_of("O(n)"), ComplexityRank::Linear);
        assert_eq!(rank_of("O(V + E)"), ComplexityRank::Linear);
        assert_eq!(rank_of("O(n log n)"), ComplexityRank::Linearithmic);
        assert_eq!(rank_of("O(E log V)"), ComplexityRank::Linearithmic);
        assert_eq!(rank_of("O(n²)"), ComplexityRank::Quadratic);
        assert_eq!(rank_of("O(V * E)"), ComplexityRank::Quadratic);
        assert_eq!(rank_of("O(2^n)"), ComplexityRank::Exponential);
        // the worst of the declarations wins
        assert_eq!(
            rank_of("O(n log n) 平均 / O(n²) 最坏"),
            ComplexityRank::Quadratic
        );
        assert_eq!(
            rank_of("O(log n) 插入/弹出，O(1) 取顶，建堆 O(n)"),
            ComplexityRank::Linear
        );
        // a cost that cannot be ordered is not ordered
        assert_eq!(rank_of("O(总模式长度 + 文本长度 + 匹配数)"), ComplexityRank::Unknown);
        assert_eq!(rank_of("O(容量)"), ComplexityRank::Unknown);
    }

    #[test]
    fn a_cost_that_cannot_be_ordered_never_refuses_a_candidate() {
        assert!(!ComplexityRank::Unknown.exceeds(ComplexityBudget::Logarithmic));
        assert!(!ComplexityRank::NotApplicable.exceeds(ComplexityBudget::Logarithmic));
        assert!(ComplexityRank::Quadratic.exceeds(ComplexityBudget::Linear));
        assert!(!ComplexityRank::Linear.exceeds(ComplexityBudget::Linear));
        assert!(!ComplexityRank::Linear.exceeds(ComplexityBudget::Unbounded));
    }

    #[test]
    fn the_caller_that_asks_for_an_instantiable_asset_gets_nothing_of_this_repository() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("单例");
        request.constraints.require_instantiable = true;
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        assert!(result.recommended.is_empty());
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "pattern.gof.creational.singleton")
            .expect("the refusal is there");
        assert_eq!(
            refusal.reason,
            RejectionReason::NoImplementationInLanguage
        );
    }

    #[test]
    fn a_revoked_implementation_is_not_recommended() {
        let index = real_index();
        let mut implementations = real_implementations();
        implementations
            .register(Implementation {
                id: "pattern.gof.creational.singleton.rust".to_string(),
                asset_id: "pattern.gof.creational.singleton".to_string(),
                language: "rust".to_string(),
                version: "1.0.0".to_string(),
                status: ImplementationStatus::Revoked,
                engine: "tase-template".to_string(),
                path: Some("templates/rust/patterns/singleton".to_string()),
                sha256: Some("a".repeat(64)),
                parameters_schema: Some("schemas/singleton-params.json".to_string()),
                tests_manifest: Some("tests/singleton.yaml".to_string()),
            })
            .expect("the implementation is registered");
        let result = recommend(&index, &implementations, &request("单例")).expect("the answer is there");
        assert!(
            !result
                .recommended
                .iter()
                .any(|candidate| candidate.asset_id == "pattern.gof.creational.singleton")
        );
        let refusal = result
            .rejected
            .iter()
            .find(|refusal| refusal.asset_id == "pattern.gof.creational.singleton")
            .expect("the refusal is there");
        assert_eq!(refusal.reason, RejectionReason::Revoked);
    }

    #[test]
    fn an_approved_implementation_makes_the_candidate_instantiable() {
        let index = real_index();
        let mut implementations = real_implementations();
        implementations
            .register(Implementation {
                id: "pattern.gof.creational.singleton.rust".to_string(),
                asset_id: "pattern.gof.creational.singleton".to_string(),
                language: "rust".to_string(),
                version: "1.0.0".to_string(),
                status: ImplementationStatus::Approved,
                engine: "tase-template".to_string(),
                path: Some("templates/rust/patterns/singleton".to_string()),
                sha256: Some("a".repeat(64)),
                parameters_schema: Some("schemas/singleton-params.json".to_string()),
                tests_manifest: Some("tests/singleton.yaml".to_string()),
            })
            .expect("the implementation is registered");
        let mut request = request("单例");
        request.constraints.allowed_licenses = vec!["MIT".to_string()];
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        let candidate = first(&result);
        assert!(candidate.instantiable);
        assert_eq!(candidate.breakdown.implementation, WEIGHT_IMPLEMENTATION);
        // the id of the implementation and the evidence travel with the result
        assert_eq!(
            candidate.implementation_id.as_deref(),
            Some("pattern.gof.creational.singleton.rust")
        );
        assert_eq!(
            candidate.proof_evidence.as_deref(),
            Some("tests/singleton.yaml")
        );
        assert!(
            candidate
                .reasons
                .iter()
                .any(|reason| reason.contains("APPROVED"))
        );
    }

    #[test]
    fn the_order_of_the_candidates_is_the_score_and_then_the_id() {
        let index = real_index();
        let implementations = real_implementations();
        let result = recommend(&index, &implementations, &request("图")).expect("the answer is there");
        for pair in result.recommended.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            assert!(
                left.score > right.score
                    || (left.score == right.score && left.asset_id < right.asset_id),
                "{} ({}), {} ({})",
                left.asset_id,
                left.score,
                right.asset_id,
                right.score
            );
        }
    }

    #[test]
    fn an_empty_intent_is_refused() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("单例");
        request.intent = "   ".to_string();
        let error = recommend(&index, &implementations, &request).expect_err("there is no intent");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        request.intent = String::new();
        assert!(recommend(&index, &implementations, &request).is_err());
    }

    #[test]
    fn a_limit_that_is_too_large_is_refused() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("单例");
        request.limit = MAX_LIMIT + 1;
        let error = recommend(&index, &implementations, &request).expect_err("the limit is too large");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        request.limit = MAX_LIMIT;
        assert!(recommend(&index, &implementations, &request).is_ok());
    }

    #[test]
    fn the_layer_and_the_category_narrow_the_candidates() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("排序");
        request.layer = Some(Layer::Algorithm);
        request.category = Some("sorting".to_string());
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        for candidate in &result.recommended {
            assert_eq!(candidate.layer, Layer::Algorithm);
        }
        assert!(!result.recommended.is_empty());
        // a layer that does not fit the intent gives nothing
        request.layer = Some(Layer::Concurrency);
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        assert!(result.recommended.is_empty());
    }

    #[test]
    fn the_answer_is_cut_at_the_limit_and_says_how_many_were_left_out() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("图");
        request.limit = 2;
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        assert_eq!(result.recommended.len(), 2);
        assert!(result.truncated > 0);
    }

    #[test]
    fn the_score_of_the_parts_never_leaves_their_weight() {
        let index = real_index();
        let implementations = real_implementations();
        let mut request = request("最短路径");
        request.constraints.require_thread_safety = true;
        request.constraints.require_stable_order = true;
        request.constraints.allow_negative_inputs = false;
        request.constraints.max_complexity = Some(ComplexityBudget::Unbounded);
        let result = recommend(&index, &implementations, &request).expect("the answer is there");
        for candidate in &result.recommended {
            assert!(candidate.breakdown.intent <= WEIGHT_INTENT);
            assert!(candidate.breakdown.constraints <= WEIGHT_CONSTRAINTS);
            assert!(candidate.breakdown.complexity <= WEIGHT_COMPLEXITY);
            assert!(candidate.breakdown.implementation <= WEIGHT_IMPLEMENTATION);
            assert!(candidate.score <= MAX_SCORE);
        }
    }
}
