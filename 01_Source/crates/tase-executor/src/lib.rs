//! TASE executor: the read-only preview, the apply, the rebase decision, the conflict decision, the pinned
//! workspace, the Git compare-and-swap commit and the undo description (detailed design §5, §6.3 and §8).
//!
//! The executor is the part of TASE that **turns a plan into a change of a repository**. The planner decides what the
//! change is and the registry decides whether it may be made; the executor is the one that computes the diff, that
//! checks the base, that hands the change to the editor and that makes the commit.
//!
//! # What the crate covers of detailed design §8
//!
//! Detailed design §8 ("事务适配和端侧同步", DD-ADAPTER) is short, so the table below is the whole of it:
//!
//! | step of §8 | what this crate does |
//! |---|---|
//! | §8.1 the local chain `CLI/HTTP -> ide-shell-protocol -> tase-executor -> DocumentEditAdapter -> ide-kernel::Document` | the middle of the chain: [`apply::DocumentEditAdapter`] is the door to the host, and the executor is what stands behind it |
//! | §8.1 "一个 Plan 在本地编辑器显示为单一逻辑 undo 事务" | [`undo::UndoTransaction`]: the plan, its documents and the inverse of every document travel as **one** transaction, so the host can undo the plan in one gesture |
//! | §8.1 "若批量编辑过程中失败，利用内核暂存/快照回滚并恢复 buffer dirty/cursor 状态，保证半成品不呈现为成功" | [`apply::apply_to_host`] stops at the first document the host refuses and never reports a partial apply as a success; [`apply::rollback`] writes the inverse edits of the documents that were already written |
//! | §8.2 the cloud chain `HTTP Router -> Queue -> Worker -> Staging Repository -> DocumentEditAdapter -> Git tree/commit -> CAS ref` | the last two steps: [`git_tx::GitRepo::create_commit`] builds the commit object and [`git_tx::GitRepo::compare_and_swap`] moves the ref |
//! | §8.3 the `PositionMapper` for CRLF, BOM, CJK and emoji | **not** here: the mapping belongs to `tase-semantic` (UTF-16 is the basis of every edit of this repository) and to `pime-core::textrange`; this crate only uses their offsets |
//!
//! The rebase and the conflict decision of detailed design §5.3, §5.4 and §6.2 are the executor's too, and they are
//! in [`rebase`] and [`conflict`].
//!
//! # What the crate does NOT cover, and why
//!
//! * **The write of the buffer.** This repository has no `Document` type and no `apply` method: `pime-core` holds no
//!   buffer and no undo stack, and the text is written by the host editor (the mapping document §7-G1). The apply of
//!   this crate therefore produces a **constrained diff plus the inverse of that diff** and hands both to the host;
//!   it never writes a file and never keeps a text buffer of its own.
//! * **The single undo stack of the editor.** A "single logical undo transaction" is a *description* here, not a stack:
//!   [`undo::UndoTransaction`] says what the host has to apply to go back. The host owns the stack.
//! * **The queue, the lease and the fencing.** Detailed design §5.2 and §6.1 belong to `tase-worker` and to
//!   `tase-local-store`: the executor is called by the worker, it does not schedule anything.
//! * **The database, the `CommitCoordinator` and the reconciler.** Detailed design §5.5 puts the single write door of
//!   the branch ref in the database; this crate only offers the compare-and-swap it calls.
//! * **The checks in the sandbox** (format, compile, test). They need the allowlist and the sandbox of detailed
//!   design §7.2, and they belong to `tase-worker`.
//!
//! # The rules of the crate
//!
//! * **One diff.** The preview shows the diff of [`preview::diff_of`] and the apply hands the same diff to the host:
//!   the two can never disagree, because there is only one function that computes it.
//! * **UTF-16 everywhere.** Every offset of this crate is a UTF-16 code unit, like every edit of this repository. An
//!   emoji is two units and one char; the two are never mixed.
//! * **The inverse is part of the change.** Every change carries the edits that undo it, so a failed apply can be
//!   rolled back and a successful one can be undone in one gesture.
//! * **No third-party dependency, no network.** The Git capability is the `git` command line, driven with a fixed
//!   argument vector (never a shell string). The tests never touch the network.
//!
//! The crate has no third-party dependency: the JSON, the SHA-256, the UTF-16 ranges and the line diff come from
//! `pime-core`, the DTOs and the error codes from `tase-contract`, the digest of the plan from `tase-planner`, the
//! catalog from `tase-registry` and the symbol index from `tase-semantic`.

#![forbid(unsafe_code)]

pub mod apply;
pub mod conflict;
pub mod git_tx;
pub mod preview;
pub mod rebase;
pub mod undo;
pub mod workspace;

pub use apply::{
    ApplyOutcome, ApplyPlan, DocumentEdit, DocumentEditAdapter, HostEditOutcome, HostEditRequest,
};
pub use conflict::{Conflict, ConflictKind};
pub use git_tx::{FileWrite, GitCli, GitRepo, MemoryGit};
pub use preview::{ChangeKind, FileChange, Preview, Risk};
pub use rebase::{ChangeSetSource, ChangedFile, RebaseOutcome};
pub use undo::{UndoDocument, UndoTransaction};
pub use workspace::{DocumentSnapshot, PinnedFile, WorkspaceSnapshot};

use tase_contract::{ErrorCode, TaseError};

/// The version of the executor.
pub const EXECUTOR_VERSION: &str = "1.1.0";

/// The result of a call of the executor.
///
/// The failure of the executor is a business failure of the contract layer: the twelve codes of detailed design §4.3
/// are the codes the transports know, and the executor picks the one that describes what happened (`STALE_BASE` for a
/// base that moved, `SYMBOL_CONFLICT` for a symbol another agent changed, ...).
pub type Result<T> = std::result::Result<T, TaseError>;

/// Turns the failure of the core into the failure of the contract.
///
/// The core and the contract have their own error models; the executor speaks the contract one, because the
/// transports of the contract are the ones that reach the user.
pub(crate) fn from_core(error: pime_core::error::PimeError) -> TaseError {
    TaseError::new(
        ErrorCode::ValidationFailed,
        format!("{}: {}", error.code, error.message),
    )
}
