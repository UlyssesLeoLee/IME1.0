//! DD-10 section 12.1: registration / uninstall PLAN generator (UT-002, UT-061).
//!
//! The plan is data: a list of operations an installer would perform. It can only ever mention this product's
//! CLSID and the three product profile GUIDs; there is deliberately NO operation type for changing the default
//! input method or for touching another IME's profile, so such a plan cannot be expressed at all.
//! [`SystemState`] is a fake of the OS registration state used to prove the effect of a plan in tests.

use crate::profiles::{Guid, LanguageProfile, PIME_CLSID, builtin_profiles, is_own_guid};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanOp {
    /// `ITfInputProcessorProfiles::Register` (the COM class / text service).
    RegisterTip {
        clsid: Guid,
    },
    /// Category registration (keyboard TIP category).
    RegisterCategory {
        clsid: Guid,
    },
    AddLanguageProfile {
        clsid: Guid,
        langid: u16,
        profile: Guid,
    },
    RemoveLanguageProfile {
        clsid: Guid,
        langid: u16,
        profile: Guid,
    },
    UnregisterCategory {
        clsid: Guid,
    },
    UnregisterTip {
        clsid: Guid,
    },
    /// Removal of the COM class registration written by this installer.
    DeleteComRegistration {
        clsid: Guid,
    },
}

impl PlanOp {
    /// Every GUID the operation refers to.
    pub fn guids(&self) -> Vec<Guid> {
        match self {
            PlanOp::RegisterTip { clsid }
            | PlanOp::RegisterCategory { clsid }
            | PlanOp::UnregisterCategory { clsid }
            | PlanOp::UnregisterTip { clsid }
            | PlanOp::DeleteComRegistration { clsid } => vec![*clsid],
            PlanOp::AddLanguageProfile { clsid, profile, .. }
            | PlanOp::RemoveLanguageProfile { clsid, profile, .. } => vec![*clsid, *profile],
        }
    }

    fn is_removal(&self) -> bool {
        matches!(
            self,
            PlanOp::RemoveLanguageProfile { .. }
                | PlanOp::UnregisterCategory { .. }
                | PlanOp::UnregisterTip { .. }
                | PlanOp::DeleteComRegistration { .. }
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub ops: Vec<PlanOp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    NothingSelected,
    UnknownLanguage(u16),
    DuplicateSelection(u16),
    /// A plan mentions a GUID that does not belong to this product.
    ForeignGuid(Guid),
}

impl Plan {
    /// Rejects any operation that refers to a GUID other than this product's CLSID / profile GUIDs.
    pub fn validate_own_only(&self) -> Result<(), PlanError> {
        for op in &self.ops {
            if let Some(g) = op.guids().into_iter().find(|g| !is_own_guid(g)) {
                return Err(PlanError::ForeignGuid(g));
            }
        }
        Ok(())
    }

    pub fn touches_foreign_guid(&self) -> bool {
        self.validate_own_only().is_err()
    }

    pub fn removals(&self) -> impl Iterator<Item = &PlanOp> {
        self.ops.iter().filter(|o| o.is_removal())
    }
}

fn own_profile(langid: u16) -> Option<LanguageProfile> {
    builtin_profiles().into_iter().find(|p| p.langid == langid)
}

/// Registration plan for the languages the user ticked in the installer (any subset of zh-CN/en-US/ja-JP).
pub fn plan_install(selected_langids: &[u16]) -> Result<Plan, PlanError> {
    if selected_langids.is_empty() {
        return Err(PlanError::NothingSelected);
    }
    let mut seen = HashSet::new();
    let mut profiles = Vec::new();
    for &l in selected_langids {
        let p = own_profile(l).ok_or(PlanError::UnknownLanguage(l))?;
        if !seen.insert(l) {
            return Err(PlanError::DuplicateSelection(l));
        }
        profiles.push(p);
    }
    let mut ops = vec![
        PlanOp::RegisterTip { clsid: PIME_CLSID },
        PlanOp::RegisterCategory { clsid: PIME_CLSID },
    ];
    ops.extend(profiles.iter().map(|p| PlanOp::AddLanguageProfile {
        clsid: p.clsid,
        langid: p.langid,
        profile: p.profile,
    }));
    let plan = Plan { ops };
    plan.validate_own_only()?;
    Ok(plan)
}

/// One profile as it exists in the (fake) OS registration state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstalledProfile {
    pub clsid: Guid,
    pub profile: Guid,
    pub langid: u16,
}

/// Fake of the OS text-service state: registered classes, language profiles and the default keyboard per language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemState {
    pub registered_clsids: HashSet<Guid>,
    pub profiles: Vec<InstalledProfile>,
    pub default_keyboard: HashMap<u16, Guid>,
}

impl SystemState {
    /// Applies a plan like an installer would. Refuses plans that mention foreign GUIDs; never touches
    /// `default_keyboard` except for dropping an entry whose profile no longer exists (OS fallback behaviour).
    pub fn apply(&mut self, plan: &Plan) -> Result<(), PlanError> {
        plan.validate_own_only()?;
        for op in &plan.ops {
            match op {
                PlanOp::RegisterTip { clsid } => {
                    self.registered_clsids.insert(*clsid);
                }
                PlanOp::RegisterCategory { .. } | PlanOp::UnregisterCategory { .. } => {}
                PlanOp::AddLanguageProfile {
                    clsid,
                    langid,
                    profile,
                } => {
                    if !self.profiles.iter().any(|p| p.profile == *profile) {
                        self.profiles.push(InstalledProfile {
                            clsid: *clsid,
                            profile: *profile,
                            langid: *langid,
                        });
                    }
                }
                PlanOp::RemoveLanguageProfile { profile, .. } => {
                    self.profiles.retain(|p| p.profile != *profile);
                }
                PlanOp::UnregisterTip { clsid } | PlanOp::DeleteComRegistration { clsid } => {
                    self.registered_clsids.remove(clsid);
                }
            }
        }
        let live: HashSet<Guid> = self.profiles.iter().map(|p| p.profile).collect();
        self.default_keyboard
            .retain(|_, g| live.contains(g) || !is_own_guid(g));
        Ok(())
    }
}

/// Uninstall plan: removes only this product's profiles that are actually installed, its category and its class.
pub fn plan_uninstall(state: &SystemState) -> Plan {
    let mut ops: Vec<PlanOp> = state
        .profiles
        .iter()
        .filter(|p| p.clsid == PIME_CLSID && is_own_guid(&p.profile))
        .map(|p| PlanOp::RemoveLanguageProfile {
            clsid: p.clsid,
            langid: p.langid,
            profile: p.profile,
        })
        .collect();
    if state.registered_clsids.contains(&PIME_CLSID) || !ops.is_empty() {
        ops.push(PlanOp::UnregisterCategory { clsid: PIME_CLSID });
        ops.push(PlanOp::UnregisterTip { clsid: PIME_CLSID });
        ops.push(PlanOp::DeleteComRegistration { clsid: PIME_CLSID });
    }
    Plan { ops }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{PROFILE_EN, PROFILE_JA, PROFILE_ZH};

    // Fake OS GUIDs (test doubles; they are NOT the real Microsoft IME GUIDs).
    const OS_CLSID_PINYIN: Guid = Guid::new(0xAAAA0001, 1, 1, [1; 8]);
    const OS_PROFILE_PINYIN: Guid = Guid::new(0xAAAA0002, 1, 1, [2; 8]);
    const OS_CLSID_JAPANESE: Guid = Guid::new(0xBBBB0001, 1, 1, [3; 8]);
    const OS_PROFILE_JAPANESE: Guid = Guid::new(0xBBBB0002, 1, 1, [4; 8]);

    fn os_state() -> SystemState {
        let mut s = SystemState::default();
        s.registered_clsids
            .extend([OS_CLSID_PINYIN, OS_CLSID_JAPANESE]);
        s.profiles.push(InstalledProfile {
            clsid: OS_CLSID_PINYIN,
            profile: OS_PROFILE_PINYIN,
            langid: 0x0804,
        });
        s.profiles.push(InstalledProfile {
            clsid: OS_CLSID_JAPANESE,
            profile: OS_PROFILE_JAPANESE,
            langid: 0x0411,
        });
        s.default_keyboard.insert(0x0804, OS_PROFILE_PINYIN);
        s.default_keyboard.insert(0x0411, OS_PROFILE_JAPANESE);
        s
    }

    #[test]
    fn ut_002_install_plan_never_changes_default_ime_or_other_profiles() {
        let before = os_state();
        let plan = plan_install(&[0x0804, 0x0409, 0x0411]).unwrap();
        assert!(!plan.touches_foreign_guid());
        assert_eq!(
            plan.removals().count(),
            0,
            "an install plan removes nothing"
        );
        let mut after = before.clone();
        after.apply(&plan).unwrap();
        assert_eq!(
            after.default_keyboard, before.default_keyboard,
            "default keyboard targets are unchanged"
        );
        for p in &before.profiles {
            assert!(
                after.profiles.contains(p),
                "existing profile {} survives",
                p.profile
            );
        }
        assert!(
            after.registered_clsids.contains(&OS_CLSID_PINYIN)
                && after.registered_clsids.contains(&OS_CLSID_JAPANESE)
        );
        let own: Vec<Guid> = after
            .profiles
            .iter()
            .filter(|p| p.clsid == PIME_CLSID)
            .map(|p| p.profile)
            .collect();
        assert_eq!(own, vec![PROFILE_ZH, PROFILE_EN, PROFILE_JA]);
    }

    #[test]
    fn ut_002_selection_errors_and_subsets() {
        assert_eq!(plan_install(&[]), Err(PlanError::NothingSelected));
        assert_eq!(
            plan_install(&[0x0407]),
            Err(PlanError::UnknownLanguage(0x0407))
        );
        assert_eq!(
            plan_install(&[0x0804, 0x0804]),
            Err(PlanError::DuplicateSelection(0x0804))
        );
        let plan = plan_install(&[0x0411]).unwrap();
        let langs: Vec<u16> = plan
            .ops
            .iter()
            .filter_map(|o| {
                if let PlanOp::AddLanguageProfile { langid, .. } = o {
                    Some(*langid)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(langs, vec![0x0411]);
    }

    #[test]
    fn ut_061_uninstall_removes_only_own_guids() {
        let mut state = os_state();
        state
            .apply(&plan_install(&[0x0804, 0x0409, 0x0411]).unwrap())
            .unwrap();
        let os_only = os_state();
        let plan = plan_uninstall(&state);
        assert!(!plan.touches_foreign_guid());
        let removed: HashSet<Guid> = plan.ops.iter().flat_map(|o| o.guids()).collect();
        assert!(!removed.contains(&OS_PROFILE_PINYIN) && !removed.contains(&OS_PROFILE_JAPANESE));
        assert!(!removed.contains(&OS_CLSID_PINYIN) && !removed.contains(&OS_CLSID_JAPANESE));
        assert_eq!(
            plan.ops
                .iter()
                .filter(|o| matches!(o, PlanOp::RemoveLanguageProfile { .. }))
                .count(),
            3
        );
        state.apply(&plan).unwrap();
        assert_eq!(
            state, os_only,
            "the system is back to exactly the other IMEs, default IME configuration unchanged"
        );
    }

    #[test]
    fn ut_061_uninstall_on_a_clean_system_is_empty_and_idempotent() {
        let state = os_state();
        assert!(plan_uninstall(&state).ops.is_empty());
        let mut s = state.clone();
        s.apply(&plan_install(&[0x0409]).unwrap()).unwrap();
        let un = plan_uninstall(&s);
        s.apply(&un).unwrap();
        assert!(plan_uninstall(&s).ops.is_empty());
    }

    #[test]
    fn ut_061_plans_mentioning_foreign_guids_are_refused() {
        let evil = Plan {
            ops: vec![PlanOp::RemoveLanguageProfile {
                clsid: OS_CLSID_PINYIN,
                langid: 0x0804,
                profile: OS_PROFILE_PINYIN,
            }],
        };
        assert_eq!(
            evil.validate_own_only(),
            Err(PlanError::ForeignGuid(OS_CLSID_PINYIN))
        );
        let mut state = os_state();
        let before = state.clone();
        assert!(state.apply(&evil).is_err());
        assert_eq!(state, before, "a refused plan changes nothing");
    }

    #[test]
    fn ut_061_default_pointing_at_own_profile_is_only_dropped_not_reassigned() {
        let mut state = os_state();
        state.apply(&plan_install(&[0x0409]).unwrap()).unwrap();
        state.default_keyboard.insert(0x0409, PROFILE_EN);
        let plan = plan_uninstall(&state);
        state.apply(&plan).unwrap();
        assert!(
            !state.default_keyboard.contains_key(&0x0409),
            "no replacement default is chosen by the plan"
        );
        assert_eq!(
            state.default_keyboard.get(&0x0804),
            Some(&OS_PROFILE_PINYIN)
        );
    }
}
