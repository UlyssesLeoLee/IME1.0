//! Which input method the one-key switch goes to.
//!
//! The input method may be resident (a tray process that lives while the user works) and may be switched with one
//! key: the user is writing in the Japanese input method, presses the switch key, writes code with IME1.0, presses
//! it again and is back in the Japanese input method.
//!
//! The decision itself is pure and lives here so that it can be tested without Windows: the Windows part only
//! answers two questions - which profile is active now, and how do I activate another one.

/// The three languages of the product, in the order the switch key walks through them.
pub const LANGUAGES: [&str; 3] = ["zh-CN", "en-US", "ja-JP"];

/// The profile of the prediction engine for an input language of the product.
///
/// The Chinese and the Japanese profiles are answered by the dictionaries of the core (the pinyin and the
/// kana); the English one is the engine of the code, with the English words behind it.
pub fn profile_of(language: &str) -> &'static str {
    match language {
        "zh-CN" => crate::cjk::PROFILE_ZH,
        "ja-JP" => crate::cjk::PROFILE_JA,
        _ => crate::cjk::PROFILE_CODE,
    }
}

/// The language that follows the one that is active.
///
/// The switch key walks through the three languages of the product: zh-CN, en-US, ja-JP and around. A language that
/// is not one of ours (the profile of another text service) is followed by the first one.
pub fn next_language(current: &str) -> &'static str {
    match LANGUAGES.iter().position(|l| *l == current) {
        Some(index) => LANGUAGES[(index + 1) % LANGUAGES.len()],
        None => LANGUAGES[0],
    }
}

/// The identity of one input method profile of the system.
///
/// The two GUIDs are the ones Windows uses: the class of the text service (the input method itself) and the profile
/// inside it (a language of that text service). They are kept as the text Windows shows them in, because that is
/// how they travel between the registry, the process boundary and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// The class of the text service, in braces, lower case: `{43f95ed1-95b4-421d-8f5a-837128c2d854}`.
    pub clsid: String,
    /// The profile inside the text service, in braces, lower case.
    pub profile: String,
    /// The language of the profile (0x0804 = zh-CN, 0x0409 = en-US, 0x0411 = ja-JP).
    pub langid: u16,
}

impl Profile {
    /// A profile from the three values Windows uses.
    pub fn new(clsid: &str, profile: &str, langid: u16) -> Profile {
        Profile {
            clsid: clsid.to_string(),
            profile: profile.to_string(),
            langid,
        }
    }

    /// Whether this profile belongs to the given text service.
    pub fn is_of(&self, clsid: &str) -> bool {
        self.clsid.eq_ignore_ascii_case(clsid)
    }

    /// The text that travels between the registry and the log: `clsid|profile|langid`.
    ///
    /// It is deliberately not a JSON document: it goes into a registry value that other tools may read, and it must
    /// stay readable without a parser.
    pub fn to_record(&self) -> String {
        format!("{}|{}|{:04x}", self.clsid, self.profile, self.langid)
    }

    /// The profile of a record written by [`Profile::to_record`]. None for anything that is not one.
    pub fn from_record(record: &str) -> Option<Profile> {
        let mut parts = record.split('|');
        let clsid = parts.next()?.trim();
        let profile = parts.next()?.trim();
        let langid = u16::from_str_radix(parts.next()?.trim(), 16).ok()?;
        if parts.next().is_some() || clsid.is_empty() || profile.is_empty() {
            return None;
        }
        Some(Profile::new(clsid, profile, langid))
    }
}

/// What a press of the switch key does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Switch to one of our own profiles.
    ToOurs(Profile),
    /// Switch back to the profile that was active before ours.
    ToPrevious(Profile),
    /// There is nothing to switch to (no other profile is known).
    Nowhere,
}

/// The profiles of IME1.0 itself, in the order they are preferred: the language of the document first when it is one
/// of ours, then the rest.
pub fn own_profiles(clsid: &str, zh: &str, en: &str, ja: &str) -> Vec<Profile> {
    vec![
        Profile::new(clsid, zh, 0x0804),
        Profile::new(clsid, en, 0x0409),
        Profile::new(clsid, ja, 0x0411),
    ]
}

/// Decide what a press of the switch key does.
///
/// * If ours is active, the key goes back to the profile that was active before it (`previous`).
/// * If another input method is active, the key comes to us - to the profile of the language that is active now when
///   we have one, otherwise to the first of ours.
/// * If there is nothing to go to, the key does nothing (it must never leave the user without an input method).
pub fn decide(
    current: Option<&Profile>,
    previous: Option<&Profile>,
    ours: &[Profile],
) -> Switch {
    let Some(current) = current else {
        // The active profile could not be read: the key must not change the input method on a guess.
        return Switch::Nowhere;
    };
    let is_ours = ours.iter().any(|o| o.profile.eq_ignore_ascii_case(&current.profile));
    if is_ours {
        return match previous {
            // going back to the same profile would be a no-op the user would read as "it did not work"
            Some(p) if !ours.iter().any(|o| o.profile.eq_ignore_ascii_case(&p.profile)) => {
                Switch::ToPrevious(p.clone())
            }
            _ => Switch::Nowhere,
        };
    }
    // another input method is active: come to ours, preferring the language that is active now
    let target = ours
        .iter()
        .find(|o| current.langid == o.langid)
        .or_else(|| ours.first());
    match target {
        Some(p) => Switch::ToOurs(p.clone()),
        None => Switch::Nowhere,
    }
}

/// The profile to remember after a switch: the one that was active before, unless it was ours.
///
/// The switch to ours must remember the input method the user came from, so that the second press can go back. A
/// switch back to the previous input method forgets ours (the next press comes to us again, from wherever the user
/// is then).
pub fn remember(current: Option<&Profile>, previous: Option<&Profile>, ours: &[Profile]) -> Option<Profile> {
    let current = current?;
    if ours.iter().any(|o| o.profile.eq_ignore_ascii_case(&current.profile)) {
        // we are leaving ours: the profile to come back to is the one we came from
        previous.cloned()
    } else {
        Some(current.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLSID: &str = "{43f95ed1-95b4-421d-8f5a-837128c2d854}";

    fn ours() -> Vec<Profile> {
        own_profiles(
            CLSID,
            "{223fd919-f8cb-476c-a852-351e7381c801}",
            "{68ed92de-77cf-43b1-a7b2-d3be2686e2a0}",
            "{22e54b52-5082-42d3-9e41-1c2e0ce74dfb}",
        )
    }

    fn japanese() -> Profile {
        Profile::new(
            "{e3600580-1cf5-4305-a3b4-8f1a0f0a2b5d}",
            "{fa550b03-5ad7-411f-a5ac-ca038ec3d5f2}",
            0x0411,
        )
    }

    #[test]
    fn the_switch_key_walks_through_the_three_languages() {
        assert_eq!(next_language("zh-CN"), "en-US");
        assert_eq!(next_language("en-US"), "ja-JP");
        assert_eq!(next_language("ja-JP"), "zh-CN", "and around");
        assert_eq!(
            next_language("de-DE"),
            LANGUAGES[0],
            "a language that is not ours is followed by the first one"
        );
    }

    /// Every language of the product has a profile of the engine: the Chinese and the Japanese are answered by the
    /// dictionaries of the core, the English one by the engine of the code.
    #[test]
    fn every_language_has_a_profile_of_the_engine() {
        assert_eq!(profile_of("zh-CN"), crate::cjk::PROFILE_ZH);
        assert_eq!(profile_of("ja-JP"), crate::cjk::PROFILE_JA);
        assert_eq!(profile_of("en-US"), crate::cjk::PROFILE_CODE);
        assert_eq!(
            profile_of("de-DE"),
            crate::cjk::PROFILE_CODE,
            "a language that is not ours is the code"
        );
    }

    #[test]
    fn a_record_survives_the_round_trip() {
        let p = japanese();
        assert_eq!(Profile::from_record(&p.to_record()), Some(p));
        assert_eq!(Profile::from_record(""), None);
        assert_eq!(Profile::from_record("a|b"), None);
        assert_eq!(Profile::from_record("a|b|zz"), None);
        assert_eq!(Profile::from_record("a|b|0411|extra"), None);
    }

    #[test]
    fn the_key_comes_to_us_when_another_input_method_is_active() {
        let other = japanese();
        // our profile of the language that is active now (Japanese) is the one the key goes to
        let target = decide(Some(&other), None, &ours());
        assert_eq!(target, Switch::ToOurs(ours()[2].clone()));
    }

    #[test]
    fn the_key_prefers_our_profile_of_the_language_that_is_active() {
        let other = japanese();
        // the Japanese profile of IME1.0 is the one that matches the language that is active
        let target = decide(Some(&other), None, &ours());
        assert_eq!(target, Switch::ToOurs(ours()[2].clone()));
    }

    #[test]
    fn the_key_comes_to_our_first_profile_when_no_language_matches() {
        // an input method of a language we do not have (Klingon): the first of ours is the one
        let other = Profile::new(
            "{e3600580-1cf5-4305-a3b4-8f1a0f0a2b5d}",
            "{fa550b03-5ad7-411f-a5ac-ca038ec3d5f2}",
            0x01FF,
        );
        assert_eq!(decide(Some(&other), None, &ours()), Switch::ToOurs(ours()[0].clone()));
    }

    #[test]
    fn the_key_goes_back_to_the_input_method_we_came_from() {
        let other = japanese();
        let mine = ours()[0].clone();
        let target = decide(Some(&mine), Some(&other), &ours());
        assert_eq!(target, Switch::ToPrevious(other));
    }

    #[test]
    fn the_key_does_nothing_when_there_is_nothing_to_go_to() {
        let mine = ours()[0].clone();
        assert_eq!(decide(Some(&mine), None, &ours()), Switch::Nowhere);
        assert_eq!(
            decide(None, None, &ours()),
            Switch::Nowhere,
            "the active profile could not be read: the key does not guess"
        );
        assert_eq!(decide(None, None, &[]), Switch::Nowhere);
    }

    #[test]
    fn the_key_never_goes_back_to_ours_own_profile() {
        let mine = ours()[0].clone();
        // "previous" is one of ours: that is not somewhere to go back to
        assert_eq!(decide(Some(&mine), Some(&mine), &ours()), Switch::Nowhere);
    }

    #[test]
    fn coming_to_us_remembers_the_input_method_we_came_from() {
        let other = japanese();
        assert_eq!(remember(Some(&other), None, &ours()), Some(other.clone()));
        // leaving ours: the profile to come back to is the one we came from
        let mine = ours()[0].clone();
        assert_eq!(remember(Some(&mine), Some(&other), &ours()), Some(other));
    }

    #[test]
    fn going_back_forgets_ours() {
        let mine = ours()[0].clone();
        let other = japanese();
        // we are at another input method now: the next press must come to us again, so ours is forgotten
        assert_eq!(remember(Some(&other), Some(&mine), &ours()), Some(other));
    }
}
