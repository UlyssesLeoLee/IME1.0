//! Edit plans and the Prepare/Apply transaction (FR-035/036/037/040, NFR-502, DD section 9.4), port of
//! `core/transaction.js` `EditCoordinator`.
//!
//! Rules enforced here: a single current file; explicit confirmation (the literal `true`); baseline version +
//! hash re-check; a one-use token that is marked used BEFORE the write; read-back verification;
//! `EDIT_UNKNOWN` is final (no automatic retry).
//!
//! The JavaScript coordinator calls the editor itself. The core cannot, so `apply` is split in two:
//!
//! * [`Coordinator::begin_apply`] re-validates everything, marks the token used and hands back the edits to write
//!   (and starts a deadline);
//! * [`Coordinator::finish_apply`] receives what the editor said (accepted / rejected / failed) and the document
//!   state after the write, verifies the hash and answers `APPLIED` or `EDIT_UNKNOWN`.
//!
//! A `finish_apply` that does not arrive within the deadline is `EDIT_UNKNOWN` on the next call that touches that
//! plan, and a `finish_apply` that arrives too late is `EDIT_UNKNOWN`, never `APPLIED`. Time is always passed in
//! (`now_ms`), so tests use a manual clock.

use crate::context::{Snapshot, assert_full_file_writable};
use crate::error::{Result, fail};
use crate::ids::IdGen;
use crate::sha256::sha256_hex;
use crate::textrange::{self, Edit, Range};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub plan_ttl_ms: u64,
    pub token_ttl_ms: u64,
    pub apply_timeout_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            plan_ttl_ms: 10 * 60 * 1000,
            token_ttl_ms: 60 * 1000,
            apply_timeout_ms: 5000,
        }
    }
}

/// How long the record of an apply that never finished is kept (so that a late `finish_apply` still answers
/// `EDIT_UNKNOWN`) after its deadline.
const PENDING_GRACE_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Full,
    Insert,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "full" => Some(Kind::Full),
            "insert" => Some(Kind::Insert),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Full => "full",
            Kind::Insert => "insert",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    ReviewReady,
    Prepared,
    Stale,
    Unknown,
    Applied,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::ReviewReady => "REVIEW_READY",
            Status::Prepared => "PREPARED",
            Status::Stale => "STALE",
            Status::Unknown => "UNKNOWN",
            Status::Applied => "APPLIED",
        }
    }
}

/// The state of the document as the editor reports it right now (`text` is needed: the core does the hashing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocState {
    pub uri: String,
    pub version: i64,
    pub text: String,
    pub is_closed: bool,
    /// `false` = read-only (`EDIT_REJECTED`); the default is writable.
    pub editable: bool,
    pub selection: Option<Range>,
}

impl DocState {
    pub fn new(uri: &str, version: i64, text: &str) -> DocState {
        DocState {
            uri: uri.to_string(),
            version,
            text: text.to_string(),
            is_closed: false,
            editable: true,
            selection: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub id: String,
    pub kind: Kind,
    pub uri: String,
    pub editor_instance: String,
    pub baseline_version: i64,
    pub baseline_hash: String,
    pub edits: Vec<Edit>,
    pub selection: Option<Range>,
    pub proposed_hash: String,
    pub expires_at: u64,
    pub status: Status,
}

struct Token {
    plan_id: String,
    used: bool,
    expires_at: u64,
}

struct Pending {
    plan_id: String,
    deadline: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub plans: u64,
    pub prepared: u64,
    pub apply_calls: u64,
    pub writes: u64,
}

/// What the editor answered to the write that `begin_apply` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// `applyEdit` resolved with a truthy value.
    Accepted,
    /// `applyEdit` resolved with `false`: the editor refused the edit.
    Rejected,
    /// `applyEdit` threw, or the bridge gave up waiting for it.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub new_version: i64,
    pub new_hash: String,
}

pub struct PlanRequest<'a> {
    pub kind: &'a str,
    pub snapshot: &'a Snapshot,
    pub edits: Vec<Edit>,
    /// Every document the proposal touches; each must be the snapshot's document.
    pub targets: Vec<String>,
    pub selection: Option<Range>,
}

pub struct PrepareArgs<'a> {
    pub state: Option<&'a DocState>,
    /// Must be the literal `true` of a deliberate user action.
    pub confirmed: bool,
    /// The permission broker's verdict.
    pub grant_ok: bool,
    /// The syntax check's verdict; `false` disables the default apply.
    pub validation_ok: bool,
}

pub struct Coordinator {
    pub timing: Timing,
    plans: HashMap<String, Plan>,
    tokens: HashMap<String, Token>,
    pending: HashMap<String, Pending>,
    pub stats: Stats,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new(Timing::default())
    }
}

fn same_selection(a: &Option<Range>, b: &Option<Range>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a == b)
}

impl Coordinator {
    pub fn new(timing: Timing) -> Coordinator {
        Coordinator {
            timing,
            plans: HashMap::new(),
            tokens: HashMap::new(),
            pending: HashMap::new(),
            stats: Stats::default(),
        }
    }

    pub fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.get(id)
    }

    pub fn plan_count(&self) -> usize {
        self.plans.len()
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// `edits` are UTF-16 offsets into `snapshot.content`.
    pub fn create_plan(&mut self, now_ms: u64, ids: &mut IdGen, req: PlanRequest) -> Result<Plan> {
        let Some(kind) = Kind::parse(req.kind) else {
            return fail("PATCH_INVALID", "unknown plan kind");
        };
        if req.targets.iter().any(|t| *t != req.snapshot.document_uri) {
            return fail("CROSS_FILE_DENIED", "v1 edits only the current file");
        }
        if kind == Kind::Full {
            assert_full_file_writable(Some(req.snapshot))?;
        }
        if req.edits.is_empty() {
            return fail("PATCH_INVALID", "plan has no edits");
        }
        let sorted = textrange::validate_offsets(&req.snapshot.content, &req.edits)?;
        let proposed = textrange::apply_edits(&req.snapshot.content, &sorted)?;
        let plan = Plan {
            id: ids.next("plan", 16),
            kind,
            uri: req.snapshot.document_uri.clone(),
            editor_instance: req.snapshot.editor_instance_id.clone(),
            baseline_version: req.snapshot.document_version,
            baseline_hash: req.snapshot.sha256.clone(),
            edits: sorted,
            selection: if kind == Kind::Insert {
                req.selection.or(req.snapshot.selection)
            } else {
                None
            },
            proposed_hash: sha256_hex(&proposed),
            expires_at: now_ms.saturating_add(self.timing.plan_ttl_ms),
            status: Status::ReviewReady,
        };
        self.plans.insert(plan.id.clone(), plan.clone());
        self.stats.plans += 1;
        Ok(plan)
    }

    /// Plans whose `finish_apply` did not arrive in time have an unknown result from now on.
    fn expire_pending(&mut self, now_ms: u64) {
        for p in self.pending.values() {
            if now_ms > p.deadline
                && let Some(plan) = self.plans.get_mut(&p.plan_id)
                && plan.status == Status::Prepared
            {
                plan.status = Status::Unknown;
            }
        }
    }

    /// The document changed, closed or lost focus: nothing prepared from the old baseline may be applied.
    pub fn invalidate(&mut self, now_ms: u64, uri: &str) -> usize {
        self.expire_pending(now_ms);
        let mut n = 0;
        for p in self.plans.values_mut() {
            if p.uri == uri && matches!(p.status, Status::ReviewReady | Status::Prepared) {
                p.status = Status::Stale;
                n += 1;
            }
        }
        n
    }

    fn check(plan: &mut Plan, now_ms: u64, state: Option<&DocState>) -> Result<()> {
        if plan.status == Status::Stale {
            return fail("STALE_DOCUMENT", "the document changed after the preview");
        }
        if plan.status == Status::Unknown {
            return fail(
                "EDIT_UNKNOWN",
                "an earlier apply of this plan has an unknown result; it is not retried",
            );
        }
        if plan.expires_at <= now_ms {
            return fail("STALE_RESULT", "proposal expired");
        }
        let Some(state) = state.filter(|s| !s.is_closed) else {
            return fail("DOCUMENT_CLOSED", "document is closed");
        };
        if state.uri != plan.uri
            || state.version != plan.baseline_version
            || sha256_hex(&state.text) != plan.baseline_hash
        {
            plan.status = Status::Stale;
            return fail(
                "STALE_DOCUMENT",
                "the document version or content differs from the reviewed baseline",
            );
        }
        if !state.editable {
            return fail("EDIT_REJECTED", "document is read-only");
        }
        if plan.kind == Kind::Insert && !same_selection(&plan.selection, &state.selection) {
            plan.status = Status::Stale;
            return fail(
                "STALE_SELECTION",
                "the cursor or selection moved since the proposal was made",
            );
        }
        Ok(())
    }

    /// `confirmed` must come from a deliberate user action; `grant_ok` from the permission broker; the validation
    /// verdict from the validators. Returns the one-use apply token.
    pub fn prepare(
        &mut self,
        now_ms: u64,
        ids: &mut IdGen,
        plan_id: &str,
        args: PrepareArgs,
    ) -> Result<String> {
        self.expire_pending(now_ms);
        let Some(plan) = self.plans.get_mut(plan_id) else {
            return fail("TOKEN_INVALID", "unknown plan");
        };
        if !args.confirmed {
            return fail(
                "USER_CONFIRMATION_REQUIRED",
                "explicit confirmation is required before anything is written",
            );
        }
        if !args.grant_ok {
            return fail("GRANT_DENIED", "authorisation is missing or revoked");
        }
        Self::check(plan, now_ms, args.state)?;
        if !args.validation_ok {
            return fail(
                "VALIDATION_FAILED",
                "syntax check failed; default apply is disabled",
            );
        }
        let token_id = ids.next("token", 16);
        self.tokens.insert(
            token_id.clone(),
            Token {
                plan_id: plan_id.to_string(),
                used: false,
                expires_at: now_ms.saturating_add(self.timing.token_ttl_ms),
            },
        );
        plan.status = Status::Prepared;
        self.stats.prepared += 1;
        Ok(token_id)
    }

    /// Re-validates, marks the token used (before anything is written: a replay can never write twice) and returns
    /// the edits the bridge must write in ONE editor edit. The result is reported with [`Self::finish_apply`].
    pub fn begin_apply(
        &mut self,
        now_ms: u64,
        token_id: &str,
        state: Option<&DocState>,
    ) -> Result<Vec<Edit>> {
        self.expire_pending(now_ms);
        self.stats.apply_calls += 1;
        let Some(token) = self.tokens.get(token_id) else {
            return fail(
                "TOKEN_INVALID",
                "apply token is unknown, already used or expired",
            );
        };
        if token.used || token.expires_at <= now_ms {
            return fail(
                "TOKEN_INVALID",
                "apply token is unknown, already used or expired",
            );
        }
        let plan_id = token.plan_id.clone();
        // another write of this document is still waiting for its result: nothing is consumed, nothing is written
        let uri = self.plans.get(&plan_id).map(|p| p.uri.clone());
        if let Some(uri) = uri {
            let busy = self.pending.values().any(|p| {
                now_ms <= p.deadline && self.plans.get(&p.plan_id).is_some_and(|o| o.uri == uri)
            });
            if busy {
                return fail(
                    "EDIT_REJECTED",
                    "another apply of this document has not finished yet",
                );
            }
        }
        if let Some(t) = self.tokens.get_mut(token_id) {
            t.used = true; // consumed before the first write
        }
        let Some(plan) = self.plans.get_mut(&plan_id) else {
            return fail("TOKEN_INVALID", "the plan of this token is gone");
        };
        Self::check(plan, now_ms, state)?;
        self.stats.writes += 1;
        let edits = plan.edits.clone();
        self.pending.insert(
            token_id.to_string(),
            Pending {
                plan_id,
                deadline: now_ms.saturating_add(self.timing.apply_timeout_ms),
            },
        );
        Ok(edits)
    }

    /// The editor's answer to the write of [`Self::begin_apply`] plus the document state after it.
    pub fn finish_apply(
        &mut self,
        now_ms: u64,
        token_id: &str,
        outcome: ApplyOutcome,
        state: Option<&DocState>,
    ) -> Result<Applied> {
        self.expire_pending(now_ms);
        let Some(pending) = self.pending.remove(token_id) else {
            return fail("TOKEN_INVALID", "no apply is in progress for this token");
        };
        let Some(plan) = self.plans.get_mut(&pending.plan_id) else {
            return fail(
                "EDIT_UNKNOWN",
                "the plan is gone; the result of the write is unknown",
            );
        };
        let unknown = |plan: &mut Plan, message: &str| -> Result<Applied> {
            plan.status = Status::Unknown;
            fail("EDIT_UNKNOWN", message)
        };
        if now_ms > pending.deadline || plan.status == Status::Unknown {
            return unknown(
                plan,
                "the editor did not confirm the edit in time; check the document manually",
            );
        }
        match outcome {
            ApplyOutcome::Failed => unknown(
                plan,
                "the editor did not confirm the edit; check the document manually",
            ),
            ApplyOutcome::Rejected => {
                if state.is_some_and(|s| sha256_hex(&s.text) == plan.baseline_hash) {
                    plan.status = Status::Stale;
                    return fail(
                        "EDIT_REJECTED",
                        "the editor rejected the edit; nothing was written",
                    );
                }
                unknown(
                    plan,
                    "the editor rejected the edit but the document changed",
                )
            }
            ApplyOutcome::Accepted => match state {
                Some(after) if sha256_hex(&after.text) == plan.proposed_hash => {
                    plan.status = Status::Applied;
                    Ok(Applied {
                        new_version: after.version,
                        new_hash: plan.proposed_hash.clone(),
                    })
                }
                _ => unknown(
                    plan,
                    "read-back does not match the proposed text; the edit is not retried",
                ),
            },
        }
    }

    pub fn discard(&mut self, plan_id: &str) {
        self.plans.remove(plan_id);
    }

    pub fn status_of(&mut self, now_ms: u64, plan_id: &str) -> &'static str {
        self.expire_pending(now_ms);
        self.plans
            .get(plan_id)
            .map_or("UNKNOWN_PLAN", |p| p.status.as_str())
    }

    /// Drops expired plans, used tokens and applied plans (source fragments must not linger, DD section 11).
    /// The record of an apply that is still waiting for its result stays until the grace period is over.
    pub fn purge(&mut self, now_ms: u64) {
        self.expire_pending(now_ms);
        self.pending
            .retain(|_, p| now_ms <= p.deadline.saturating_add(PENDING_GRACE_MS));
        let waiting: Vec<String> = self.pending.values().map(|p| p.plan_id.clone()).collect();
        self.plans.retain(|id, p| {
            waiting.contains(id) || !(p.expires_at <= now_ms || p.status == Status::Applied)
        });
        let live_pending = &self.pending;
        self.tokens
            .retain(|id, k| live_pending.contains_key(id) || !(k.expires_at <= now_ms || k.used));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{SnapshotInput, make_snapshot};
    use crate::textrange::pos;

    const URI: &str = "file:///w/a.rs";
    const T0: u64 = 1_000_000;
    const TEXT: &str = "fn a() {}\nfn b() {}\n";

    fn ids() -> IdGen {
        let entropy: Vec<u8> = (0..32u8)
            .map(|i| i.wrapping_mul(11).wrapping_add(7))
            .collect();
        IdGen::new(&entropy).unwrap()
    }

    fn snap(text: &str) -> Snapshot {
        let mut i = SnapshotInput::new(URI, 42, text);
        i.language_id = "rust".to_string();
        i.selection = Some(Range {
            start: pos(0, 0),
            end: pos(0, 0),
        });
        make_snapshot(i).unwrap()
    }

    fn snap_with(text: &str, level: &str) -> Snapshot {
        let mut i = SnapshotInput::new(URI, 42, text);
        i.level = level.to_string();
        make_snapshot(i).unwrap()
    }

    fn e(start: usize, end: usize, text: &str) -> Edit {
        Edit {
            start,
            end,
            text: text.to_string(),
        }
    }

    /// A fake editor document with a version counter, an undo stack and failure modes for the write.
    struct FakeDoc {
        uri: String,
        text: String,
        version: i64,
        history: Vec<String>,
        closed: bool,
        selection: Option<Range>,
        apply_calls: u32,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Ok,
        Throw,
        Hang,
        Reject,
        WrongText,
    }

    impl FakeDoc {
        fn new(text: &str) -> FakeDoc {
            FakeDoc {
                uri: URI.to_string(),
                text: text.to_string(),
                version: 42,
                history: vec![],
                closed: false,
                selection: None,
                apply_calls: 0,
            }
        }
        fn user_edit(&mut self, text: &str) {
            self.history
                .push(std::mem::replace(&mut self.text, text.to_string()));
            self.version += 1;
        }
        fn undo(&mut self) {
            if let Some(t) = self.history.pop() {
                self.text = t;
                self.version += 1;
            }
        }
        fn state(&self) -> DocState {
            DocState {
                uri: self.uri.clone(),
                version: self.version,
                text: self.text.clone(),
                is_closed: self.closed,
                editable: true,
                selection: self.selection,
            }
        }
        /// The editor's side of an apply: what `begin_apply` returned is written according to the mode.
        fn write(&mut self, edits: &[Edit], mode: Mode) -> ApplyOutcome {
            self.apply_calls += 1;
            match mode {
                Mode::Throw | Mode::Hang => ApplyOutcome::Failed,
                Mode::Reject => ApplyOutcome::Rejected,
                Mode::WrongText => {
                    let t = textrange::apply_edits(&self.text, edits).unwrap() + "\n// extra";
                    self.user_edit(&t);
                    ApplyOutcome::Accepted
                }
                Mode::Ok => {
                    let t = textrange::apply_edits(&self.text, edits).unwrap();
                    self.user_edit(&t);
                    ApplyOutcome::Accepted
                }
            }
        }
    }

    struct Rig {
        co: Coordinator,
        ids: IdGen,
        now: u64,
    }

    impl Rig {
        fn new() -> Rig {
            Rig {
                co: Coordinator::new(Timing {
                    apply_timeout_ms: 80,
                    ..Timing::default()
                }),
                ids: ids(),
                now: T0,
            }
        }
        fn plan(&mut self, kind: &str, s: &Snapshot, edits: Vec<Edit>) -> Result<Plan> {
            self.co.create_plan(
                self.now,
                &mut self.ids,
                PlanRequest {
                    kind,
                    snapshot: s,
                    edits,
                    targets: vec![],
                    selection: None,
                },
            )
        }
        fn full(&mut self, s: &Snapshot) -> Plan {
            self.plan("full", s, vec![e(0, 2, "FN")]).unwrap()
        }
        fn prep(&mut self, plan: &Plan, doc: &FakeDoc) -> Result<String> {
            self.prep_with(plan, doc, true, true, true)
        }
        fn prep_with(
            &mut self,
            plan: &Plan,
            doc: &FakeDoc,
            confirmed: bool,
            grant_ok: bool,
            validation_ok: bool,
        ) -> Result<String> {
            let state = doc.state();
            self.co.prepare(
                self.now,
                &mut self.ids,
                &plan.id,
                PrepareArgs {
                    state: Some(&state),
                    confirmed,
                    grant_ok,
                    validation_ok,
                },
            )
        }
        /// begin -> editor write -> finish, the whole of the old `apply`.
        fn apply(&mut self, token: &str, doc: &mut FakeDoc, mode: Mode) -> Result<Applied> {
            let edits = self.co.begin_apply(self.now, token, Some(&doc.state()))?;
            let outcome = doc.write(&edits, mode);
            if mode == Mode::Hang {
                self.now += 200; // the bridge's deadline passes; it reports the failure
            }
            self.co
                .finish_apply(self.now, token, outcome, Some(&doc.state()))
        }
    }

    fn code<T>(r: Result<T>) -> &'static str {
        match r {
            Ok(_) => "NO_ERROR",
            Err(e) => e.code,
        }
    }

    #[test]
    fn ut_044_ut_053_it_035_whole_file_and_insert_paths_are_independent() {
        let mut rig = Rig::new();
        let l1 = snap_with("fn a() {}\n", "L1");
        assert_eq!(
            code(rig.plan("full", &l1, vec![e(0, 2, "FN")])),
            "FULL_CONTEXT_REQUIRED"
        );
        let plan = rig.plan("insert", &l1, vec![e(0, 0, "// hi\n")]).unwrap();
        assert_eq!(rig.co.status_of(rig.now, &plan.id), "REVIEW_READY");
        assert_eq!(
            code(rig.plan("rewrite", &l1, vec![e(0, 0, "x")])),
            "PATCH_INVALID"
        );
    }

    #[test]
    fn ut_030_an_l4_disk_snapshot_can_never_become_a_plan() {
        let mut rig = Rig::new();
        let disk = snap_with("on disk\n", "L4");
        assert_eq!(
            code(rig.plan("full", &disk, vec![e(0, 2, "ON")])),
            "FULL_CONTEXT_REQUIRED"
        );
    }

    #[test]
    fn ut_046_it_029_zero_length_insert_at_line_3_col_6_with_emoji_changes_only_that_range() {
        let text = "one\ntwo\nthree \u{1F600} four\nfive\n";
        let s = snap(text);
        let off = textrange::to_offset(text, pos(2, 6)).unwrap();
        assert_eq!(
            off,
            "one\ntwo\nthree ".encode_utf16().count(),
            "column 6 is right before the emoji"
        );
        let sel = Range {
            start: pos(2, 6),
            end: pos(2, 6),
        };
        let mut rig = Rig::new();
        let plan = rig
            .co
            .create_plan(
                rig.now,
                &mut rig.ids,
                PlanRequest {
                    kind: "insert",
                    snapshot: &s,
                    edits: vec![e(off, off, "\u{1F600}\n")],
                    targets: vec![],
                    selection: Some(sel),
                },
            )
            .unwrap();
        let mut doc = FakeDoc::new(text);
        doc.selection = plan.selection;
        let tok = rig.prep(&plan, &doc).unwrap();
        let out = rig.apply(&tok, &mut doc, Mode::Ok).unwrap();
        assert_eq!(out.new_hash, plan.proposed_hash);
        let units = textrange::units(text);
        let mut expected: Vec<u16> = units[..off].to_vec();
        expected.extend("\u{1F600}\n".encode_utf16());
        expected.extend_from_slice(&units[off..]);
        assert_eq!(doc.text, String::from_utf16(&expected).unwrap());
        assert_eq!(
            textrange::from_offset(&doc.text, off + 3).unwrap(),
            pos(3, 0),
            "the cursor lands after the inserted text"
        );
    }

    #[test]
    fn ut_047_it_030_nothing_is_written_without_explicit_confirmation() {
        let mut rig = Rig::new();
        let doc = FakeDoc::new(TEXT);
        let s = snap(TEXT);
        let plan = rig.full(&s);
        assert_eq!(
            code(rig.prep_with(&plan, &doc, false, true, true)),
            "USER_CONFIRMATION_REQUIRED"
        );
        rig.co.discard(&plan.id); // panel closed
        assert_eq!(code(rig.prep(&plan, &doc)), "TOKEN_INVALID");
        let p2 = rig.full(&s);
        rig.now += rig.co.timing.plan_ttl_ms; // timeout
        assert_eq!(code(rig.prep(&p2, &doc)), "STALE_RESULT");
        assert_eq!(rig.co.stats.writes, 0);
        assert_eq!(doc.apply_calls, 0);
        assert_eq!(doc.text, TEXT);
    }

    #[test]
    fn ut_048_overlapping_edits_are_rejected_disjoint_edits_build_a_reviewable_result() {
        let s = snap("0123456789");
        let mut rig = Rig::new();
        let plan = |rig: &mut Rig, edits| rig.plan("full", &s, edits);
        assert_eq!(
            code(plan(&mut rig, vec![e(0, 5, "a"), e(3, 7, "b")])),
            "PATCH_INVALID"
        );
        assert_eq!(
            code(plan(&mut rig, vec![e(2, 2, "a"), e(2, 2, "b")])),
            "PATCH_INVALID",
            "two inserts at one offset are ambiguous"
        );
        assert_eq!(code(plan(&mut rig, vec![])), "PATCH_INVALID");
        assert_eq!(
            code(plan(&mut rig, vec![e(0, 99, "x")])),
            "TARGET_RANGE_INVALID"
        );
        let ok = plan(&mut rig, vec![e(0, 2, "AB"), e(5, 5, "-")]).unwrap();
        assert_eq!(ok.proposed_hash, sha256_hex("AB234-56789"));
        assert_eq!(rig.co.stats.plans, 1);
    }

    #[test]
    fn ut_049_it_031_st_027_a_changed_version_or_hash_yields_stale_document_and_no_token() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        doc.user_edit("fn a() {}\nfn b() {}\n// user typed during generation\n");
        assert_eq!(code(rig.prep(&plan, &doc)), "STALE_DOCUMENT");
        assert_eq!(rig.co.token_count(), 0, "no edit token is issued");
        assert_eq!(rig.co.status_of(rig.now, &plan.id), "STALE");
        assert!(
            doc.text.ends_with("generation\n"),
            "the user's newer text is intact"
        );
        // same version number but different content must also be refused (hash check)
        let mut d2 = FakeDoc::new(TEXT);
        let p2 = rig.full(&snap(TEXT));
        d2.text = "fn a() {}\nfn X() {}\n".to_string();
        assert_eq!(code(rig.prep(&p2, &d2)), "STALE_DOCUMENT");
        // and a different URI
        let mut d3 = FakeDoc::new(TEXT);
        let p3 = rig.full(&snap(TEXT));
        d3.uri = "file:///w/other.rs".to_string();
        assert_eq!(code(rig.prep(&p3, &d3)), "STALE_DOCUMENT");
    }

    #[test]
    fn ut_050_an_apply_token_works_once_replay_writes_nothing_more() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        let first = rig.apply(&token, &mut doc, Mode::Ok).unwrap();
        assert_eq!(first.new_hash, plan.proposed_hash);
        assert_eq!(code(rig.apply(&token, &mut doc, Mode::Ok)), "TOKEN_INVALID");
        assert_eq!(doc.apply_calls, 1);
        assert_eq!(
            code(rig.apply(&"0".repeat(32), &mut doc, Mode::Ok)),
            "TOKEN_INVALID"
        );
        let mut i = SnapshotInput::new(URI, doc.version, &doc.text);
        i.selection = None;
        let p2 = rig
            .plan("full", &make_snapshot(i).unwrap(), vec![e(0, 2, "fn")])
            .unwrap();
        let t2 = rig.prep(&p2, &doc).unwrap();
        rig.now += rig.co.timing.token_ttl_ms; // expired token
        assert_eq!(code(rig.apply(&t2, &mut doc, Mode::Ok)), "TOKEN_INVALID");
    }

    #[test]
    fn ut_051_it_033_an_unknown_apply_result_is_final_edit_unknown_never_retried() {
        for mode in [Mode::Throw, Mode::Hang, Mode::WrongText] {
            let mut rig = Rig::new();
            let mut doc = FakeDoc::new(TEXT);
            let plan = rig.full(&snap(TEXT));
            let token = rig.prep(&plan, &doc).unwrap();
            assert_eq!(code(rig.apply(&token, &mut doc, mode)), "EDIT_UNKNOWN");
            assert_eq!(rig.co.status_of(rig.now, &plan.id), "UNKNOWN");
            assert_eq!(
                code(rig.apply(&token, &mut doc, Mode::Ok)),
                "TOKEN_INVALID",
                "no automatic replay"
            );
            assert_eq!(doc.apply_calls, 1);
            // the plan itself is dead too: it cannot be prepared again
            assert_eq!(code(rig.prep(&plan, &doc)), "EDIT_UNKNOWN");
        }
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        assert_eq!(
            code(rig.apply(&token, &mut doc, Mode::Reject)),
            "EDIT_REJECTED",
            "a clean refusal leaves the document untouched"
        );
        assert_eq!(doc.text, TEXT);
        assert_eq!(rig.co.status_of(rig.now, &plan.id), "STALE");
    }

    #[test]
    fn ut_051_a_rejected_write_that_changed_the_document_is_edit_unknown() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        let edits = rig
            .co
            .begin_apply(rig.now, &token, Some(&doc.state()))
            .unwrap();
        assert_eq!(edits, vec![e(0, 2, "FN")]);
        doc.user_edit("something else entirely\n"); // refused by the editor, but the user typed meanwhile
        let r = rig
            .co
            .finish_apply(rig.now, &token, ApplyOutcome::Rejected, Some(&doc.state()));
        assert_eq!(code(r), "EDIT_UNKNOWN");
    }

    #[test]
    fn a_missing_finish_apply_is_edit_unknown_on_the_next_call_that_touches_the_plan() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let s = snap(TEXT);
        let plan = rig.full(&s);
        let token = rig.prep(&plan, &doc).unwrap();
        rig.co
            .begin_apply(rig.now, &token, Some(&doc.state()))
            .unwrap();
        assert_eq!(
            rig.co.status_of(rig.now, &plan.id),
            "PREPARED",
            "still within the deadline"
        );
        rig.now += 81; // the deadline (80 ms) passes and the bridge never reports
        // a second prepare of the same plan notices: the plan is dead
        assert_eq!(code(rig.prep(&plan, &doc)), "EDIT_UNKNOWN");
        assert_eq!(rig.co.status_of(rig.now, &plan.id), "UNKNOWN");
        // the late finish does not turn into APPLIED even though the document looks right
        let edits = textrange::apply_edits(TEXT, &[e(0, 2, "FN")]).unwrap();
        doc.user_edit(&edits);
        let r = rig
            .co
            .finish_apply(rig.now, &token, ApplyOutcome::Accepted, Some(&doc.state()));
        assert_eq!(code(r), "EDIT_UNKNOWN");
        assert_eq!(
            code(
                rig.co
                    .finish_apply(rig.now, &token, ApplyOutcome::Accepted, Some(&doc.state()))
            ),
            "TOKEN_INVALID",
            "a finish is answered once"
        );
    }

    #[test]
    fn a_finish_after_the_deadline_is_edit_unknown_never_applied() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        let edits = rig
            .co
            .begin_apply(rig.now, &token, Some(&doc.state()))
            .unwrap();
        let outcome = doc.write(&edits, Mode::Ok);
        rig.now += 81;
        let r = rig
            .co
            .finish_apply(rig.now, &token, outcome, Some(&doc.state()));
        assert_eq!(code(r), "EDIT_UNKNOWN");
        assert_eq!(rig.co.status_of(rig.now, &plan.id), "UNKNOWN");
        // exactly at the deadline is still in time
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        let edits = rig
            .co
            .begin_apply(rig.now, &token, Some(&doc.state()))
            .unwrap();
        let outcome = doc.write(&edits, Mode::Ok);
        rig.now += 80;
        assert!(
            rig.co
                .finish_apply(rig.now, &token, outcome, Some(&doc.state()))
                .is_ok()
        );
    }

    #[test]
    fn a_second_begin_apply_before_the_first_finish_is_refused_without_consuming_its_token() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let s = snap(TEXT);
        let p1 = rig.full(&s);
        let p2 = rig.plan("full", &s, vec![e(10, 12, "FN")]).unwrap();
        let t1 = rig.prep(&p1, &doc).unwrap();
        let t2 = rig.prep(&p2, &doc).unwrap();
        let edits = rig
            .co
            .begin_apply(rig.now, &t1, Some(&doc.state()))
            .unwrap();
        let r = rig.co.begin_apply(rig.now, &t2, Some(&doc.state()));
        assert_eq!(code(r), "EDIT_REJECTED", "the write of t1 is still pending");
        let outcome = doc.write(&edits, Mode::Ok);
        rig.co
            .finish_apply(rig.now, &t1, outcome, Some(&doc.state()))
            .unwrap();
        // t2 was not consumed, but its baseline is gone now: the document changed
        assert_eq!(
            code(rig.co.begin_apply(rig.now, &t2, Some(&doc.state()))),
            "STALE_DOCUMENT"
        );
        // the same token cannot begin twice
        assert_eq!(
            code(rig.co.begin_apply(rig.now, &t1, Some(&doc.state()))),
            "TOKEN_INVALID"
        );
    }

    #[test]
    fn a_finish_without_a_begin_is_refused() {
        let mut rig = Rig::new();
        let doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        assert_eq!(
            code(
                rig.co
                    .finish_apply(rig.now, &token, ApplyOutcome::Accepted, Some(&doc.state()))
            ),
            "TOKEN_INVALID"
        );
        assert_eq!(doc.text, TEXT);
    }

    #[test]
    fn it_032_a_verified_apply_can_be_undone_back_to_the_original_dirty_buffer() {
        let mut rig = Rig::new();
        let original = "fn a() {}\n// unsaved\n";
        let mut doc = FakeDoc::new(original);
        let plan = rig.full(&snap(original));
        let token = rig.prep(&plan, &doc).unwrap();
        rig.apply(&token, &mut doc, Mode::Ok).unwrap();
        assert_eq!(sha256_hex(&doc.text), plan.proposed_hash);
        doc.undo();
        assert_eq!(doc.text, original);
    }

    #[test]
    fn ut_054_it_036_st_030_only_the_current_file_can_be_changed() {
        let mut rig = Rig::new();
        let s = snap("fn a() {}\n");
        let request = |rig: &mut Rig, targets: Vec<String>| {
            rig.co.create_plan(
                rig.now,
                &mut rig.ids,
                PlanRequest {
                    kind: "full",
                    snapshot: &s,
                    edits: vec![e(0, 2, "FN")],
                    targets,
                    selection: None,
                },
            )
        };
        assert_eq!(
            code(request(
                &mut rig,
                vec![URI.into(), "file:///w/other.rs".into()]
            )),
            "CROSS_FILE_DENIED"
        );
        assert!(request(&mut rig, vec![URI.into()]).is_ok());
    }

    #[test]
    fn ut_075_it_015_a_revoked_grant_or_an_invalidated_plan_can_neither_prepare_nor_apply() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let s = snap(TEXT);
        let plan = rig.full(&s);
        assert_eq!(
            code(rig.prep_with(&plan, &doc, true, false, true)),
            "GRANT_DENIED"
        );
        let token = rig.prep(&plan, &doc).unwrap();
        assert_eq!(rig.co.invalidate(rig.now, URI), 1); // document changed after the token was issued
        assert_eq!(
            code(rig.apply(&token, &mut doc, Mode::Ok)),
            "STALE_DOCUMENT"
        );
        assert_eq!(doc.apply_calls, 0);
        assert_eq!(doc.text, TEXT);
        let p2 = rig.full(&s);
        doc.closed = true;
        assert_eq!(code(rig.prep(&p2, &doc)), "DOCUMENT_CLOSED");
        // a missing state counts as closed
        let r = rig.co.prepare(
            rig.now,
            &mut rig.ids,
            &p2.id,
            PrepareArgs {
                state: None,
                confirmed: true,
                grant_ok: true,
                validation_ok: true,
            },
        );
        assert_eq!(code(r), "DOCUMENT_CLOSED");
    }

    #[test]
    fn ut_052_a_failed_syntax_check_disables_the_default_apply() {
        let mut rig = Rig::new();
        let doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        assert_eq!(
            code(rig.prep_with(&plan, &doc, true, true, false)),
            "VALIDATION_FAILED"
        );
        assert_eq!(rig.co.token_count(), 0);
    }

    #[test]
    fn it_029_insert_is_refused_when_the_cursor_moved_after_the_proposal() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new("abc\ndef\n");
        let s = snap("abc\ndef\n");
        let sel0 = Range {
            start: pos(0, 1),
            end: pos(0, 1),
        };
        let plan = rig
            .co
            .create_plan(
                rig.now,
                &mut rig.ids,
                PlanRequest {
                    kind: "insert",
                    snapshot: &s,
                    edits: vec![e(1, 1, "X")],
                    targets: vec![],
                    selection: Some(sel0),
                },
            )
            .unwrap();
        doc.selection = Some(Range {
            start: pos(1, 0),
            end: pos(1, 0),
        });
        assert_eq!(code(rig.prep(&plan, &doc)), "STALE_SELECTION");
        // the same selection passes; a missing selection never matches
        let p2 = rig
            .co
            .create_plan(
                rig.now,
                &mut rig.ids,
                PlanRequest {
                    kind: "insert",
                    snapshot: &s,
                    edits: vec![e(1, 1, "X")],
                    targets: vec![],
                    selection: Some(sel0),
                },
            )
            .unwrap();
        doc.selection = Some(sel0);
        assert!(rig.prep(&p2, &doc).is_ok());
        doc.selection = None;
        let p3 = rig.plan("insert", &s, vec![e(1, 1, "X")]).unwrap();
        // p3 takes the snapshot's own selection (0,0): the document reports none -> stale
        assert_eq!(code(rig.prep(&p3, &doc)), "STALE_SELECTION");
    }

    #[test]
    fn a_read_only_document_is_rejected_after_the_baseline_checks() {
        let mut rig = Rig::new();
        let doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let mut state = doc.state();
        state.editable = false;
        let r = rig.co.prepare(
            rig.now,
            &mut rig.ids,
            &plan.id,
            PrepareArgs {
                state: Some(&state),
                confirmed: true,
                grant_ok: true,
                validation_ok: true,
            },
        );
        assert_eq!(code(r), "EDIT_REJECTED");
    }

    #[test]
    fn the_checks_run_in_the_order_of_the_js_coordinator() {
        // unknown plan < confirmation < grant < stale status < expiry < closed < baseline < validation
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        doc.closed = true;
        let plan = rig.full(&snap(TEXT));
        rig.co.invalidate(rig.now, URI); // status STALE
        rig.now += rig.co.timing.plan_ttl_ms + 1; // and expired
        assert_eq!(
            code(rig.prep_with(&plan, &doc, false, false, false)),
            "USER_CONFIRMATION_REQUIRED"
        );
        assert_eq!(
            code(rig.prep_with(&plan, &doc, true, false, false)),
            "GRANT_DENIED"
        );
        assert_eq!(
            code(rig.prep_with(&plan, &doc, true, true, false)),
            "STALE_DOCUMENT"
        );
        let fresh = {
            rig.now = T0;
            rig.full(&snap(TEXT))
        };
        rig.now += rig.co.timing.plan_ttl_ms + 1;
        assert_eq!(
            code(rig.prep_with(&fresh, &doc, true, true, false)),
            "STALE_RESULT"
        );
        rig.now = T0;
        let fresh2 = rig.full(&snap(TEXT));
        assert_eq!(
            code(rig.prep_with(&fresh2, &doc, true, true, false)),
            "DOCUMENT_CLOSED"
        );
        doc.closed = false;
        doc.user_edit("changed\n");
        assert_eq!(
            code(rig.prep_with(&fresh2, &doc, true, true, false)),
            "STALE_DOCUMENT"
        );
        let fresh3 = rig.full(&snap(TEXT));
        let ok_doc = FakeDoc::new(TEXT);
        assert_eq!(
            code(rig.prep_with(&fresh3, &ok_doc, true, true, false)),
            "VALIDATION_FAILED"
        );
    }

    #[test]
    fn nfr_502_purge_drops_expired_plans_used_tokens_and_applied_source_fragments() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        rig.apply(&token, &mut doc, Mode::Ok).unwrap();
        rig.co.purge(rig.now);
        assert_eq!(rig.co.plan_count(), 0);
        assert_eq!(rig.co.token_count(), 0);
        // an expired, never prepared plan goes too
        let s = snap(TEXT);
        rig.full(&s);
        assert_eq!(rig.co.plan_count(), 1);
        rig.now += rig.co.timing.plan_ttl_ms;
        rig.co.purge(rig.now);
        assert_eq!(rig.co.plan_count(), 0);
    }

    #[test]
    fn purge_keeps_the_record_of_an_apply_that_is_still_waiting_for_its_result() {
        let mut rig = Rig::new();
        let mut doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        let edits = rig
            .co
            .begin_apply(rig.now, &token, Some(&doc.state()))
            .unwrap();
        rig.co.purge(rig.now); // the panel was closed while the editor is still writing
        let outcome = doc.write(&edits, Mode::Ok);
        let r = rig
            .co
            .finish_apply(rig.now, &token, outcome, Some(&doc.state()));
        assert!(r.is_ok(), "{:?}", r.err());
        // after the grace period the record is dropped
        let mut rig = Rig::new();
        let doc = FakeDoc::new(TEXT);
        let plan = rig.full(&snap(TEXT));
        let token = rig.prep(&plan, &doc).unwrap();
        rig.co
            .begin_apply(rig.now, &token, Some(&doc.state()))
            .unwrap();
        rig.now += 80 + PENDING_GRACE_MS + 1;
        rig.co.purge(rig.now);
        assert_eq!(
            code(
                rig.co
                    .finish_apply(rig.now, &token, ApplyOutcome::Accepted, Some(&doc.state()))
            ),
            "TOKEN_INVALID"
        );
    }

    #[test]
    fn plan_ids_and_tokens_are_unique() {
        let mut rig = Rig::new();
        let doc = FakeDoc::new(TEXT);
        let s = snap(TEXT);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let p = rig.full(&s);
            assert!(seen.insert(p.id.clone()));
            let t = rig.prep(&p, &doc).unwrap();
            assert_eq!(t.len(), 32);
            assert!(seen.insert(t));
        }
    }
}
