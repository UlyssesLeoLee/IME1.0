//! The runner of a job: the diff of the executor, the apply to the host editor and the commit (detailed design §5.2
//! and §8.1).
//!
//! # What the runner does, and what it does not
//!
//! The runner is the part of the worker that talks to the executor and to the host:
//!
//! 1. it asks the executor for the **diff** of the job ([`tase_executor::preview`]) - the same diff the preview
//!    showed, because there is only one function that computes it;
//! 2. it asks the executor for the **plan** of the apply ([`tase_executor::apply`]) and hands it to the host through
//!    the `DocumentEditAdapter` - the runner writes no file of its own;
//! 3. when the host refuses a document or does not confirm the write, it asks the executor to **roll back** the
//!    documents that were written, so that a half-finished job never looks like a success;
//! 4. when the checks passed, it asks the executor to **commit** and to move the ref with the compare-and-swap of
//!    the executor.
//!
//! # The fencing of the runner
//!
//! The runner checks the lease of the job before it writes and before it commits: a worker whose lease ran out may not
//! write to the host and may not move the ref (detailed design §6.2). The check is the [`FenceGuard`] of the caller,
//! and the time of the check is the time of the caller: the runner never reads the clock.
//!
//! # The rebase
//!
//! The decision of the rebase needs the symbol index of the new head, and the symbol index belongs to `tase-semantic`,
//! which the worker does not depend on. The runner therefore asks a [`RebaseGate`]: the host that owns the index
//! answers with the decision of `tase-executor`, and the worker never writes a second rebase logic.

use tase_contract::TaseError;
use tase_contract::api::PlanRequest;
use tase_executor::apply::{
    ApplyOutcome, ApplyPlan, DocumentEditAdapter, apply_to_host, document_of, rollback,
};
use tase_executor::git_tx::{FileWrite, GitRepo, commit_message};
use tase_executor::preview::{ChangeKind, FileChange, Preview};
use tase_executor::rebase::RebaseOutcome;

use crate::lease::FenceGuard;

/// One document of a job: the path, the version, the text before the change and the text after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentInput {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// The version the document has in the host.
    pub base_version: i64,
    /// The text of the document before the change.
    pub base_text: String,
    /// The text of the document after the change.
    pub new_text: String,
    /// The symbols the change touches.
    pub symbols: Vec<String>,
}

impl DocumentInput {
    /// A document of a job.
    pub fn new(path: &str, base_version: i64, base_text: &str, new_text: &str) -> DocumentInput {
        DocumentInput {
            path: path.to_string(),
            base_version,
            base_text: base_text.to_string(),
            new_text: new_text.to_string(),
            symbols: Vec::new(),
        }
    }
}

/// What the worker asks the runner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRequest {
    /// The id of the job: it travels in the trailer of the commit.
    pub job_id: String,
    /// The digest of the plan of the job.
    pub plan_digest: String,
    /// The commit the job starts from.
    pub base_commit: String,
    /// The ref the commit has to move.
    pub target_ref: String,
    /// The documents of the job.
    pub documents: Vec<DocumentInput>,
}

/// What the runner did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    /// The preview of the job: the diff of the executor.
    pub preview: Preview,
    /// What the apply of the executor answered.
    pub outcome: ApplyOutcome,
    /// How many documents were rolled back.
    pub rolled_back: usize,
}

impl RunOutcome {
    /// Whether every document of the job was written.
    pub fn is_applied(&self) -> bool {
        self.outcome.is_applied()
    }

    /// The documents that were written.
    pub fn written(&self) -> &[String] {
        self.outcome.written()
    }
}

/// The decision of the rebase, asked to the layer that owns the symbol index.
///
/// The answer is the decision of `tase-executor` ([`RebaseOutcome`]): the worker asks, it does not decide.
pub trait RebaseGate {
    /// The decision for the plan of a job on the new head.
    fn decide(&self, request: &PlanRequest, head: &str) -> RebaseOutcome;
}

/// The runner of a job.
#[derive(Clone, Copy, Debug, Default)]
pub struct Runner;

impl Runner {
    /// A runner.
    pub fn new() -> Runner {
        Runner
    }

    /// The changes of a job, as the executor computes them.
    ///
    /// The diff is the diff of the executor: this crate writes no second diff.
    pub fn changes_of(request: &RunRequest) -> Result<Vec<FileChange>, TaseError> {
        let mut changes = Vec::with_capacity(request.documents.len());
        for document in &request.documents {
            let kind = if document.base_text.is_empty() {
                ChangeKind::Created
            } else {
                ChangeKind::Modified
            };
            changes.push(FileChange::new(
                &document.path,
                kind,
                &document.base_text,
                &document.new_text,
                &document.symbols,
            )?);
        }
        Ok(changes)
    }

    /// The preview of a job: the diff, the risk and the digest of the tree the job would produce.
    pub fn preview_of(request: &RunRequest) -> Result<Preview, TaseError> {
        Ok(Preview::build(
            &request.base_commit,
            &request.plan_digest,
            Runner::changes_of(request)?,
        ))
    }

    /// The plan of the apply of a job: the documents and the edits of the executor.
    pub fn plan_of(request: &RunRequest) -> Result<ApplyPlan, TaseError> {
        let mut documents = Vec::with_capacity(request.documents.len());
        for document in &request.documents {
            documents.push(document_of(
                &document.path,
                document.base_version,
                &document.base_text,
                &document.new_text,
            )?);
        }
        Ok(ApplyPlan::new(
            &request.base_commit,
            &request.plan_digest,
            documents,
        ))
    }

    /// Hands a job to the host.
    ///
    /// The lease of the job is checked first: a worker that is not the holder any more writes nothing. When the host
    /// refuses a document or does not confirm the write, the documents that were written are rolled back and the
    /// outcome names what happened.
    pub fn apply(
        &self,
        request: &RunRequest,
        adapter: &mut dyn DocumentEditAdapter,
        guard: &dyn FenceGuard,
        now_ms: u64,
    ) -> Result<RunOutcome, TaseError> {
        guard.check(now_ms).map_err(|error| error.to_error())?;
        let plan = Runner::plan_of(request)?;
        let outcome = apply_to_host(adapter, &plan)?;
        let rolled_back = if outcome.is_applied() {
            0
        } else {
            rollback(adapter, &plan, outcome.written())?
        };
        let preview = Runner::preview_of(request)?;
        Ok(RunOutcome {
            preview,
            outcome,
            rolled_back,
        })
    }

    /// Commits a job and moves the ref.
    ///
    /// The commit is the commit of the executor: the files of the job go into **one** commit, and the message carries
    /// the trailer of the job, so that the recovery finds the job in the history of the repository itself. The ref
    /// moves with the compare-and-swap of the executor: it only moves when the ref still is at the commit the job
    /// started from.
    pub fn commit(
        &self,
        request: &RunRequest,
        git: &mut dyn GitRepo,
        parent: &str,
        guard: &dyn FenceGuard,
        now_ms: u64,
    ) -> Result<String, TaseError> {
        guard.check(now_ms).map_err(|error| error.to_error())?;
        let files: Vec<(String, FileWrite)> = request
            .documents
            .iter()
            .map(|document| {
                (
                    document.path.clone(),
                    FileWrite::Content(document.new_text.clone()),
                )
            })
            .collect();
        let subject = format!("tase: the job {}", request.job_id);
        let message = commit_message(&subject, &request.job_id);
        let commit = git.create_commit(parent, &files, &message)?;
        git.compare_and_swap(&request.target_ref, parent, &commit)?;
        Ok(commit)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tase_executor::apply::{HostEditOutcome, HostEditRequest};
    use tase_executor::git_tx::MemoryGit;
    use tase_executor::workspace::DocumentSnapshot;

    use super::*;
    use crate::lease::{LeaseGuard, LeaseTable};

    const BASE: &str = "fn main() {\n    println!(\"hello\");\n}\n";
    const NEW: &str = "fn main() {\n    println!(\"hello, world\");\n}\n";

    /// The host of the tests: it holds the documents and it writes the edits it is given.
    #[derive(Clone, Debug, Default)]
    struct FakeHost {
        documents: BTreeMap<String, (i64, String)>,
        requests: Vec<HostEditRequest>,
        refuse: Option<String>,
    }

    impl FakeHost {
        fn with(path: &str, text: &str) -> FakeHost {
            let mut host = FakeHost::default();
            host.documents
                .insert(path.to_string(), (1, text.to_string()));
            host
        }

        fn refusing(path: &str, text: &str) -> FakeHost {
            let mut host = FakeHost::with(path, text);
            host.refuse = Some(path.to_string());
            host
        }

        fn text_of(&self, path: &str) -> String {
            self.documents
                .get(path)
                .map(|(_, text)| text.clone())
                .unwrap_or_default()
        }
    }

    impl DocumentEditAdapter for FakeHost {
        fn snapshot(&self, path: &str) -> Result<DocumentSnapshot, TaseError> {
            let (version, text) = self
                .documents
                .get(path)
                .ok_or_else(|| TaseError::not_found(format!("the host has no document {path}")))?;
            Ok(DocumentSnapshot::new(path, *version, text))
        }

        fn apply_edits(&mut self, request: &HostEditRequest) -> Result<HostEditOutcome, TaseError> {
            self.requests.push(request.clone());
            if self.refuse.as_deref() == Some(request.path.as_str()) {
                return Ok(HostEditOutcome::Rejected);
            }
            let Some((version, text)) = self.documents.get(&request.path) else {
                return Ok(HostEditOutcome::Failed);
            };
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut out: Vec<u16> = Vec::with_capacity(units.len());
            let mut cursor = 0usize;
            for edit in &request.edits {
                if edit.start < cursor || edit.end > units.len() {
                    return Ok(HostEditOutcome::Failed);
                }
                out.extend_from_slice(&units[cursor..edit.start]);
                out.extend(edit.text.encode_utf16());
                cursor = edit.end;
            }
            out.extend_from_slice(&units[cursor..]);
            let new_text = String::from_utf16(&out).map_err(|error| {
                TaseError::internal(format!("the host wrote invalid text: {error}"))
            })?;
            self.documents
                .insert(request.path.clone(), (version + 1, new_text));
            Ok(HostEditOutcome::Accepted)
        }
    }

    fn request() -> RunRequest {
        RunRequest {
            job_id: "job_000000000001".to_string(),
            plan_digest: "sha256:plan".to_string(),
            base_commit: "commit-1".to_string(),
            target_ref: "refs/heads/main".to_string(),
            documents: vec![DocumentInput::new("src/main.rs", 1, BASE, NEW)],
        }
    }

    /// A table with a lease of the job of the tests.
    fn lease_table() -> (LeaseTable, u64) {
        let mut table = LeaseTable::new();
        let lease = table
            .acquire("job_000000000001", "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        (table, lease.fencing_token)
    }

    #[test]
    fn the_runner_shows_the_diff_of_the_executor() {
        let preview = Runner::preview_of(&request()).expect("the preview is built");
        assert_eq!(preview.base_commit, "commit-1");
        assert_eq!(preview.plan_digest, "sha256:plan");
        assert_eq!(preview.changed_paths(), ["src/main.rs"]);
        assert!(preview.diff_text.contains("hello, world"));
        assert!(preview.new_tree_digest.starts_with("sha256:"));
    }

    #[test]
    fn the_runner_hands_the_edits_to_the_host() {
        let (table, token) = lease_table();
        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let mut host = FakeHost::with("src/main.rs", BASE);
        let runner = Runner::new();

        let outcome = runner
            .apply(&request(), &mut host, &guard, 2_000)
            .expect("the job is handed to the host");

        assert!(outcome.is_applied());
        assert_eq!(outcome.written().len(), 1);
        assert_eq!(outcome.written()[0], "src/main.rs");
        assert_eq!(host.text_of("src/main.rs"), NEW);
        assert_eq!(host.requests.len(), 1);
        assert_eq!(host.requests[0].path, "src/main.rs");
    }

    #[test]
    fn the_runner_rolls_the_job_back_when_the_host_refuses() {
        let (table, token) = lease_table();
        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let mut host = FakeHost::refusing("src/main.rs", BASE);
        let runner = Runner::new();

        let outcome = runner
            .apply(&request(), &mut host, &guard, 2_000)
            .expect("the job is handed to the host");

        assert!(!outcome.is_applied());
        assert!(matches!(outcome.outcome, ApplyOutcome::Rejected { .. }));
        assert_eq!(outcome.rolled_back, 0);
        assert_eq!(host.text_of("src/main.rs"), BASE, "the host kept the base");
    }

    #[test]
    fn the_runner_rolls_the_written_documents_back() {
        let (table, token) = lease_table();
        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let mut host = FakeHost::with("src/main.rs", BASE);
        let runner = Runner::new();

        // the first document is written, the second one is refused
        let mut job = request();
        job.documents.push(DocumentInput::new(
            "src/lib.rs",
            1,
            "pub fn run() {}\n",
            "pub fn run() -> bool { true }\n",
        ));
        host.documents.insert(
            "src/lib.rs".to_string(),
            (1, "pub fn run() {}\n".to_string()),
        );
        host.refuse = Some("src/lib.rs".to_string());

        let outcome = runner
            .apply(&job, &mut host, &guard, 2_000)
            .expect("the job is handed to the host");

        assert!(!outcome.is_applied());
        assert_eq!(outcome.rolled_back, 1);
        assert_eq!(
            host.text_of("src/main.rs"),
            BASE,
            "the document that was written is back at the base"
        );
    }

    #[test]
    fn the_runner_writes_nothing_when_the_lease_is_old() {
        let mut table = LeaseTable::new();
        let first = table
            .acquire("job_000000000001", "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let token = first.fencing_token;

        // the lease of the job is taken back by somebody else
        let _released = table.release("job_000000000001", "worker-1", token);
        let _second = table
            .acquire("job_000000000001", "worker-2", 30_000, 2_000)
            .expect("the lease is taken again");

        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let mut host = FakeHost::with("src/main.rs", BASE);
        let runner = Runner::new();

        let error = runner
            .apply(&request(), &mut host, &guard, 2_500)
            .expect_err("the write of the old holder is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::PermissionDenied);
        assert!(host.requests.is_empty(), "the host was not asked to write");
    }

    #[test]
    fn the_runner_commits_the_job_and_moves_the_ref() {
        let (table, token) = lease_table();
        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let runner = Runner::new();
        let mut git = MemoryGit::new();
        git.set_head("refs/heads/main", "commit-1");

        let commit = runner
            .commit(&request(), &mut git, "commit-1", &guard, 2_000)
            .expect("the job is committed");

        assert_eq!(
            git.head_of("refs/heads/main").map(String::as_str),
            Some(commit.as_str())
        );
        let message = git.message_of(&commit).expect("the commit has a message");
        assert!(message.contains("TASE-Job-ID: job_000000000001"));
        assert_eq!(
            git.file("src/main.rs").map(String::as_str),
            Some(NEW),
            "the commit holds the new text"
        );
    }

    #[test]
    fn the_runner_does_not_move_the_ref_when_it_moved() {
        let (table, token) = lease_table();
        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let runner = Runner::new();
        let mut git = MemoryGit::new();
        git.set_head("refs/heads/main", "commit-2");

        let error = runner
            .commit(&request(), &mut git, "commit-1", &guard, 2_000)
            .expect_err("the compare-and-swap fails");
        assert_eq!(error.code, tase_contract::ErrorCode::StaleBase);
        assert_eq!(
            git.head_of("refs/heads/main").map(String::as_str),
            Some("commit-2"),
            "the ref did not move"
        );
    }

    #[test]
    fn the_runner_commits_nothing_when_the_lease_is_old() {
        let mut table = LeaseTable::new();
        let first = table
            .acquire("job_000000000001", "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let token = first.fencing_token;

        let _released = table.release("job_000000000001", "worker-1", token);
        let _second = table
            .acquire("job_000000000001", "worker-2", 30_000, 2_000)
            .expect("the lease is taken again");

        let guard = LeaseGuard::new(&table, "job_000000000001", token);
        let runner = Runner::new();
        let mut git = MemoryGit::new();
        git.set_head("refs/heads/main", "commit-1");

        let error = runner
            .commit(&request(), &mut git, "commit-1", &guard, 2_500)
            .expect_err("the commit of the old holder is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::PermissionDenied);
        assert_eq!(
            git.head_of("refs/heads/main").map(String::as_str),
            Some("commit-1"),
            "the ref did not move"
        );
    }
}
