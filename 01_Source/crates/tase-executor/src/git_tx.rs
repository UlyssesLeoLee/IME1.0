//! The Git transaction: the commit object and the compare-and-swap of a ref (detailed design §6.3 and §5.5).
//!
//! # What the crate does with Git
//!
//! Detailed design §6.3 is explicit: the worker builds a candidate Git tree in a temporary snapshot, it validates
//! every file in an isolated worktree, it creates the commit object and it moves the protected ref with a
//! compare-and-swap. If the ref moved while the worker was working, the job goes to a rebase or to `CONFLICT`:
//! **a part of the files may never be declared as committed.**
//!
//! This module holds the last two steps: [`GitRepo::create_commit`] builds the commit object (every file of the plan
//! goes into **one** commit) and [`GitRepo::compare_and_swap`] moves the ref, but only when the ref still is where the
//! plan expects it.
//!
//! # The atomicity of a multi-file change
//!
//! The boundary of the atomicity is the ref:
//!
//! * every file of the plan is written into one tree and one commit object, so the content is never half written;
//! * the ref moves **once**, with the old value of the ref as the condition of the move, so the move is atomic: Git
//!   takes the lock of the ref, checks the old value and writes the new one;
//! * when the check fails, nothing moved: the working tree, the index and the ref are what they were, and the caller
//!   gets a `STALE_BASE`.
//!
//! # No Git library
//!
//! This repository has no Git library and the dependency policy allows none: the Git capability is the `git`
//! command line, driven with a fixed argument vector (never a shell string) and with the environment of the author
//! set, so a commit never fails because a name is missing. The tests that need the command line skip themselves when
//! it is not there, and the tests that decide something always use the memory implementation.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use tase_contract::TaseError;

use crate::rebase::{ChangeSetSource, ChangedFile};
use crate::Result;

/// The name the commits of the executor carry.
const AUTHOR_NAME: &str = "tase-executor";

/// The address the commits of the executor carry.
const AUTHOR_EMAIL: &str = "tase-executor@localhost";

/// The counter of the temporary indexes.
static INDEX_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What a commit does to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileWrite {
    /// The file holds this text.
    Content(String),
    /// The file is gone.
    Deleted,
}

/// The Git operations the executor needs.
///
/// The trait is the door of the executor to Git. It is small on purpose: the executor never walks the history and
/// never reads a tree itself, it asks for the head, for the history and for the files that changed.
pub trait GitRepo {
    /// The commit the ref points at.
    fn head(&self, ref_name: &str) -> Result<String>;

    /// Whether `ancestor` is in the history of `descendant`.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool>;

    /// The files that changed between two commits.
    fn changed_files(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>>;

    /// Creates a commit object that holds every change on top of `parent`.
    ///
    /// Every file of the plan goes into **one** commit: the content of a plan is never split over two commits.
    fn create_commit(
        &mut self,
        parent: &str,
        files: &[(String, FileWrite)],
        message: &str,
    ) -> Result<String>;

    /// Moves the ref from `expected` to `new_commit`, or refuses.
    ///
    /// The move happens only when the ref still is at `expected`: this is the compare-and-swap of detailed design
    /// §6.3. The ref is never forced and the history is never rewritten.
    fn compare_and_swap(&mut self, ref_name: &str, expected: &str, new_commit: &str) -> Result<()>;
}

/// The message of a commit of the executor.
///
/// The message carries the trailer of detailed design §5.2 (`TASE-Job-ID`), so that the reconciler of detailed design
/// §5.5 finds the job of a commit in the history of the repository itself.
pub fn commit_message(subject: &str, job_id: &str) -> String {
    format!("{subject}\n\nTASE-Job-ID: {job_id}\n")
}

/// The Git of the `git` command line.
///
/// The command is always started with a fixed argument vector and with the author of the commits set, so that a
/// commit never waits for a name and a prompt.
pub struct GitCli {
    work_dir: PathBuf,
}

impl GitCli {
    /// The Git of a working directory.
    pub fn new(work_dir: &Path) -> GitCli {
        GitCli {
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// The working directory of the Git.
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// The command of the Git, with the author of the commits and without any prompt.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(&self.work_dir)
            .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
            .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
            .env("GIT_COMMITTER_NAME", AUTHOR_NAME)
            .env("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL)
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    /// Runs the Git and gives the output back.
    fn output(&self, args: &[&str], env: &[(&str, &str)], input: Option<&str>) -> Result<Output> {
        let mut command = self.command(args);
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                TaseError::worker_unavailable(format!("the git command line could not be started: {error}"))
            })?;
        if let Some(text) = input {
            let mut stdin = child.stdin.take().expect("the stdin of the child is piped");
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| {
                    TaseError::worker_unavailable(format!("the git command line could not be written: {error}"))
                })?;
            drop(stdin);
        }
        let output = child.wait_with_output().map_err(|error| {
            TaseError::worker_unavailable(format!("the git command line could not be waited for: {error}"))
        })?;
        Ok(output)
    }

    /// Runs the Git and gives the standard output back; a failure is an error of the executor.
    fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.output(args, &[], None)?;
        if !output.status.success() {
            return Err(TaseError::worker_unavailable(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Runs the Git with an environment and gives the standard output back.
    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
        let output = self.output(args, env, None)?;
        if !output.status.success() {
            return Err(TaseError::worker_unavailable(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Runs the Git with an environment and with something on its standard input.
    fn run_env_with_input(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        input: &str,
    ) -> Result<String> {
        let output = self.output(args, env, Some(input))?;
        if !output.status.success() {
            return Err(TaseError::worker_unavailable(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

impl GitRepo for GitCli {
    fn head(&self, ref_name: &str) -> Result<String> {
        let spec = format!("{ref_name}^{{commit}}");
        let output = self.output(&["rev-parse", "--verify", "--quiet", &spec], &[], None)?;
        if !output.status.success() {
            return Err(TaseError::not_found(format!(
                "the ref {ref_name} does not exist"
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let output = self.output(
            &["merge-base", "--is-ancestor", ancestor, descendant],
            &[],
            None,
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(TaseError::worker_unavailable(format!(
                "the history could not be read: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }

    fn changed_files(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
        let output = self.run(&["diff", "--name-status", from, to])?;
        let mut changed = Vec::new();
        for line in output.lines() {
            let mut parts = line.split('\t');
            let Some(status) = parts.next() else {
                continue;
            };
            let Some(path) = parts.next() else {
                continue;
            };
            // a rename names the old path and the new path: the new one is the file of the change
            let path = if status.starts_with('R') {
                parts.next().unwrap_or(path)
            } else {
                path
            };
            changed.push(ChangedFile {
                path: path.to_string(),
                old_blob: String::new(),
                new_blob: String::new(),
            });
        }
        Ok(changed)
    }

    fn create_commit(
        &mut self,
        parent: &str,
        files: &[(String, FileWrite)],
        message: &str,
    ) -> Result<String> {
        let index_path = std::env::temp_dir().join(format!(
            "tase-executor-index-{}-{}",
            std::process::id(),
            INDEX_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&index_path);
        let index = index_path.to_string_lossy().to_string();
        let env = [("GIT_INDEX_FILE", index.as_str())];

        let result = (|| -> Result<String> {
            self.run_env(&["read-tree", parent], &env)?;
            for (path, write) in files {
                match write {
                    FileWrite::Content(text) => {
                        let blob = self.run_env_with_input(&["hash-object", "-w", "--stdin"], &env, text)?;
                        let cache_info = format!("100644,{blob},{path}");
                        self.run_env(&["update-index", "--add", "--cacheinfo", &cache_info], &env)?;
                    }
                    FileWrite::Deleted => {
                        self.run_env(&["update-index", "--force-remove", path], &env)?;
                    }
                }
            }
            let tree = self.run_env(&["write-tree"], &env)?;
            self.run(&["commit-tree", &tree, "-p", parent, "-m", message])
        })();
        let _ = std::fs::remove_file(&index_path);
        result
    }

    fn compare_and_swap(&mut self, ref_name: &str, expected: &str, new_commit: &str) -> Result<()> {
        let output = self.output(&["update-ref", ref_name, new_commit, expected], &[], None)?;
        if output.status.success() {
            return Ok(());
        }
        Err(TaseError::stale_base(format!(
            "the ref {ref_name} is not at {expected}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

impl ChangeSetSource for GitCli {
    fn is_ancestor_of(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        GitRepo::is_ancestor(self, ancestor, descendant)
    }

    fn changed_files_between(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
        GitRepo::changed_files(self, from, to)
    }
}

/// One commit of the memory.
#[derive(Clone, Debug)]
struct MemoryCommit {
    /// The commit this one is on top of.
    parent: String,
    /// The files of the commit.
    files: BTreeMap<String, String>,
    /// The message of the commit.
    message: String,
}

/// The Git of a test: the refs, the commits and the files live in the memory.
///
/// The implementation answers exactly like the command line does, without a repository and without a process: the
/// tests that decide something (the rebase, the conflict, the compare-and-swap) are therefore deterministic and run
/// everywhere.
#[derive(Clone, Debug, Default)]
pub struct MemoryGit {
    refs: BTreeMap<String, String>,
    commits: BTreeMap<String, MemoryCommit>,
    files: BTreeMap<String, String>,
    counter: u64,
}

impl MemoryGit {
    /// The Git of nothing.
    pub fn new() -> MemoryGit {
        MemoryGit::default()
    }

    /// Writes a file of the working tree.
    pub fn write_file(&mut self, path: &str, content: &str) {
        self.files.insert(path.to_string(), content.to_string());
    }

    /// The content of a file of the working tree.
    pub fn file(&self, path: &str) -> Option<&String> {
        self.files.get(path)
    }

    /// Points a ref at a commit.
    pub fn set_head(&mut self, ref_name: &str, commit: &str) {
        self.refs.insert(ref_name.to_string(), commit.to_string());
    }

    /// The commit a ref points at.
    pub fn head_of(&self, ref_name: &str) -> Option<&String> {
        self.refs.get(ref_name)
    }

    /// Creates a commit on top of a parent and gives its id back.
    ///
    /// The commit holds the files of its parent, with the files of the change written over them. A file of the change
    /// that is deleted is gone from the commit.
    pub fn commit(
        &mut self,
        parent: &str,
        files: &[(String, FileWrite)],
        message: &str,
    ) -> Result<String> {
        let mut tree = match self.commits.get(parent) {
            Some(commit) => commit.files.clone(),
            None => BTreeMap::new(),
        };
        for (path, write) in files {
            match write {
                FileWrite::Content(text) => {
                    tree.insert(path.clone(), text.clone());
                }
                FileWrite::Deleted => {
                    tree.remove(path);
                }
            }
        }
        self.counter += 1;
        let id = format!("{:040x}", self.counter);
        self.commits.insert(
            id.clone(),
            MemoryCommit {
                parent: parent.to_string(),
                files: tree,
                message: message.to_string(),
            },
        );
        self.files = self
            .commits
            .get(&id)
            .map(|commit| commit.files.clone())
            .unwrap_or_default();
        Ok(id)
    }

    /// The message of a commit.
    pub fn message_of(&self, commit: &str) -> Option<&str> {
        self.commits
            .get(commit)
            .map(|c| c.message.as_str())
    }
}

impl GitRepo for MemoryGit {
    fn head(&self, ref_name: &str) -> Result<String> {
        self.refs
            .get(ref_name)
            .cloned()
            .ok_or_else(|| TaseError::not_found(format!("the ref {ref_name} does not exist")))
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let mut current = descendant.to_string();
        loop {
            if current == ancestor {
                return Ok(true);
            }
            match self.commits.get(&current) {
                Some(commit) if !commit.parent.is_empty() => current = commit.parent.clone(),
                _ => return Ok(false),
            }
        }
    }

    fn changed_files(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
        let before = self
            .commits
            .get(from)
            .map(|commit| commit.files.clone())
            .unwrap_or_default();
        let after = self
            .commits
            .get(to)
            .map(|commit| commit.files.clone())
            .unwrap_or_default();
        let mut changed = Vec::new();
        for (path, content) in &after {
            if before.get(path) != Some(content) {
                changed.push(ChangedFile {
                    path: path.clone(),
                    old_blob: before.get(path).cloned().unwrap_or_default(),
                    new_blob: content.clone(),
                });
            }
        }
        for (path, content) in &before {
            if !after.contains_key(path) {
                changed.push(ChangedFile {
                    path: path.clone(),
                    old_blob: content.clone(),
                    new_blob: String::new(),
                });
            }
        }
        Ok(changed)
    }

    fn create_commit(
        &mut self,
        parent: &str,
        files: &[(String, FileWrite)],
        message: &str,
    ) -> Result<String> {
        self.commit(parent, files, message)
    }

    fn compare_and_swap(&mut self, ref_name: &str, expected: &str, new_commit: &str) -> Result<()> {
        match self.refs.get(ref_name) {
            Some(current) if current == expected => {
                self.refs
                    .insert(ref_name.to_string(), new_commit.to_string());
                Ok(())
            }
            Some(current) => Err(TaseError::stale_base(format!(
                "the ref {ref_name} is at {current}, not at {expected}"
            ))),
            None => Err(TaseError::stale_base(format!(
                "the ref {ref_name} does not exist"
            ))),
        }
    }
}

impl ChangeSetSource for MemoryGit {
    fn is_ancestor_of(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        GitRepo::is_ancestor(self, ancestor, descendant)
    }

    fn changed_files_between(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
        GitRepo::changed_files(self, from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_with_a_commit() -> (MemoryGit, String) {
        let mut git = MemoryGit::new();
        git.write_file("src/main.rs", "fn main() {}\n");
        let first = git
            .commit(
                "",
                &[(
                    "src/main.rs".to_string(),
                    FileWrite::Content("fn main() {}\n".to_string()),
                )],
                "the first commit",
            )
            .expect("the commit is created");
        git.set_head("refs/heads/main", &first);
        (git, first)
    }

    #[test]
    fn the_compare_and_swap_moves_the_ref_when_the_head_is_the_one_that_was_expected() {
        let (mut git, first) = memory_with_a_commit();
        let second = git
            .commit(
                &first,
                &[(
                    "src/main.rs".to_string(),
                    FileWrite::Content("fn main() { /* changed */ }\n".to_string()),
                )],
                "the second commit",
            )
            .expect("the commit is created");
        git.compare_and_swap("refs/heads/main", &first, &second)
            .expect("the ref moves");
        assert_eq!(git.head("refs/heads/main").expect("the ref is there"), second);
    }

    #[test]
    fn the_compare_and_swap_refuses_a_head_that_moved_and_changes_nothing() {
        let (mut git, first) = memory_with_a_commit();
        let second = git
            .commit(
                &first,
                &[(
                    "src/main.rs".to_string(),
                    FileWrite::Content("fn main() { /* changed */ }\n".to_string()),
                )],
                "the second commit",
            )
            .expect("the commit is created");
        // somebody else moved the ref
        git.set_head("refs/heads/main", &second);

        let error = git
            .compare_and_swap("refs/heads/main", &first, &second)
            .unwrap_err();
        assert_eq!(error.code, tase_contract::ErrorCode::StaleBase);
        // the ref is where it was and the working tree is untouched
        assert_eq!(git.head("refs/heads/main").expect("the ref is there"), second);
        assert_eq!(
            git.file("src/main.rs").map(String::as_str),
            Some("fn main() { /* changed */ }\n")
        );
    }

    #[test]
    fn the_compare_and_swap_never_creates_a_ref_that_is_not_there() {
        let (mut git, first) = memory_with_a_commit();
        let error = git
            .compare_and_swap("refs/heads/other", &first, &first)
            .unwrap_err();
        assert_eq!(error.code, tase_contract::ErrorCode::StaleBase);
        assert!(git.head_of("refs/heads/other").is_none());
    }

    #[test]
    fn the_history_tells_what_is_an_ancestor_of_what() {
        let (mut git, first) = memory_with_a_commit();
        let second = git
            .commit(
                &first,
                &[("src/other.rs".to_string(), FileWrite::Content("x\n".to_string()))],
                "the second commit",
            )
            .expect("the commit is created");
        assert!(git.is_ancestor(&first, &second).expect("the history is read"));
        assert!(!git.is_ancestor(&second, &first).expect("the history is read"));
        assert!(!git.is_ancestor("0000", &second).expect("the history is read"));
    }

    #[test]
    fn the_changed_files_are_the_files_that_are_not_the_same() {
        let (mut git, first) = memory_with_a_commit();
        let second = git
            .commit(
                &first,
                &[(
                    "src/main.rs".to_string(),
                    FileWrite::Content("fn main() { /* changed */ }\n".to_string()),
                )],
                "the second commit",
            )
            .expect("the commit is created");
        let changed = git.changed_files(&first, &second).expect("the diff is read");
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].path, "src/main.rs");
    }

    #[test]
    fn a_commit_holds_every_file_of_the_plan_in_one_commit() {
        let (mut git, first) = memory_with_a_commit();
        let second = git
            .create_commit(
                &first,
                &[
                    (
                        "src/main.rs".to_string(),
                        FileWrite::Content("fn main() {}\nfn other() {}\n".to_string()),
                    ),
                    (
                        "src/other.rs".to_string(),
                        FileWrite::Content("fn other() {}\n".to_string()),
                    ),
                ],
                &commit_message("the change of the job", "job_001"),
            )
            .expect("the commit is created");
        // the message carries the trailer of the design book
        let message = git.message_of(&second).expect("the commit is there");
        assert!(message.contains("TASE-Job-ID: job_001"));
        // both files are in the one commit
        let changed = git.changed_files(&first, &second).expect("the diff is read");
        assert_eq!(changed.len(), 2);
    }

    #[test]
    fn a_file_that_is_deleted_is_gone_from_the_commit() {
        let (mut git, first) = memory_with_a_commit();
        let second = git
            .commit(
                &first,
                &[("src/main.rs".to_string(), FileWrite::Deleted)],
                "the file is gone",
            )
            .expect("the commit is created");
        let changed = git.changed_files(&first, &second).expect("the diff is read");
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].path, "src/main.rs");
        assert_eq!(changed[0].new_blob, "");
    }

    /// Whether the `git` command line is on the machine.
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// A repository in a temporary directory, with one commit.
    fn real_repository(name: &str) -> Option<(PathBuf, String)> {
        let dir = std::env::temp_dir().join(format!("tase-executor-git-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the directory is created");
        let run = |args: &[&str]| -> bool {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
                .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
                .env("GIT_COMMITTER_NAME", AUTHOR_NAME)
                .env("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL)
                .output()
                .is_ok_and(|output| output.status.success())
        };
        if !run(&["init", "--quiet"]) {
            return None;
        }
        if !run(&["config", "user.email", AUTHOR_EMAIL]) || !run(&["config", "user.name", AUTHOR_NAME]) {
            return None;
        }
        std::fs::write(dir.join("file.txt"), "one\n").expect("the file is written");
        if !run(&["add", "file.txt"]) || !run(&["commit", "--quiet", "-m", "the first commit"]) {
            return None;
        }
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())?;
        Some((dir, head))
    }

    #[test]
    fn the_command_line_refuses_a_compare_and_swap_whose_head_moved() {
        if !git_available() {
            eprintln!("skipped: the git command line is not on this machine");
            return;
        }
        let Some((dir, head)) = real_repository("cas") else {
            eprintln!("skipped: the git command line could not make a repository");
            return;
        };
        let mut git = GitCli::new(&dir);

        // a commit that holds the same content: the tree is the same, the commit is a new one
        let commit = git
            .create_commit(
                &head,
                &[("file.txt".to_string(), FileWrite::Content("two\n".to_string()))],
                &commit_message("the change of the job", "job_001"),
            )
            .expect("the commit is created");

        // the head is not the one that was expected: the ref must not move
        let error = git
            .compare_and_swap("refs/heads/master", "0000000000000000000000000000000000000000", &commit)
            .unwrap_err();
        assert_eq!(error.code, tase_contract::ErrorCode::StaleBase);
        // the ref and the working tree are what they were
        assert_eq!(git.head("refs/heads/master").expect("the ref is there"), head);
        assert_eq!(
            std::fs::read_to_string(dir.join("file.txt")).expect("the file is there"),
            "one\n"
        );

        // the head is the one that was expected: the ref moves
        git.compare_and_swap("refs/heads/master", &head, &commit)
            .expect("the ref moves");
        assert_eq!(git.head("refs/heads/master").expect("the ref is there"), commit);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_command_line_reads_the_head_the_history_and_the_changed_files() {
        if !git_available() {
            eprintln!("skipped: the git command line is not on this machine");
            return;
        }
        let Some((dir, head)) = real_repository("read") else {
            eprintln!("skipped: the git command line could not make a repository");
            return;
        };
        let mut git = GitCli::new(&dir);
        assert_eq!(git.head("refs/heads/master").expect("the ref is there"), head);
        assert!(git.is_ancestor(&head, &head).expect("the history is read"));

        let commit = git
            .create_commit(
                &head,
                &[("file.txt".to_string(), FileWrite::Content("two\n".to_string()))],
                "the change",
            )
            .expect("the commit is created");
        let changed = git.changed_files(&head, &commit).expect("the diff is read");
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].path, "file.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
