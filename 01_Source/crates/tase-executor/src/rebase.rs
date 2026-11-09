//! The rebase decision: may the plan still run on the new head (detailed design §5.3 and §5.4).
//!
//! Detailed design §5.3 lists the conditions of an automatic rebase. They are read here one by one:
//!
//! 1. the base of the plan has to be a known ancestor of the new head (otherwise the plan is refused or a human has
//!    to look at it);
//! 2. the target symbols are indexed **again** at the new head: they have to be there, and they have to be unique;
//! 3. the real write set of the plan and the write set of the changes since the base are compared: an overlap is a
//!    conflict;
//! 4. the declarative plan is run again on the new head - never the old byte offsets on the new file;
//! 5. the gates (compile, test, policy) run again on the new candidate.
//!
//! This module decides **1, 2 and 3**. Step 4 is the worker (it runs the plan again), and step 5 is the worker too.
//! What the module never does is overwrite: the outcomes are "the plan may run again on the new head" or "the plan
//! may not run, and here is why".
//!
//! # The rule of the overlap
//!
//! Detailed design §5.4 says that two agents that touch different functions of the same file may still conflict, and
//! that a shared `use` section is a conflict even when the names differ. The semantic layer of this repository is a
//! line-level scan and not an AST parser, so the module cannot prove that two changes of the same file are
//! independent: **a file that both the plan and the head write is a conflict**, and the reason is named as precisely
//! as the facts allow (the symbol is gone, the symbol is not unique any more, the signature moved, the imports moved).

use tase_contract::api::{Operation, OperationKind, PlanRequest};
use tase_semantic::SemanticIndex;
use tase_semantic::symbol::{Symbol, SymbolKind};

use crate::conflict::{Conflict, ConflictKind};
use crate::Result;

/// A file that changed between two commits.
///
/// The two digests are the object ids of the file in the source of the facts (the blob ids of Git, the digests of
/// the memory of a test). The executor compares the paths of the changed files; the digests are for the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// The object id of the file before the change.
    pub old_blob: String,
    /// The object id of the file after the change.
    pub new_blob: String,
}

/// Where the facts about the repository come from.
///
/// The trait is the small read-only door the rebase needs: the history and the files that changed. The Git of
/// [`crate::git_tx`] implements it, and a test implements it with the memory. The names of its methods are not the
/// names of [`crate::git_tx::GitRepo`]: the two traits answer different questions and both may be in scope at the
/// same time.
pub trait ChangeSetSource {
    /// Whether `ancestor` is in the history of `descendant`.
    fn is_ancestor_of(&self, ancestor: &str, descendant: &str) -> Result<bool>;

    /// The files that changed between two commits.
    fn changed_files_between(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>>;
}

/// The decision of the rebase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// The head is the base of the plan: there is nothing to rebase.
    UpToDate,
    /// The plan may run again on the new head: the write sets do not overlap.
    Rebaseable {
        /// The commit the plan has to run on.
        head: String,
    },
    /// The plan may not run: the conflict is the reason.
    Conflict {
        /// The conflict that was found.
        conflict: Conflict,
    },
    /// The plan may not run and a human has to look at it.
    ManualReview {
        /// The conflict that was found.
        conflict: Conflict,
    },
}

impl RebaseOutcome {
    /// Whether the plan may run on the new head.
    pub fn is_rebaseable(&self) -> bool {
        matches!(self, RebaseOutcome::Rebaseable { .. })
    }

    /// The conflict of the decision, when there is one.
    pub fn conflict(&self) -> Option<&Conflict> {
        match self {
            RebaseOutcome::Conflict { conflict } | RebaseOutcome::ManualReview { conflict } => Some(conflict),
            _ => None,
        }
    }
}

/// The hash of the signature of a symbol: `sha256:` and the SHA-256 of the signature text.
///
/// The signature is the header of the declaration (`pub fn run(x: u32) -> bool`). A symbol without a readable header
/// has the hash of the empty text, so the hash of a symbol never depends on how the reader found it.
pub fn signature_hash(signature: Option<&str>) -> String {
    format!(
        "sha256:{}",
        pime_core::sha256::sha256_hex(signature.unwrap_or(""))
    )
}

/// Decides whether the plan may run on the new head.
///
/// The `index` is the semantic index of the **new** head: the symbols of the new head are the ones the plan is
/// checked against, never the ones of the old base.
pub fn decide(
    request: &PlanRequest,
    head: &str,
    changes: &dyn ChangeSetSource,
    index: &SemanticIndex,
) -> RebaseOutcome {
    let base = request.base_commit.as_str();
    if base == head {
        return RebaseOutcome::UpToDate;
    }

    match changes.is_ancestor_of(base, head) {
        Ok(true) => {}
        Ok(false) => {
            return RebaseOutcome::ManualReview {
                conflict: Conflict::new(
                    ConflictKind::BaseNotAncestor,
                    "",
                    None,
                    format!("the base {base} is not in the history of {head}"),
                ),
            };
        }
        Err(error) => {
            return RebaseOutcome::ManualReview {
                conflict: Conflict::new(
                    ConflictKind::BaseNotAncestor,
                    "",
                    None,
                    format!("the history of {base} could not be read: {error}"),
                ),
            };
        }
    }

    let changed = match changes.changed_files_between(base, head) {
        Ok(files) => files,
        Err(error) => {
            return RebaseOutcome::ManualReview {
                conflict: Conflict::new(
                    ConflictKind::PreconditionFailed,
                    "",
                    None,
                    format!("the changes between {base} and {head} could not be read: {error}"),
                ),
            };
        }
    };

    for operation in &request.operations {
        let path = operation.target.path.as_str();
        if !changed.iter().any(|file| file.path == path) {
            // the plan writes a file the head did not touch: the write sets do not overlap
            continue;
        }
        if let Some(conflict) = conflict_of(operation, index) {
            return RebaseOutcome::Conflict { conflict };
        }
    }

    RebaseOutcome::Rebaseable {
        head: head.to_string(),
    }
}

/// The conflict of one operation whose file changed at the new head.
///
/// The reason is the most precise one the facts allow: the imports moved, the symbol is gone, the symbol is not
/// unique any more, the signature moved, or the file itself moved.
fn conflict_of(operation: &Operation, index: &SemanticIndex) -> Option<Conflict> {
    let path = operation.target.path.as_str();
    let symbol = operation.target.symbol.as_deref();

    if operation.kind == OperationKind::UpdateImports {
        return Some(Conflict::new(
            ConflictKind::ImportGroupOverlap,
            path,
            symbol,
            format!("the imports of {path} changed at the new head"),
        ));
    }

    let Some(symbol) = symbol else {
        return Some(Conflict::new(
            ConflictKind::FileOverlap,
            path,
            None,
            format!("the file {path} changed at the new head and the plan writes the whole file"),
        ));
    };

    let candidates: Vec<&Symbol> = index
        .file(path)
        .map(|file| file.symbols_named(symbol).collect())
        .unwrap_or_default();
    let candidates: Vec<&Symbol> = match operation.target.symbol_kind.as_deref() {
        Some(kind) => candidates
            .into_iter()
            .filter(|s| s.identity.kind == SymbolKind::parse(kind))
            .collect(),
        None => candidates,
    };

    match candidates.len() {
        0 => Some(Conflict::new(
            ConflictKind::SymbolMissing,
            path,
            Some(symbol),
            format!("the symbol {symbol} is not in {path} at the new head"),
        )),
        1 => {
            let found = candidates[0];
            if let Some(preconditions) = &operation.preconditions
                && let Some(expected) = &preconditions.symbol_signature_hash
                && signature_hash(found.signature.as_deref()) != *expected
            {
                return Some(Conflict::new(
                    ConflictKind::SignatureChanged,
                    path,
                    Some(symbol),
                    format!("the signature of {symbol} changed at the new head"),
                ));
            }
            Some(Conflict::new(
                ConflictKind::SymbolOverlap,
                path,
                Some(symbol),
                format!("the file {path} changed at the new head and the plan writes {symbol}"),
            ))
        }
        _ => Some(Conflict::new(
            ConflictKind::SymbolAmbiguous,
            path,
            Some(symbol),
            format!("the symbol {symbol} is not unique in {path} at the new head"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::api::Target;
    use tase_contract::OperationId;
    use tase_semantic::adapter::SourceFile;

    const BASE_TEXT: &str = "fn alpha() {\n    println!(\"alpha\");\n}\n\nfn beta() {\n    println!(\"beta\");\n}\n";
    const OTHER_TEXT: &str = "fn gamma() {}\n";

    /// The files of a commit of the memory: the path and the content.
    type MemoryFiles = Vec<(String, String)>;

    /// The commits of the memory: the id, the parent and the files.
    type MemoryCommits = Vec<(String, String, MemoryFiles)>;

    /// A repository in the memory: the commits, their parents and the content of their files.
    struct MemoryHistory {
        commits: MemoryCommits,
    }

    impl MemoryHistory {
        fn new() -> MemoryHistory {
            MemoryHistory {
                commits: Vec::new(),
            }
        }

        fn commit(&mut self, id: &str, parent: &str, files: &[(&str, &str)]) {
            self.commits.push((
                id.to_string(),
                parent.to_string(),
                files
                    .iter()
                    .map(|(path, content)| (path.to_string(), content.to_string()))
                    .collect(),
            ));
        }

        fn files_of(&self, id: &str) -> MemoryFiles {
            self.commits
                .iter()
                .find(|(commit, _, _)| commit == id)
                .map(|(_, _, files)| files.clone())
                .unwrap_or_default()
        }
    }

    impl ChangeSetSource for MemoryHistory {
        fn is_ancestor_of(&self, ancestor: &str, descendant: &str) -> Result<bool> {
            let mut current = descendant.to_string();
            loop {
                if current == ancestor {
                    return Ok(true);
                }
                let Some((_, parent, _)) = self.commits.iter().find(|(commit, _, _)| commit == &current) else {
                    return Ok(false);
                };
                if parent.is_empty() {
                    return Ok(false);
                }
                current = parent.clone();
            }
        }

        fn changed_files_between(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
            let before = self.files_of(from);
            let after = self.files_of(to);
            let mut changed = Vec::new();
            for (path, content) in &after {
                let old = before
                    .iter()
                    .find(|(other, _)| other == path)
                    .map(|(_, content)| content.clone())
                    .unwrap_or_default();
                if &old != content {
                    changed.push(ChangedFile {
                        path: path.clone(),
                        old_blob: old,
                        new_blob: content.clone(),
                    });
                }
            }
            for (path, content) in &before {
                if !after.iter().any(|(other, _)| other == path) {
                    changed.push(ChangedFile {
                        path: path.clone(),
                        old_blob: content.clone(),
                        new_blob: String::new(),
                    });
                }
            }
            Ok(changed)
        }
    }

    fn operation(path: &str, symbol: Option<&str>, kind: OperationKind) -> Operation {
        Operation {
            op_id: OperationId::parse("op_001").expect("the id is legal"),
            kind,
            target: Target {
                path: path.to_string(),
                symbol: symbol.map(str::to_string),
                symbol_kind: symbol.map(|_| "function".to_string()),
            },
            template: None,
            params: Vec::new(),
            preconditions: None,
        }
    }

    fn request(base: &str, operations: Vec<Operation>) -> PlanRequest {
        PlanRequest {
            contract_version: tase_contract::CONTRACT_VERSION.to_string(),
            repository_id: tase_contract::RepositoryId::parse("repo_ide1").expect("the id is legal"),
            target_ref: "refs/heads/main".to_string(),
            base_commit: base.to_string(),
            strategy: "semantic".to_string(),
            operations,
            validation_profile: "rust".to_string(),
            approval_policy: "default".to_string(),
        }
    }

    fn index_of(text: &str) -> SemanticIndex {
        let mut index = SemanticIndex::with_default_adapters();
        index.index_repository(&[SourceFile::new("src/main.rs", text)]);
        index
    }

    fn history() -> MemoryHistory {
        let mut history = MemoryHistory::new();
        history.commit("commit-a", "", &[("src/main.rs", "fn alpha() {}\n")]);
        history.commit(
            "commit-b",
            "commit-a",
            &[("src/main.rs", "fn alpha() { /* changed */ }\n")],
        );
        history.commit(
            "commit-c",
            "commit-b",
            &[("src/main.rs", "fn alpha() { /* changed twice */ }\n")],
        );
        history
    }

    #[test]
    fn a_plan_whose_head_is_its_base_is_up_to_date() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let plan = request(
            "commit-a",
            vec![operation("src/main.rs", Some("alpha"), OperationKind::ReplaceSymbolBody)],
        );
        let outcome = decide(&plan, "commit-a", &history, &index);
        assert_eq!(outcome, RebaseOutcome::UpToDate);
    }

    #[test]
    fn a_plan_that_writes_another_file_may_run_on_the_new_head() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let plan = request(
            "commit-a",
            vec![operation("src/other.rs", None, OperationKind::Format)],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        assert_eq!(
            outcome,
            RebaseOutcome::Rebaseable {
                head: "commit-b".to_string()
            }
        );
        assert!(outcome.is_rebaseable());
    }

    #[test]
    fn a_plan_that_writes_a_file_the_head_changed_is_a_conflict_and_not_an_overwrite() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let plan = request(
            "commit-a",
            vec![operation("src/main.rs", None, OperationKind::Format)],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        let conflict = outcome.conflict().expect("there is a conflict");
        assert_eq!(conflict.kind, ConflictKind::FileOverlap);
        assert_eq!(conflict.path, "src/main.rs");
        assert!(!outcome.is_rebaseable());
    }

    #[test]
    fn a_plan_that_writes_a_symbol_of_a_file_the_head_changed_is_a_conflict() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let plan = request(
            "commit-a",
            vec![operation(
                "src/main.rs",
                Some("alpha"),
                OperationKind::ReplaceSymbolBody,
            )],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        let conflict = outcome.conflict().expect("there is a conflict");
        assert_eq!(conflict.kind, ConflictKind::SymbolOverlap);
        assert_eq!(conflict.symbol.as_deref(), Some("alpha"));
    }

    #[test]
    fn a_symbol_that_is_gone_at_the_new_head_is_named_as_missing() {
        let history = history();
        // the new head holds another file: alpha is not there any more
        let index = index_of(OTHER_TEXT);
        let plan = request(
            "commit-a",
            vec![operation(
                "src/main.rs",
                Some("alpha"),
                OperationKind::ReplaceSymbolBody,
            )],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        let conflict = outcome.conflict().expect("there is a conflict");
        assert_eq!(conflict.kind, ConflictKind::SymbolMissing);
    }

    #[test]
    fn a_symbol_that_is_not_unique_any_more_is_named_as_ambiguous() {
        let history = history();
        let text = "fn alpha() {}\nmod inner {\n    fn alpha() {}\n}\n";
        let index = index_of(text);
        let plan = request(
            "commit-a",
            vec![operation(
                "src/main.rs",
                Some("alpha"),
                OperationKind::ReplaceSymbolBody,
            )],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        let conflict = outcome.conflict().expect("there is a conflict");
        // the two symbols have the same kind, so the symbol is not unique any more
        assert_eq!(conflict.kind, ConflictKind::SymbolAmbiguous);
        assert_eq!(conflict.symbol.as_deref(), Some("alpha"));
    }

    #[test]
    fn a_signature_that_moved_is_named_as_changed() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let mut operation = operation(
            "src/main.rs",
            Some("alpha"),
            OperationKind::ReplaceSymbolBody,
        );
        operation.preconditions = Some(tase_contract::api::Preconditions {
            file_blob_sha: None,
            symbol_signature_hash: Some("sha256:0000".to_string()),
            expected_symbol_exists: None,
        });
        let plan = request("commit-a", vec![operation]);
        let outcome = decide(&plan, "commit-b", &history, &index);
        let conflict = outcome.conflict().expect("there is a conflict");
        assert_eq!(conflict.kind, ConflictKind::SignatureChanged);
    }

    #[test]
    fn a_base_that_is_not_an_ancestor_goes_to_a_human() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let plan = request(
            "commit-c",
            vec![operation("src/other.rs", None, OperationKind::Format)],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        match outcome {
            RebaseOutcome::ManualReview { conflict } => {
                assert_eq!(conflict.kind, ConflictKind::BaseNotAncestor);
            }
            other => panic!("the outcome is {other:?}"),
        }
    }

    #[test]
    fn the_imports_of_a_file_that_changed_are_a_conflict_of_their_own() {
        let history = history();
        let index = index_of(BASE_TEXT);
        let plan = request(
            "commit-a",
            vec![operation("src/main.rs", None, OperationKind::UpdateImports)],
        );
        let outcome = decide(&plan, "commit-b", &history, &index);
        let conflict = outcome.conflict().expect("there is a conflict");
        assert_eq!(conflict.kind, ConflictKind::ImportGroupOverlap);
    }

    #[test]
    fn the_hash_of_a_signature_is_the_hash_of_its_text() {
        assert_eq!(
            signature_hash(Some("fn alpha()")),
            format!("sha256:{}", pime_core::sha256::sha256_hex("fn alpha()"))
        );
        assert_eq!(
            signature_hash(None),
            format!("sha256:{}", pime_core::sha256::sha256_hex(""))
        );
    }
}
