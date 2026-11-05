//! The decision routing of the planner (basic design §3.1).
//!
//! The basic design gives three routes: a template that matches completely, a semantic edit of a symbol that can be
//! located, and a freeform patch when neither of them holds. The planner knows four strategies, because a request
//! that asks for a test is a request of its own: the test is written, it is not a side effect of another change.
//!
//! # The rules
//!
//! The rules are evaluated in this order and the first one that holds decides:
//!
//! | Rule | Condition | Strategy | Why |
//! |---|---|---|---|
//! | `R1` | the registry offered a template (the tags are not empty) **and** the request creates new content (there is
//!   no existing symbol to anchor to) | [`Strategy::Template`] | a template produces new code: it is the route of
//!   basic design §3.1 ("模板完全匹配 + 参数充分") |
//! | `R2` | the intent asks for a test | [`Strategy::Test`] | a test is an artefact of its own; it is never a side
//!   effect of another change |
//! | `R3` | the change touches more than one file | [`Strategy::Refactor`] | a change that crosses files has to be
//!   re-parsed and verified over its whole scope (detailed design §4.3), which is the semantic route |
//! | `R4` | everything else (a local change of a symbol) | [`Strategy::FreeformPatch`] | a local change of a symbol
//!   body is the freeform route of basic design §3.1 |
//!
//! A template that would overwrite an existing symbol is **not** the template route: the symbol has to be located
//! first, which is the semantic route. That is why `R1` asks for "no existing symbol".
//!
//! # The policy
//!
//! [`StrategyPolicy`] is the configuration of detailed design §12 (`planner.prefer_template`, `planner.allow_freeform`):
//!
//! * `prefer_template = false` skips the rule `R1`: the request continues at `R2`;
//! * `allow_freeform = false` refuses the route `R4`. When the request has a symbol to anchor to, it is escalated to
//!   the semantic route (`Refactor`, rule `P1`); when it has none, there is no route left and
//!   [`route_checked`] reports the failure (rule `P2`).
//!
//! The failure-safe rule of basic design §3.1 is kept: a template whose parameters are not valid is **not** silently
//! downgraded to a freeform generation; only a new plan may do that.

use tase_contract::error::TaseError;

/// The strategy of a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Strategy {
    /// A template of the registry is instantiated.
    Template,
    /// A local change of a symbol is written by the model.
    FreeformPatch,
    /// A change that is located by its symbols and that may cross files.
    Refactor,
    /// A test is written.
    Test,
}

impl Strategy {
    /// Every strategy.
    pub const ALL: [Strategy; 4] = [
        Strategy::Template,
        Strategy::FreeformPatch,
        Strategy::Refactor,
        Strategy::Test,
    ];

    /// The name of the strategy on the wire.
    ///
    /// The name is the value of the field `strategy` of a plan request (detailed design §2.3). The contract names three
    /// of the four strategies (`template`, `semantic`, `freeform`); the test strategy is the fourth one and is written
    /// `test`.
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Template => "template",
            Strategy::FreeformPatch => "freeform",
            Strategy::Refactor => "semantic",
            Strategy::Test => "test",
        }
    }

    /// The name of the strategy inside the planner.
    ///
    /// It is the name of the variant of the enum, so that the wire name (`semantic`) and the name of the variant
    /// (`refactor`) never mix up in a log.
    pub fn planner_name(self) -> &'static str {
        match self {
            Strategy::Template => "template",
            Strategy::FreeformPatch => "freeform_patch",
            Strategy::Refactor => "refactor",
            Strategy::Test => "test",
        }
    }

    /// The strategy of a wire value; an unknown value is refused.
    ///
    /// The wire names of the contract (`template`, `semantic`, `freeform`) are accepted, and so are the names of the
    /// variants (`freeform_patch`, `refactor`).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(name: &str) -> Result<Self, TaseError> {
        match name {
            "template" => Ok(Strategy::Template),
            "freeform" | "freeform_patch" => Ok(Strategy::FreeformPatch),
            "semantic" | "refactor" => Ok(Strategy::Refactor),
            "test" => Ok(Strategy::Test),
            other => Err(TaseError::invalid_argument(format!(
                "unknown strategy {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Strategy {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s)
    }
}

/// What the router knows about a request.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RouteInput {
    /// What the agent is asked to do, in the words of the caller.
    pub intent: String,
    /// The language of the target, when it is known.
    ///
    /// The router does not filter on it: the registry already filtered the templates by language (detailed design
    /// §19.3). The planner only keeps it in the reason of the decision.
    pub language: Option<String>,
    /// The tags of the templates the registry offered for this request.
    ///
    /// An empty list means that no template was offered. The tags are what the registry matched, not a wish of the
    /// caller.
    pub tags: Vec<String>,
    /// Whether the request carries an existing symbol to anchor to.
    ///
    /// `false` means that the work creates new content (a new file or a new symbol), which is what a template
    /// produces.
    pub has_symbols: bool,
    /// Whether the change touches more than one file.
    pub touches_multiple_files: bool,
}

/// The policy of the routing (detailed design §12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrategyPolicy {
    /// Whether a template that matches completely wins over the other routes.
    pub prefer_template: bool,
    /// Whether the freeform patch route is allowed at all.
    pub allow_freeform: bool,
}

impl Default for StrategyPolicy {
    /// The default of the design book: the template first, the freeform allowed.
    fn default() -> Self {
        StrategyPolicy {
            prefer_template: true,
            allow_freeform: true,
        }
    }
}

/// The decision of the router: the strategy, the rule that decided it and the reason in one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteDecision {
    /// The strategy of the plan.
    pub strategy: Strategy,
    /// The rule that decided it (`R1` .. `R4`, `P1`, `P2`).
    pub rule: &'static str,
    /// Why the rule decided it, in one readable line.
    pub reason: String,
}

/// The failure of a routing that the policy does not allow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingError {
    /// The code of the failure.
    pub code: &'static str,
    /// What went wrong, in one line.
    pub message: String,
}

/// The code of a routing that has no route left.
pub const CODE_FREEFORM_NOT_ALLOWED: &str = "FREEFORM_NOT_ALLOWED";

/// The route of a request, with the default policy.
pub fn route(input: &RouteInput) -> Strategy {
    route_with_policy(input, &StrategyPolicy::default())
}

/// The route of a request, with a policy.
pub fn route_with_policy(input: &RouteInput, policy: &StrategyPolicy) -> Strategy {
    decide_with_policy(input, policy).strategy
}

/// The decision of the router, with the default policy.
pub fn decide(input: &RouteInput) -> RouteDecision {
    decide_with_policy(input, &StrategyPolicy::default())
}

/// The decision of the router, with a policy.
pub fn decide_with_policy(input: &RouteInput, policy: &StrategyPolicy) -> RouteDecision {
    let mut decision = classify(input);

    // The policy skips the rule R1: the request continues at the rule R2.
    if decision.rule == "R1" && !policy.prefer_template {
        decision = classify_after_template(input);
    }

    // The policy refuses the freeform route.
    if decision.strategy == Strategy::FreeformPatch && !policy.allow_freeform {
        if input.has_symbols {
            decision = RouteDecision {
                strategy: Strategy::Refactor,
                rule: "P1",
                reason: "the freeform route is not allowed: the request is escalated to the semantic route \
                         because it has a symbol to anchor to"
                    .to_string(),
            };
        } else {
            decision = RouteDecision {
                strategy: Strategy::FreeformPatch,
                rule: "P2",
                reason:
                    "the freeform route is not allowed and the request has no symbol to anchor to: \
                         there is no route left"
                        .to_string(),
            };
        }
    }

    decision
}

/// The route of a request, with a policy, that fails when the policy leaves no route.
pub fn route_checked(
    input: &RouteInput,
    policy: &StrategyPolicy,
) -> Result<Strategy, RoutingError> {
    let decision = decide_with_policy(input, policy);
    if decision.rule == "P2" {
        return Err(RoutingError {
            code: CODE_FREEFORM_NOT_ALLOWED,
            message: decision.reason.clone(),
        });
    }
    Ok(decision.strategy)
}

/// The rules of the design book, without the policy.
fn classify(input: &RouteInput) -> RouteDecision {
    // R1: a template that matches completely and that creates new content.
    if !input.tags.is_empty() && !input.has_symbols {
        return RouteDecision {
            strategy: Strategy::Template,
            rule: "R1",
            reason: format!(
                "the registry offered the template tag(s) {} and the request creates new content",
                input.tags.join(", ")
            ),
        };
    }
    classify_after_template(input)
}

/// The rules of the design book, from the rule `R2` on.
fn classify_after_template(input: &RouteInput) -> RouteDecision {
    // R2: the intent asks for a test.
    if mentions_test(&input.intent) {
        return RouteDecision {
            strategy: Strategy::Test,
            rule: "R2",
            reason: "the intent asks for a test".to_string(),
        };
    }

    // R3: the change crosses files.
    if input.touches_multiple_files {
        return RouteDecision {
            strategy: Strategy::Refactor,
            rule: "R3",
            reason:
                "the change touches more than one file: it has to be verified over its whole scope"
                    .to_string(),
        };
    }

    // R4: a local change of a symbol.
    RouteDecision {
        strategy: Strategy::FreeformPatch,
        rule: "R4",
        reason: "the change is local and no template was offered".to_string(),
    }
}

/// The keywords of a test request, in Chinese.
const TEST_KEYWORDS_CJK: [&str; 3] = ["测试", "单测", "用例"];

/// The keywords of a test request, in English; they are matched as whole words.
const TEST_KEYWORDS_ASCII: [&str; 6] = ["test", "tests", "testing", "unittest", "spec", "specs"];

/// Whether the intent asks for a test.
///
/// The English keywords are matched as whole words, so that "latest" is not a test request; the Chinese keywords are
/// matched as they are, because Chinese has no word separator.
fn mentions_test(intent: &str) -> bool {
    let lower = intent.to_lowercase();
    for keyword in TEST_KEYWORDS_CJK {
        if lower.contains(keyword) {
            return true;
        }
    }
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if TEST_KEYWORDS_ASCII.contains(token) {
            return true;
        }
        // "unit test" is two words.
        if *token == "unit" && tokens.get(index + 1).is_some_and(|next| *next == "test") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request that asks for a handler of a new file.
    fn new_file_request() -> RouteInput {
        RouteInput {
            intent: "create a handler for the user entity".to_string(),
            language: Some("rust".to_string()),
            tags: vec!["handler".to_string(), "crud".to_string()],
            has_symbols: false,
            touches_multiple_files: false,
        }
    }

    #[test]
    fn the_names_of_the_strategies_are_the_names_of_the_contract() {
        assert_eq!(Strategy::Template.as_str(), "template");
        assert_eq!(Strategy::FreeformPatch.as_str(), "freeform");
        assert_eq!(Strategy::Refactor.as_str(), "semantic");
        assert_eq!(Strategy::Test.as_str(), "test");
        for strategy in Strategy::ALL {
            assert_eq!(
                Strategy::from_str(strategy.as_str()).unwrap(),
                strategy,
                "{strategy:?}"
            );
            assert_eq!(
                Strategy::from_str(strategy.planner_name()).unwrap(),
                strategy,
                "{strategy:?}"
            );
            assert_eq!(strategy.to_string(), strategy.as_str());
            assert_eq!(strategy.as_str().parse::<Strategy>().unwrap(), strategy);
        }
        assert!(Strategy::from_str("nonsense").is_err());
    }

    #[test]
    fn the_rule_r1_sends_a_new_content_request_with_a_template_to_the_template() {
        let decision = decide(&new_file_request());
        assert_eq!(decision.strategy, Strategy::Template);
        assert_eq!(decision.rule, "R1");
        assert!(decision.reason.contains("handler"), "{}", decision.reason);
    }

    #[test]
    fn the_rule_r1_does_not_take_a_request_with_an_existing_symbol() {
        let mut input = new_file_request();
        input.has_symbols = true;
        let decision = decide(&input);
        assert_eq!(decision.strategy, Strategy::FreeformPatch);
        assert_eq!(decision.rule, "R4");
    }

    #[test]
    fn a_request_without_a_template_is_never_the_template_route() {
        let mut input = new_file_request();
        input.tags.clear();
        let decision = decide(&input);
        assert_eq!(decision.strategy, Strategy::FreeformPatch);
        assert_eq!(decision.rule, "R4");
    }

    #[test]
    fn the_rule_r2_sends_a_request_that_asks_for_a_test_to_the_test_route() {
        let mut input = new_file_request();
        input.tags.clear();
        input.intent = "write a unit test for the parser".to_string();
        let decision = decide(&input);
        assert_eq!(decision.strategy, Strategy::Test);
        assert_eq!(decision.rule, "R2");
    }

    #[test]
    fn a_chinese_request_that_asks_for_a_test_is_the_test_route() {
        let mut input = new_file_request();
        input.tags.clear();
        input.intent = "为解析器补一个测试用例".to_string();
        assert_eq!(decide(&input).strategy, Strategy::Test);
    }

    #[test]
    fn a_word_that_only_ends_with_test_is_not_a_test_request() {
        let mut input = new_file_request();
        input.tags.clear();
        input.intent = "update the latest version of the handler".to_string();
        assert_eq!(decide(&input).strategy, Strategy::FreeformPatch);
    }

    #[test]
    fn the_rule_r3_sends_a_change_that_crosses_files_to_the_refactor_route() {
        let mut input = new_file_request();
        input.tags.clear();
        input.has_symbols = true;
        input.touches_multiple_files = true;
        let decision = decide(&input);
        assert_eq!(decision.strategy, Strategy::Refactor);
        assert_eq!(decision.rule, "R3");
    }

    #[test]
    fn the_rule_r4_sends_a_local_change_of_a_symbol_to_the_freeform_route() {
        let mut input = new_file_request();
        input.tags.clear();
        input.has_symbols = true;
        let decision = decide(&input);
        assert_eq!(decision.strategy, Strategy::FreeformPatch);
        assert_eq!(decision.rule, "R4");
    }

    #[test]
    fn the_test_rule_wins_over_the_refactor_rule() {
        let mut input = new_file_request();
        input.tags.clear();
        input.has_symbols = true;
        input.touches_multiple_files = true;
        input.intent = "add a test for the parser".to_string();
        assert_eq!(decide(&input).rule, "R2");
        assert_eq!(decide(&input).strategy, Strategy::Test);
    }

    #[test]
    fn the_template_rule_wins_over_the_test_rule() {
        let mut input = new_file_request();
        input.intent = "write the test scaffold of the user entity".to_string();
        assert_eq!(decide(&input).rule, "R1");
        assert_eq!(decide(&input).strategy, Strategy::Template);
    }

    #[test]
    fn the_default_policy_prefers_the_template() {
        let policy = StrategyPolicy::default();
        assert!(policy.prefer_template);
        assert!(policy.allow_freeform);
    }

    #[test]
    fn a_policy_that_does_not_prefer_the_template_skips_the_rule_r1() {
        let policy = StrategyPolicy {
            prefer_template: false,
            allow_freeform: true,
        };
        let decision = decide_with_policy(&new_file_request(), &policy);
        assert_eq!(decision.rule, "R4");
        assert_eq!(decision.strategy, Strategy::FreeformPatch);
    }

    #[test]
    fn a_policy_that_forbids_the_freeform_escalates_a_symbol_request_to_the_semantic_route() {
        let policy = StrategyPolicy {
            prefer_template: true,
            allow_freeform: false,
        };
        let mut input = new_file_request();
        input.tags.clear();
        input.has_symbols = true;
        let decision = decide_with_policy(&input, &policy);
        assert_eq!(decision.rule, "P1");
        assert_eq!(decision.strategy, Strategy::Refactor);
    }

    #[test]
    fn a_policy_that_forbids_the_freeform_leaves_no_route_without_a_symbol() {
        let policy = StrategyPolicy {
            prefer_template: true,
            allow_freeform: false,
        };
        let mut input = new_file_request();
        input.tags.clear();
        let error = route_checked(&input, &policy).unwrap_err();
        assert_eq!(error.code, CODE_FREEFORM_NOT_ALLOWED);
        assert!(error.message.contains("no route"), "{}", error.message);
    }

    #[test]
    fn the_checked_route_returns_the_strategy_when_the_policy_allows_it() {
        let policy = StrategyPolicy::default();
        assert_eq!(
            route_checked(&new_file_request(), &policy).unwrap(),
            Strategy::Template
        );
    }

    #[test]
    fn the_route_and_the_decision_agree() {
        let inputs = [
            new_file_request(),
            RouteInput {
                intent: "write a test".to_string(),
                ..Default::default()
            },
            RouteInput {
                touches_multiple_files: true,
                ..Default::default()
            },
        ];
        for input in inputs {
            assert_eq!(route(&input), decide(&input).strategy);
        }
    }
}
