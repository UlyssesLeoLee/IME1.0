//! The privacy and the safety rules of TASE, shared with the extension and with the agents.
//!
//! `tase-ime-core` is the only truth source for these decisions (DD-IME sections 20.2 and 20.3, RD-IME section
//! 15). The text service does **not** keep a second copy of them: it asks the shared policy and obeys the answer.
//! That is what makes the three sides agree - the candidate window of the input method, the bridge the AI clients
//! write through and the extension of the editor.
//!
//! # What the text service knows about its context
//!
//! * the host says whether the field is a password or a secure field (`HostText::is_secure_field`);
//! * the thread manager says whether the thread runs on the secure desktop (`TF_TMAE_SECUREMODE`).
//!
//! Both are protected contexts for the shared policy: nothing is read and nothing is written there, and the reason
//! of the refusal names the kind of the context. Everything else is an ordinary editing context - the product is a
//! programming input method, so the context of the user is a source code one.
//!
//! # What this module does NOT do
//!
//! It does not decide *what* is written and it does not read anything by itself: it only answers the question
//! "may the text service collect / write here", with the rule that the rest of the product shares.

use tase_ime_core::context::{ContextKind, InputContext};
use tase_ime_core::policy::{Decision, PrivacyPolicy};

/// The policy the text service obeys.
///
/// The conservative default: nothing is collected around the caret and nothing is handed to an agent. The two
/// permissions are off in the shipped product, exactly as in the extension.
pub fn policy() -> PrivacyPolicy {
    PrivacyPolicy::default()
}

/// The context of the text service: a protected one when the host says so, a source code one otherwise.
///
/// `secure_mode` is the secure desktop (the thread manager says so); `secure_field` is the answer of the host
/// about the field that has the focus.
pub fn context_of(secure_mode: bool, secure_field: bool) -> InputContext {
    let kind = if secure_mode {
        ContextKind::SecureDesktop
    } else if secure_field {
        ContextKind::Password
    } else {
        ContextKind::Code
    };
    InputContext::safe(kind, None, 0)
}

/// The decision of the shared policy for this context.
pub fn decide(secure_mode: bool, secure_field: bool) -> Decision {
    policy().decide(&context_of(secure_mode, secure_field))
}

/// Whether the text service may read anything here (the candidates, the text around the caret).
///
/// A protected context never may: the shared policy refuses it before any permission is considered.
pub fn may_collect(secure_mode: bool, secure_field: bool) -> bool {
    decide(secure_mode, secure_field).allows_read()
}

/// Whether the text service may write into the focused document.
///
/// Only a context the policy allows may be written into. A context that may only be read (an unknown one) is not
/// written into either: the text service does not know what it is, so it does not touch it.
pub fn may_write(secure_mode: bool, secure_field: bool) -> bool {
    decide(secure_mode, secure_field).is_allowed()
}

/// The reason of the refusal, when there is one (the same words the extension shows).
pub fn refusal_reason(secure_mode: bool, secure_field: bool) -> Option<&'static str> {
    decide(secure_mode, secure_field).reason()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_ime_core::policy::{
        REASON_BROWSER_SECURE_INPUT, REASON_PASSWORD, REASON_SECURE_DESKTOP, REASON_UAC_PROMPT,
    };

    #[test]
    fn a_password_field_is_a_protected_context() {
        assert!(!may_collect(false, true), "nothing is collected");
        assert!(!may_write(false, true), "nothing is written");
        assert_eq!(
            refusal_reason(false, true),
            Some(REASON_PASSWORD),
            "the reason names the kind of the context"
        );
    }

    #[test]
    fn the_secure_desktop_is_a_protected_context_of_its_own() {
        assert!(!may_collect(true, false));
        assert!(!may_write(true, false));
        assert_eq!(refusal_reason(true, false), Some(REASON_SECURE_DESKTOP));
    }

    #[test]
    fn the_secure_desktop_wins_over_the_answer_of_the_host() {
        // both are protected: the reason of the secure desktop is the one that is reported
        assert_eq!(
            refusal_reason(true, true),
            Some(REASON_SECURE_DESKTOP),
            "the secure desktop is the outer context"
        );
    }

    #[test]
    fn an_ordinary_field_is_allowed() {
        assert!(may_collect(false, false), "the candidates may be shown");
        assert!(may_write(false, false), "the text may be written");
        assert_eq!(refusal_reason(false, false), None);
    }

    #[test]
    fn the_reasons_are_the_ones_the_extension_mirrors() {
        // the words of the refusal are part of what the three sides agree on: the extension has the same strings
        assert_eq!(REASON_PASSWORD, "password: the candidates are disabled in a password control");
        assert_eq!(
            REASON_SECURE_DESKTOP,
            "secure_desktop: the candidates are disabled on the secure desktop"
        );
        assert_eq!(
            REASON_UAC_PROMPT,
            "uac_prompt: the candidates are disabled in an elevation prompt"
        );
        assert_eq!(
            REASON_BROWSER_SECURE_INPUT,
            "browser_secure_input: the candidates are disabled in a secure input field of a browser"
        );
    }

    #[test]
    fn the_policy_of_the_text_service_shares_nothing_by_default() {
        let policy = policy();
        assert!(!policy.allow_excerpt_in_code_context);
        assert!(!policy.allow_agent_orchestration);
        // the context of the user is a code one, and it is still not shared without the permission
        assert!(!policy.may_share_with_agent(&context_of(false, false)));
    }
}
