//! TASE-IME-CORE/1: the pure logic of the input method of TASE (DD-IME, DD section 17.1).
//!
//! This crate is the **human side** of TASE: what the input method shows, in which order, and what it is allowed
//! to look at. It is deliberately free of any platform dependency:
//!
//! * no COM, no TSF, no `windows` crate, no `unsafe` - the adapter that talks to the Windows text service lives in
//!   its own crate (`tase-ime-tsf`), so this one compiles and is tested on every target;
//! * no clock, no random, no file, no network - the ranking is a pure function of the candidates and of the request,
//!   so the same input always gives the same order;
//! * no way out around a protected context: the model has no field for the position of the candidate window, for a
//!   key event or for the handle of the focused window, because an agent that chooses the same asset gets a
//!   `TasePlan`/`plan_digest` instead of those (DD-IME section 20.4).
//!
//! The four modules are the four questions of the input method:
//!
//! | module | question |
//! |---|---|
//! | [`candidate`] | what is a candidate? |
//! | [`context`] | where is the user typing, and is this candidate still the right one? |
//! | [`rank`] | which candidate comes first, and why? |
//! | [`policy`] | what may the input method do at all? |
//!
//! The crate is built on [`pime_core`]: the strict JSON parser and the error codes come from there, and no other
//! dependency is allowed.
#![forbid(unsafe_code)]

pub mod candidate;
pub mod context;
pub mod policy;
pub mod rank;

pub use candidate::{AssetRef, Candidate, CandidateStatus};
pub use context::{ContextKind, InputContext};
pub use policy::{
    Decision, PrivacyPolicy, FORBIDDEN_SHORTCUTS, LOCAL_CACHE_P95_BUDGET_MS,
};
pub use rank::{rank, score_candidate, RankRequest, RankScore, RankedCandidate};

/// The version of the crate.
pub const VERSION: &str = "0.1.0";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::Candidate;
    use crate::context::{ContextKind, InputContext};
    use crate::rank::RankRequest;

    #[test]
    fn the_crate_offers_the_candidate_the_context_the_rank_and_the_policy() {
        let asset = AssetRef::parse("pattern.gof.behavioral.strategy@1.0.0").expect("an asset");
        let candidate = Candidate::new("cand-104", asset);
        let context = InputContext::for_code(Some("rust"), 1);
        let request = RankRequest::new("strategy", Some("rust"), context);
        let ranked = rank(&[candidate], &request);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.candidate_id, "cand-104");
        assert_eq!(
            PrivacyPolicy::default().decide(&InputContext::safe(
                ContextKind::Password,
                None,
                1
            )),
            Decision::Deny(policy::REASON_PASSWORD)
        );
        assert_eq!(VERSION, "0.1.0");
    }
}
