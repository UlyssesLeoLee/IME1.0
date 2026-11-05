//! The privacy and safety policy of the input method (DD-IME sections 20.2 and 20.3, RD-IME section 15).
//!
//! The rules the design asks for, in one place:
//!
//! * in a password control, on the secure desktop, in an elevation prompt, in a restricted AppContainer and in a
//!   secure input field of a browser the input method offers **nothing** and reads **nothing**;
//! * it never tries to work around those restrictions: it does not simulate the keyboard, it does not inject itself
//!   into another process, it does not read the clipboard and it does not read the screen
//!   ([`FORBIDDEN_SHORTCUTS`]);
//! * it collects the text around the caret only when the user allowed it, and it sends anything to an external
//!   language model only when the user allowed **that** as well;
//! * the candidates of the local cache have to be there fast: `p95 <= 50ms` is the budget of the *local* cache
//!   only, the UI thread of the text service must never wait for a model, a compilation, a Git commit or the
//!   network.

use crate::context::{ContextKind, InputContext};

/// The budget of the local cache candidates: `p95 <= 50ms` (DD-IME section 20.2).
///
/// The budget is only about the candidates that are already on the machine. A candidate that needs the network does
/// not get a longer deadline: the UI thread keeps the ordinary text input alive and the candidate arrives later or
/// not at all.
pub const LOCAL_CACHE_P95_BUDGET_MS: u64 = 50;

/// The ways out the input method is not allowed to take (DD-IME section 20.2).
///
/// In a protected context the candidates are disabled; they are **not** enabled again by simulating the keyboard, by
/// injecting a process, by reading the clipboard or by reading the screen.
pub const FORBIDDEN_SHORTCUTS: [&str; 4] = [
    "keyboard_simulation",
    "process_injection",
    "clipboard_read",
    "screen_read",
];

/// The reason of the refusal in a password control.
pub const REASON_PASSWORD: &str = "password: the candidates are disabled in a password control";
/// The reason of the refusal on the secure desktop.
pub const REASON_SECURE_DESKTOP: &str = "secure_desktop: the candidates are disabled on the secure desktop";
/// The reason of the refusal in an elevation prompt.
pub const REASON_UAC_PROMPT: &str = "uac_prompt: the candidates are disabled in an elevation prompt";
/// The reason of the refusal in a restricted AppContainer.
pub const REASON_APP_CONTAINER: &str = "app_container: the candidates are disabled in a restricted AppContainer";
/// The reason of the refusal in a secure input field of a browser.
pub const REASON_BROWSER_SECURE_INPUT: &str =
    "browser_secure_input: the candidates are disabled in a secure input field of a browser";

/// What the input method may do in a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The context is a normal one: the candidates may be shown and the text may be inserted.
    Allow,
    /// The context is not identified: the candidates may be read, but nothing is inserted and nothing is sent.
    AllowReadOnly,
    /// The context is a protected one: nothing is read and nothing is shown. The reason says which kind it is.
    Deny(&'static str),
}

impl Decision {
    /// Whether the candidates may be shown and the text may be inserted.
    pub fn is_allowed(self) -> bool {
        matches!(self, Decision::Allow)
    }

    /// Whether the context may only be read.
    pub fn is_read_only(self) -> bool {
        matches!(self, Decision::AllowReadOnly)
    }

    /// Whether the context is refused.
    pub fn is_denied(self) -> bool {
        matches!(self, Decision::Deny(_))
    }

    /// The reason of the refusal, when there is one.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Decision::Deny(reason) => Some(reason),
            _ => None,
        }
    }

    /// Whether the context may be read at all (a refused context may not).
    pub fn allows_read(self) -> bool {
        !self.is_denied()
    }
}

/// The privacy and safety policy of the input method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrivacyPolicy {
    /// Whether the text around the caret may be collected in a source code context.
    ///
    /// Off by default: RD-IME section 15 says that the input method does not collect the content of a password
    /// control, of the secure desktop, of the whole file of another program or of any foreground window text.
    pub allow_excerpt_in_code_context: bool,
    /// Whether the context may be handed to the orchestration of an agent (and with it to an external language
    /// model).
    ///
    /// Off by default: DD-IME section 20.2 says that the transfer to an external model needs an explicit
    /// permission.
    pub allow_agent_orchestration: bool,
}

impl Default for PrivacyPolicy {
    /// The conservative default: nothing is collected and nothing is shared.
    fn default() -> PrivacyPolicy {
        PrivacyPolicy {
            allow_excerpt_in_code_context: false,
            allow_agent_orchestration: false,
        }
    }
}

impl PrivacyPolicy {
    /// A policy with the two permissions set as they are given.
    pub fn new(allow_excerpt_in_code_context: bool, allow_agent_orchestration: bool) -> PrivacyPolicy {
        PrivacyPolicy {
            allow_excerpt_in_code_context,
            allow_agent_orchestration,
        }
    }

    /// What the input method may do in this context.
    pub fn decide(&self, context: &InputContext) -> Decision {
        match context.kind {
            ContextKind::Password => Decision::Deny(REASON_PASSWORD),
            ContextKind::SecureDesktop => Decision::Deny(REASON_SECURE_DESKTOP),
            ContextKind::UacPrompt => Decision::Deny(REASON_UAC_PROMPT),
            ContextKind::AppContainer => Decision::Deny(REASON_APP_CONTAINER),
            ContextKind::BrowserSecureInput => Decision::Deny(REASON_BROWSER_SECURE_INPUT),
            ContextKind::Code | ContextKind::PlainText => Decision::Allow,
            // an unknown context is read, but nothing is inserted and nothing is sent
            ContextKind::Unknown => Decision::AllowReadOnly,
        }
    }

    /// Whether the text around the caret may be collected in this context.
    ///
    /// A protected context never has one, whatever the permissions are: the kind of the context is looked at first,
    /// so a password control, the secure desktop, an elevation prompt, a restricted AppContainer and a secure input
    /// field of a browser are refused before any permission is considered.
    pub fn excerpt_allowed(&self, context: &InputContext) -> bool {
        if context.kind.is_sensitive() {
            return false;
        }
        // only the source code context of an editor may be collected, and only with the permission
        context.kind == ContextKind::Code && self.allow_excerpt_in_code_context
    }

    /// Whether the context may be handed to the orchestration of an agent.
    ///
    /// Both are needed: the permission of the user and a context that may be read. A context that is only read
    /// (an unknown one) is not shared: the input method uses the *authorized* editing contexts only (RD-IME
    /// section 15), and it does not know what an unknown one is.
    pub fn may_share_with_agent(&self, context: &InputContext) -> bool {
        self.allow_agent_orchestration && self.decide(context).is_allowed()
    }

    /// How long the local cache candidates stay fresh (the budget of DD-IME section 20.2).
    pub fn expires_after_ms(&self) -> u64 {
        LOCAL_CACHE_P95_BUDGET_MS
    }
}

/// Whether the local cache candidates were fast enough (DD-IME section 20.2: `p95 <= 50ms`).
pub fn within_local_budget(elapsed_ms: u64) -> bool {
    elapsed_ms <= LOCAL_CACHE_P95_BUDGET_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_of(kind: ContextKind) -> InputContext {
        InputContext::safe(kind, Some("rust"), 1)
    }

    #[test]
    fn dd_0202_every_context_kind_gets_the_decision_the_design_asks_for() {
        let policy = PrivacyPolicy::default();
        assert_eq!(policy.decide(&context_of(ContextKind::Code)), Decision::Allow);
        assert_eq!(
            policy.decide(&context_of(ContextKind::PlainText)),
            Decision::Allow
        );
        assert_eq!(
            policy.decide(&context_of(ContextKind::Unknown)),
            Decision::AllowReadOnly
        );
        for kind in [
            ContextKind::Password,
            ContextKind::SecureDesktop,
            ContextKind::UacPrompt,
            ContextKind::AppContainer,
            ContextKind::BrowserSecureInput,
        ] {
            assert!(
                policy.decide(&context_of(kind)).is_denied(),
                "{kind:?} is refused"
            );
        }
    }

    #[test]
    fn dd_0202_the_reason_of_a_refusal_names_the_kind_of_the_context() {
        let policy = PrivacyPolicy::default();
        for kind in [
            ContextKind::Password,
            ContextKind::SecureDesktop,
            ContextKind::UacPrompt,
            ContextKind::AppContainer,
            ContextKind::BrowserSecureInput,
        ] {
            let reason = policy.decide(&context_of(kind)).reason().expect("a reason");
            assert!(
                reason.contains(kind.as_str()),
                "the reason {reason} does not name {kind:?}"
            );
        }
        assert_eq!(policy.decide(&context_of(ContextKind::Code)).reason(), None);
    }

    #[test]
    fn rd_015_no_excerpt_is_allowed_in_a_protected_context() {
        for allow_excerpt in [false, true] {
            let policy = PrivacyPolicy::new(allow_excerpt, false);
            for kind in [
                ContextKind::Password,
                ContextKind::SecureDesktop,
                ContextKind::UacPrompt,
                ContextKind::AppContainer,
                ContextKind::BrowserSecureInput,
            ] {
                assert!(
                    !policy.excerpt_allowed(&context_of(kind)),
                    "{kind:?} never has an excerpt, even with the permission"
                );
            }
        }
    }

    #[test]
    fn the_excerpt_of_a_code_context_needs_the_permission() {
        let closed = PrivacyPolicy::default();
        assert!(!closed.excerpt_allowed(&context_of(ContextKind::Code)));
        let open = PrivacyPolicy::new(true, false);
        assert!(open.excerpt_allowed(&context_of(ContextKind::Code)));
        assert!(
            !open.excerpt_allowed(&context_of(ContextKind::PlainText)),
            "the text of another program is not collected"
        );
        assert!(!open.excerpt_allowed(&context_of(ContextKind::Unknown)));
    }

    #[test]
    fn rd_015_sharing_with_an_agent_needs_the_permission_and_a_readable_context() {
        let closed = PrivacyPolicy::default();
        assert!(!closed.may_share_with_agent(&context_of(ContextKind::Code)));
        let open = PrivacyPolicy::new(false, true);
        assert!(open.may_share_with_agent(&context_of(ContextKind::Code)));
        assert!(open.may_share_with_agent(&context_of(ContextKind::PlainText)));
        assert!(
            !open.may_share_with_agent(&context_of(ContextKind::Password)),
            "a password control is never shared"
        );
        assert!(
            !open.may_share_with_agent(&context_of(ContextKind::Unknown)),
            "an unknown context is not an authorized editing context"
        );
    }

    #[test]
    fn dd_0202_the_local_budget_is_fifty_milliseconds() {
        assert_eq!(LOCAL_CACHE_P95_BUDGET_MS, 50);
        assert!(within_local_budget(0));
        assert!(within_local_budget(50));
        assert!(!within_local_budget(51));
        assert_eq!(PrivacyPolicy::default().expires_after_ms(), 50);
    }

    #[test]
    fn dd_0202_the_forbidden_shortcuts_are_the_four_of_the_design() {
        assert_eq!(
            FORBIDDEN_SHORTCUTS,
            [
                "keyboard_simulation",
                "process_injection",
                "clipboard_read",
                "screen_read",
            ]
        );
        for shortcut in FORBIDDEN_SHORTCUTS {
            assert!(!shortcut.is_empty());
        }
    }

    #[test]
    fn the_default_policy_collects_nothing_and_shares_nothing() {
        let policy = PrivacyPolicy::default();
        assert!(!policy.allow_excerpt_in_code_context);
        assert!(!policy.allow_agent_orchestration);
        assert!(!policy.excerpt_allowed(&context_of(ContextKind::Code)));
        assert!(!policy.may_share_with_agent(&context_of(ContextKind::Code)));
    }
}
