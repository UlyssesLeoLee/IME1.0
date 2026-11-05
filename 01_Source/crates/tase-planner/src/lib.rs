//! TASE planner: the decision routing, the trimming of the context, the validation of a plan and the digest of a
//! plan (detailed design §2.3 and §9.1, basic design §3.1).
//!
//! The planner is the part of TASE that decides **how** a change is written and **what** a change is. It never writes
//! a file: the design book says it in one line at the end of the pipeline of §2.3 — "Planner 不执行文件写入" — and this
//! crate follows it literally. The planner touches no disk, no network and no git: every function of this crate is a
//! pure function of its arguments, so a plan can be built, checked and digested without a repository and without a
//! worker.
//!
//! # What the crate holds
//!
//! * [`strategy`] — the decision routing of basic design §3.1: which route a request takes (template, semantic,
//!   freeform, test) and why;
//! * [`context`] — the trimming of the context of a model call (detailed design §9.1): what goes into the call and
//!   what was left out;
//! * [`validator`] — the static checks of a plan: the findings of a request that the plan service refuses;
//! * [`plan_hash`] — the canonical JSON of a plan and its SHA-256 digest, which is the identity of the plan.
//!
//! # The pipeline of the plan service
//!
//! The plan service of detailed design §2.3 runs nine steps: identity, schema, the permission of the repository and
//! of the branch, the content addresses, the pinning of the template, the static checks, the order of the operations,
//! the canonical JSON, the digest and the save of the `plan_id`. The steps that this crate covers are the static
//! checks ([`validator::validate_plan`]), the canonical JSON and the digest ([`plan_hash::canonical_json`] and
//! [`plan_hash::digest_of`]). The steps that need the repository, the registry or the semantic layer are not here:
//! this crate never goes and looks.
//!
//! The crate has no third-party dependency: the JSON and the SHA-256 come from `pime-core` and the DTOs from
//! `tase-contract`.

#![forbid(unsafe_code)]

pub mod context;
pub mod plan_hash;
pub mod strategy;
pub mod validator;

pub use context::{
    ContextBudget, ContextItem, Selection, Trim, select, select_with_report, summarise,
};
pub use plan_hash::{canonical_json, digest_of, plan_digest};
pub use strategy::{
    CODE_FREEFORM_NOT_ALLOWED, RouteDecision, RouteInput, RoutingError, Strategy, StrategyPolicy,
    decide, decide_with_policy, route, route_checked, route_with_policy,
};
pub use validator::{ValidationIssue, is_valid, validate_plan};

/// The version of the planner.
pub const PLANNER_VERSION: &str = "1.1.0";
