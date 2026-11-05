//! The input context of the input method and the generation that invalidates the candidates of an old focus
//! (DD-IME section 20.2).
//!
//! Two rules of the design live here:
//!
//! * **the context is what the candidates are allowed to see.** RD-IME section 15 says that the input method does
//!   not collect the content of a password control, of the secure desktop, of the whole file of another program or
//!   of any foreground window text. The kind of the context is therefore the first thing the privacy policy looks
//!   at, and the excerpt (the piece of text around the caret) is optional: a context that was built without one
//!   never carries one.
//! * **a change of focus kills the candidates.** DD-IME section 20.2 says that the input method drops the old
//!   candidates as soon as it sees that the input context changed, and that it checks the generation again when it
//!   commits, so that a result of an agent cannot land in another application. The generation is the number that
//!   makes that check possible.

/// The kind of the place the user is typing in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextKind {
    /// An ordinary text field.
    PlainText,
    /// A source code field of an editor.
    Code,
    /// A password control. Nothing is ever read from one.
    Password,
    /// The secure desktop (the logon screen and the other secure surfaces of the system).
    SecureDesktop,
    /// An elevation prompt (UAC).
    UacPrompt,
    /// A restricted AppContainer.
    AppContainer,
    /// A secure input field of a browser.
    BrowserSecureInput,
    /// A place the input method does not know.
    Unknown,
}

impl ContextKind {
    /// Every kind of context.
    pub const ALL: &'static [ContextKind] = &[
        ContextKind::PlainText,
        ContextKind::Code,
        ContextKind::Password,
        ContextKind::SecureDesktop,
        ContextKind::UacPrompt,
        ContextKind::AppContainer,
        ContextKind::BrowserSecureInput,
        ContextKind::Unknown,
    ];

    /// The name of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ContextKind::PlainText => "plain_text",
            ContextKind::Code => "code",
            ContextKind::Password => "password",
            ContextKind::SecureDesktop => "secure_desktop",
            ContextKind::UacPrompt => "uac_prompt",
            ContextKind::AppContainer => "app_container",
            ContextKind::BrowserSecureInput => "browser_secure_input",
            ContextKind::Unknown => "unknown",
        }
    }

    /// The kind of a name; an unknown name is refused.
    pub fn parse(name: &str) -> Option<ContextKind> {
        match name {
            "plain_text" => Some(ContextKind::PlainText),
            "code" => Some(ContextKind::Code),
            "password" => Some(ContextKind::Password),
            "secure_desktop" => Some(ContextKind::SecureDesktop),
            "uac_prompt" => Some(ContextKind::UacPrompt),
            "app_container" => Some(ContextKind::AppContainer),
            "browser_secure_input" => Some(ContextKind::BrowserSecureInput),
            "unknown" => Some(ContextKind::Unknown),
            _ => None,
        }
    }

    /// Whether the candidates must be disabled in this kind of context.
    ///
    /// These are the five cases of DD-IME section 20.2 and of RD-IME section 15: in them the input method offers
    /// nothing, and it does not try to work around the restriction either.
    pub fn is_sensitive(self) -> bool {
        matches!(
            self,
            ContextKind::Password
                | ContextKind::SecureDesktop
                | ContextKind::UacPrompt
                | ContextKind::AppContainer
                | ContextKind::BrowserSecureInput
        )
    }
}

/// The context the user is typing in.
#[derive(Clone, Debug, PartialEq)]
pub struct InputContext {
    /// The kind of the place.
    pub kind: ContextKind,
    /// The language the context suggests, when the host says one.
    pub language_hint: Option<String>,
    /// The number that tells the candidates of which focus they are (DD-IME section 20.2).
    pub focus_context_generation: u64,
    /// A piece of the text around the caret, when the context is allowed to carry one.
    pub excerpt: Option<String>,
}

impl InputContext {
    /// A context without an excerpt: the safe form, and the one every protected context uses.
    pub fn safe(
        kind: ContextKind,
        language_hint: Option<&str>,
        focus_context_generation: u64,
    ) -> InputContext {
        InputContext {
            kind,
            language_hint: language_hint.map(|l| l.to_string()),
            focus_context_generation,
            excerpt: None,
        }
    }

    /// A context with an excerpt of the text around the caret.
    ///
    /// The caller is the one that asks the host for the text: the privacy policy decides whether such an excerpt is
    /// allowed at all (see [`crate::policy::PrivacyPolicy::excerpt_allowed`]).
    pub fn with_excerpt(
        kind: ContextKind,
        language_hint: Option<&str>,
        focus_context_generation: u64,
        excerpt: &str,
    ) -> InputContext {
        InputContext {
            excerpt: Some(excerpt.to_string()),
            ..InputContext::safe(kind, language_hint, focus_context_generation)
        }
    }

    /// A password control: no language, no excerpt, nothing at all.
    pub fn for_password(focus_context_generation: u64) -> InputContext {
        InputContext::safe(ContextKind::Password, None, focus_context_generation)
    }

    /// A source code field of an editor.
    pub fn for_code(language_hint: Option<&str>, focus_context_generation: u64) -> InputContext {
        InputContext::safe(ContextKind::Code, language_hint, focus_context_generation)
    }

    /// The generation the candidates of this context belong to.
    pub fn generation(&self) -> u64 {
        self.focus_context_generation
    }

    /// Whether the candidates of this context are stale (DD-IME section 20.2).
    ///
    /// The input method checks this again when it commits: a result that was made for another focus is dropped
    /// instead of being written into the window that has the focus now.
    pub fn is_stale(&self, current_generation: u64) -> bool {
        current_generation != self.focus_context_generation
    }

    /// The next generation: the context changed, so the old candidates are dropped.
    ///
    /// The excerpt goes with the old focus - it describes a text that is not the one in front of the user any more.
    /// The number saturates at `u64::MAX` instead of wrapping: a wrapped number would look like an old generation
    /// and would let a stale candidate through.
    pub fn advance_generation(&mut self) {
        self.focus_context_generation = self.focus_context_generation.saturating_add(1);
        self.excerpt = None;
    }

    /// The excerpt of the context, when it has one.
    pub fn excerpt(&self) -> Option<&str> {
        self.excerpt.as_deref()
    }

    /// The language of the context, when it has one.
    pub fn language(&self) -> Option<&str> {
        self.language_hint.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dd_0202_the_generation_says_whether_the_candidates_are_stale() {
        let context = InputContext::for_code(Some("rust"), 7);
        assert_eq!(context.generation(), 7);
        assert!(!context.is_stale(7), "the same generation is not stale");
        assert!(context.is_stale(8), "a newer generation makes them stale");
        assert!(context.is_stale(6), "an older generation makes them stale too");
    }

    #[test]
    fn dd_0202_advancing_the_generation_never_panics() {
        let mut context = InputContext::for_code(Some("rust"), 0);
        context.advance_generation();
        assert_eq!(context.generation(), 1);
        context.focus_context_generation = u64::MAX;
        context.advance_generation();
        assert_eq!(
            context.generation(),
            u64::MAX,
            "the number saturates instead of wrapping"
        );
        context.advance_generation();
        assert_eq!(context.generation(), u64::MAX);
    }

    #[test]
    fn dd_0202_a_new_generation_drops_the_excerpt_of_the_old_focus() {
        let mut context = InputContext::with_excerpt(
            ContextKind::Code,
            Some("rust"),
            3,
            "fn main() { /* here */ }",
        );
        assert_eq!(context.excerpt(), Some("fn main() { /* here */ }"));
        context.advance_generation();
        assert_eq!(context.generation(), 4);
        assert_eq!(context.excerpt(), None);
    }

    #[test]
    fn rd_015_a_password_context_never_carries_an_excerpt() {
        let context = InputContext::for_password(1);
        assert_eq!(context.kind, ContextKind::Password);
        assert_eq!(context.excerpt(), None);
        assert_eq!(context.language(), None);
    }

    #[test]
    fn a_safe_context_has_no_excerpt() {
        let context = InputContext::safe(ContextKind::Code, Some("rust"), 2);
        assert_eq!(context.excerpt(), None);
        assert_eq!(context.language(), Some("rust"));
    }

    #[test]
    fn every_context_kind_has_a_name_and_comes_back_from_it() {
        for kind in ContextKind::ALL {
            assert_eq!(ContextKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(ContextKind::ALL.len(), 8);
        assert_eq!(ContextKind::parse("PlainText"), None);
        assert_eq!(ContextKind::parse(""), None);
    }

    #[test]
    fn the_five_protected_kinds_are_the_sensitive_ones() {
        let sensitive: Vec<ContextKind> = ContextKind::ALL
            .iter()
            .copied()
            .filter(|k| k.is_sensitive())
            .collect();
        assert_eq!(
            sensitive,
            vec![
                ContextKind::Password,
                ContextKind::SecureDesktop,
                ContextKind::UacPrompt,
                ContextKind::AppContainer,
                ContextKind::BrowserSecureInput,
            ]
        );
        assert!(!ContextKind::Code.is_sensitive());
        assert!(!ContextKind::PlainText.is_sensitive());
        assert!(!ContextKind::Unknown.is_sensitive());
    }
}
