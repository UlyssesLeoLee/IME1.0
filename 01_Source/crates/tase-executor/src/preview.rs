//! The read-only preview of a plan (detailed design §5.1).
//!
//! The preview of detailed design §5.1 runs the plan of a repository into an isolated worktree, validates the paths,
//! the syntax, the size of the generated files and the policy, computes the real diff and the affected set, runs the
//! checks of the preview and stores an immutable artifact.
//!
//! This crate holds the part of that pipeline that is **pure**: it takes the text of a file before the change and the
//! text of the same file after the change, and it produces the constrained diff, the counters and the artifact. It
//! does not run the plan (the planner and the semantic layer do that), it does not touch the disk and it does not run
//! the checks (the worker does that, in the sandbox).
//!
//! # The one diff of the crate
//!
//! [`diff_of`] is the only function of the crate that computes a diff. The preview shows it and the apply hands it
//! to the host editor, so the two can never disagree: there is no second diff to drift away from the first one. The
//! diff is the line diff of `pime-core::edit::diff`, whose offsets are UTF-16 code units and whose hunks are
//! non-overlapping replacements against the exact baseline text.

use pime_core::edit::diff::{build_hunks, render_hunks, stats, DEFAULT_CONTEXT};
use pime_core::sha256::sha256_hex;
use tase_contract::api::Change;

use crate::conflict::Conflict;
use crate::workspace::validate_repo_relative_path;
use crate::Result;

/// The hunk of a change: the line diff of `pime-core`, re-exported so that the callers of this crate do not have
/// to know where it comes from.
pub use pime_core::edit::diff::Hunk;

/// The number of changed lines from which a change is a medium risk.
pub const RISK_MEDIUM_LINES: u32 = 40;

/// The number of changed lines from which a change is a high risk.
pub const RISK_HIGH_LINES: u32 = 200;

/// What a change does to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The file exists before and after the change.
    Modified,
    /// The file does not exist before the change.
    Created,
    /// The file does not exist after the change.
    Deleted,
}

impl ChangeKind {
    /// The name of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Modified => "modified",
            ChangeKind::Created => "created",
            ChangeKind::Deleted => "deleted",
        }
    }
}

/// The risk class of a change.
///
/// The risk decides whether a human has to approve the change before it is applied (the `requires_approval` field of
/// the preview of the contract layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// A few lines inside the files that already exist.
    Low,
    /// A lot of lines, or a change that touches many files.
    Medium,
    /// A file that appears or disappears, or a very big change.
    High,
}

impl Risk {
    /// The name of the class.
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        }
    }

    /// Whether a change of this class needs an approval before it is applied.
    pub fn requires_approval(self) -> bool {
        matches!(self, Risk::High)
    }
}

/// One file of a preview.
///
/// The change holds the hunks and the rendered diff of the file, never the text of the file: the text is the host's,
/// and a preview artifact is a diff and a digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// What the change does to the file.
    pub kind: ChangeKind,
    /// The SHA-256 of the content of the file before the change.
    pub base_sha256: String,
    /// The SHA-256 of the content of the file after the change.
    pub new_sha256: String,
    /// The hunks of the change, in the order of the file.
    pub hunks: Vec<Hunk>,
    /// The number of lines the change adds.
    pub added_lines: u32,
    /// The number of lines the change removes.
    pub deleted_lines: u32,
    /// The symbols the change touches.
    pub symbols: Vec<String>,
    /// The diff of the file as plain text.
    pub diff_text: String,
}

impl FileChange {
    /// The change of one file: the diff between the text before the change and the text after it.
    pub fn new(
        path: &str,
        kind: ChangeKind,
        base_text: &str,
        new_text: &str,
        symbols: &[String],
    ) -> Result<FileChange> {
        validate_repo_relative_path(path)?;
        let hunks = diff_of(base_text, new_text);
        let counters = stats(&hunks);
        let diff_text = render_file_diff(base_text, &hunks);
        Ok(FileChange {
            path: path.to_string(),
            kind,
            base_sha256: sha256_hex(base_text),
            new_sha256: sha256_hex(new_text),
            added_lines: counters.added as u32,
            deleted_lines: counters.removed as u32,
            hunks,
            symbols: symbols.to_vec(),
            diff_text,
        })
    }

    /// The change as the DTO of the contract layer.
    ///
    /// The contract asks where the diff is stored (`diff_uri`); this crate has no artifact store, so the field is
    /// empty and the layer above fills it with the place it wrote the diff to.
    pub fn to_contract_change(&self) -> Change {
        Change {
            path: self.path.clone(),
            symbols: self.symbols.clone(),
            added_lines: self.added_lines,
            deleted_lines: self.deleted_lines,
            diff_uri: None,
        }
    }
}

/// The constrained diff between the text before a change and the text after it.
///
/// This is the one diff of the crate: the preview shows it and the apply hands it to the host. The hunks are
/// non-overlapping and their offsets are UTF-16 code units of the text before the change.
pub fn diff_of(base: &str, new: &str) -> Vec<Hunk> {
    build_hunks(base, new)
}

/// The risk class of a set of changes.
///
/// A file that appears or disappears is always a high risk; a change of many lines is high when it touches more than
/// [`RISK_HIGH_LINES`] lines and medium when it touches more than [`RISK_MEDIUM_LINES`] lines.
pub fn risk_of(changes: &[FileChange]) -> Risk {
    let mut lines = 0u32;
    for change in changes {
        if matches!(change.kind, ChangeKind::Created | ChangeKind::Deleted) {
            return Risk::High;
        }
        lines += change.added_lines + change.deleted_lines;
    }
    if lines >= RISK_HIGH_LINES {
        Risk::High
    } else if lines >= RISK_MEDIUM_LINES {
        Risk::Medium
    } else {
        Risk::Low
    }
}

/// The diff of one file as plain text.
///
/// The markers are the plain text ones (` `, `-`, `+`), so the meaning never depends on a colour alone.
fn render_file_diff(base: &str, hunks: &[Hunk]) -> String {
    let mut out = String::new();
    for view in render_hunks(base, hunks, DEFAULT_CONTEXT) {
        out.push_str(&view.header);
        out.push('\n');
        for (marker, text) in view.lines {
            out.push(marker);
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

/// The preview of a plan: the changes it makes, the diff, the digest of the tree it would produce and the conflicts
/// that were found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    /// The commit the preview starts from.
    pub base_commit: String,
    /// The digest of the plan the preview was made of.
    pub plan_digest: String,
    /// The changes, in the order of the operations of the plan.
    pub changes: Vec<FileChange>,
    /// The digest of the tree the preview would produce.
    pub new_tree_digest: String,
    /// The diff of every change, as plain text.
    pub diff_text: String,
    /// The conflicts that were found; a preview with a conflict is never a green preview.
    pub conflicts: Vec<Conflict>,
    /// Whether a human has to approve the plan before it is applied.
    pub requires_approval: bool,
    /// The risk class of the change.
    pub risk: Risk,
}

impl Preview {
    /// The preview of a set of changes.
    pub fn build(base_commit: &str, plan_digest: &str, changes: Vec<FileChange>) -> Preview {
        let new_tree_digest = tree_digest(&changes);
        let diff_text = render_diff(&changes);
        let risk = risk_of(&changes);
        Preview {
            base_commit: base_commit.to_string(),
            plan_digest: plan_digest.to_string(),
            requires_approval: risk.requires_approval(),
            risk,
            changes,
            new_tree_digest,
            diff_text,
            conflicts: Vec::new(),
        }
    }

    /// The same preview, with the conflicts that were found.
    ///
    /// A preview that holds a conflict always needs an approval: nobody may apply it without looking at it.
    pub fn with_conflicts(mut self, conflicts: Vec<Conflict>) -> Preview {
        self.requires_approval = self.requires_approval || !conflicts.is_empty();
        self.conflicts = conflicts;
        self
    }

    /// The paths of the files the preview changes.
    pub fn changed_paths(&self) -> Vec<String> {
        self.changes.iter().map(|c| c.path.clone()).collect()
    }

    /// The changes as the DTOs of the contract layer.
    pub fn to_contract_changes(&self) -> Vec<Change> {
        self.changes.iter().map(|c| c.to_contract_change()).collect()
    }
}

/// The digest of the tree the preview would produce.
///
/// The digest is `sha256:` and the SHA-256 of the lines `path` and the digest of the new content of every file, in
/// the order of the changes. It is the identity of the result of the preview, not the SHA of a commit that does not
/// exist yet.
fn tree_digest(changes: &[FileChange]) -> String {
    let mut text = String::new();
    for change in changes {
        text.push_str(&change.path);
        text.push('\u{0}');
        text.push_str(&change.new_sha256);
        text.push('\n');
    }
    format!("sha256:{}", sha256_hex(&text))
}

/// The diff of a set of changes as plain text.
pub fn render_diff(changes: &[FileChange]) -> String {
    let mut out = String::new();
    for change in changes {
        out.push_str("--- a/");
        out.push_str(&change.path);
        out.push('\n');
        out.push_str("+++ b/");
        out.push_str(&change.path);
        out.push('\n');
        out.push_str(&change.diff_text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "fn main() {\n    println!(\"hello\");\n}\n";
    const NEW: &str = "fn main() {\n    println!(\"hello, world\");\n}\n";

    #[test]
    fn the_diff_of_two_texts_is_the_diff_of_the_hunks() {
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[]).expect("the change is built");
        assert_eq!(change.hunks.len(), 1);
        assert_eq!(change.hunks[0].old_text, "    println!(\"hello\");\n");
        assert_eq!(change.hunks[0].new_text, "    println!(\"hello, world\");\n");
        assert_eq!(change.added_lines, 1);
        assert_eq!(change.deleted_lines, 1);
        assert_eq!(change.base_sha256, sha256_hex(BASE));
        assert_eq!(change.new_sha256, sha256_hex(NEW));
    }

    #[test]
    fn a_text_that_does_not_change_has_no_hunk() {
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, BASE, &[]).expect("the change is built");
        assert!(change.hunks.is_empty());
        assert_eq!(change.diff_text, "");
        assert_eq!(change.base_sha256, change.new_sha256);
    }

    #[test]
    fn the_diff_is_plain_text_and_the_markers_are_readable() {
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[]).expect("the change is built");
        assert!(change.diff_text.contains("@@ -"));
        assert!(change.diff_text.contains("-    println!(\"hello\");"));
        assert!(change.diff_text.contains("+    println!(\"hello, world\");"));
        assert!(change.diff_text.contains(" fn main() {"));
    }

    #[test]
    fn the_preview_holds_the_digest_of_the_tree_it_would_produce() {
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[]).expect("the change is built");
        let preview = Preview::build("commit-1", "sha256:plan", vec![change]);
        assert!(preview.new_tree_digest.starts_with("sha256:"));
        assert_eq!(preview.new_tree_digest.len(), "sha256:".len() + 64);
        assert_eq!(preview.changed_paths(), ["src/main.rs"]);
        assert!(preview.diff_text.contains("--- a/src/main.rs"));
        assert!(preview.diff_text.contains("+++ b/src/main.rs"));
        assert!(!preview.requires_approval);
        assert_eq!(preview.risk, Risk::Low);
    }

    #[test]
    fn the_same_changes_give_the_same_tree_digest() {
        let first = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[]).expect("the change is built");
        let second = FileChange::new("src/lib.rs", ChangeKind::Modified, BASE, NEW, &[]).expect("the change is built");
        let one = Preview::build("commit-1", "sha256:plan", vec![first.clone()]);
        let same = Preview::build("commit-1", "sha256:plan", vec![first]);
        let other = Preview::build("commit-1", "sha256:plan", vec![second]);
        assert_eq!(one.new_tree_digest, same.new_tree_digest);
        assert_ne!(one.new_tree_digest, other.new_tree_digest);
    }

    #[test]
    fn a_file_that_appears_or_disappears_is_a_high_risk() {
        let created = FileChange::new("src/new.rs", ChangeKind::Created, "", "fn new() {}\n", &[])
            .expect("the change is built");
        let deleted = FileChange::new("src/old.rs", ChangeKind::Deleted, "fn old() {}\n", "", &[])
            .expect("the change is built");
        let modified = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[])
            .expect("the change is built");
        assert_eq!(risk_of(std::slice::from_ref(&modified)), Risk::Low);
        assert_eq!(risk_of(&[created]), Risk::High);
        assert_eq!(risk_of(&[deleted]), Risk::High);
        assert!(Risk::High.requires_approval());
        assert!(!Risk::Low.requires_approval());
        assert!(!Preview::build("commit-1", "sha256:plan", vec![modified.clone()]).requires_approval);
    }

    #[test]
    fn a_big_change_is_a_high_risk_and_needs_an_approval() {
        let base: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let new: String = (0..100).map(|i| format!("LINE {i}\n")).collect();
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, &base, &new, &[]).expect("the change is built");
        assert_eq!(risk_of(std::slice::from_ref(&change)), Risk::High);
        let preview = Preview::build("commit-1", "sha256:plan", vec![change]);
        assert!(preview.requires_approval);
    }

    #[test]
    fn a_preview_with_a_conflict_always_needs_an_approval() {
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[]).expect("the change is built");
        let conflict = Conflict::new(
            crate::conflict::ConflictKind::FileOverlap,
            "src/main.rs",
            None,
            "the file changed",
        );
        let preview = Preview::build("commit-1", "sha256:plan", vec![change]).with_conflicts(vec![conflict]);
        assert!(preview.requires_approval);
        assert_eq!(preview.conflicts.len(), 1);
    }

    #[test]
    fn the_change_refuses_a_path_that_leaves_the_repository() {
        let error = FileChange::new("../outside.rs", ChangeKind::Modified, BASE, NEW, &[]).unwrap_err();
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }
}
