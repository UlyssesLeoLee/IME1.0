//! The pinned workspace of a plan: the base the preview was computed against, and the rules of the paths.
//!
//! The workspace of detailed design §5.1 is the snapshot of the repository the preview runs on: a pinned base commit,
//! a pinned template set and a pinned content for every file. The preview is only meaningful against the base it was
//! computed on, so the base is kept as a **digest** of every file: the text itself is never kept here, the host holds
//! it.
//!
//! # The rules of the paths
//!
//! The sandbox of detailed design §7.2 refuses `..`, the absolute paths, the paths that leave the workspace, the
//! case collisions and the Windows drive and UNC injections. [`validate_repo_relative_path`] is that rule, written
//! once: a path of this crate is **relative to the root of the repository**, it uses `/` as the separator and it
//! names nothing that the operating system could interpret in another way. A path that does not pass is refused
//! before it reaches the disk, the diff or the commit.

use pime_core::sha256::sha256_hex;
use tase_contract::TaseError;

use crate::Result;

/// The names of the devices of Windows that may not name a file.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The state of one document, as the host editor sees it.
///
/// The executor never reads the disk and never keeps a buffer: the text comes from the host, in one piece, when the
/// host is asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSnapshot {
    /// The path of the file, relative to the root of the repository, with `/` as the separator.
    pub path: String,
    /// The version of the document in the host; the host increments it on every change.
    pub version: i64,
    /// The text of the document at the moment of the snapshot.
    pub text: String,
    /// The SHA-256 of the UTF-8 bytes of the text.
    pub sha256: String,
    /// Whether the document holds changes that are not saved.
    pub dirty: bool,
}

impl DocumentSnapshot {
    /// A snapshot of a document; the hash is computed from the text.
    pub fn new(path: &str, version: i64, text: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            path: path.to_string(),
            version,
            text: text.to_string(),
            sha256: sha256_hex(text),
            dirty: false,
        }
    }

    /// The same snapshot, with the flag of the unsaved changes set.
    pub fn with_dirty(mut self, dirty: bool) -> DocumentSnapshot {
        self.dirty = dirty;
        self
    }
}

/// A file of the pinned workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedFile {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// The SHA-256 of the content of the file.
    pub sha256: String,
}

/// The pinned workspace of a plan: the base commit, the digest of the plan and the digest of every file.
///
/// The workspace is what makes a preview reproducible: the same plan on the same base gives the same diff, and a
/// file that moved after the preview is found by [`WorkspaceSnapshot::verify`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    /// The commit the plan starts from.
    pub base_commit: String,
    /// The digest of the plan this workspace belongs to.
    pub plan_digest: String,
    files: Vec<PinnedFile>,
}

impl WorkspaceSnapshot {
    /// A workspace of a plan: the base commit and the digest of the plan.
    pub fn new(base_commit: &str, plan_digest: &str) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            base_commit: base_commit.to_string(),
            plan_digest: plan_digest.to_string(),
            files: Vec::new(),
        }
    }

    /// Pins a file of the workspace.
    ///
    /// A path that is not relative to the root of the repository is refused, a path that is pinned twice is refused
    /// and a path that collides with another one when the case is ignored is refused: two paths that differ only in
    /// their case are one path on Windows, and a plan that writes both would write one file twice.
    pub fn pin(&mut self, path: &str, sha256: &str) -> Result<()> {
        validate_repo_relative_path(path)?;
        if self.files.iter().any(|f| f.path == path) {
            return Err(TaseError::invalid_argument(format!(
                "the file {path} is pinned twice"
            )));
        }
        let folded = path.to_lowercase();
        if self
            .files
            .iter()
            .any(|f| f.path.to_lowercase() == folded)
        {
            return Err(TaseError::invalid_argument(format!(
                "the path {path} collides with another pinned path when the case is ignored"
            )));
        }
        self.files.push(PinnedFile {
            path: path.to_string(),
            sha256: sha256.to_string(),
        });
        Ok(())
    }

    /// The pinned file of a path.
    pub fn pinned(&self, path: &str) -> Option<&PinnedFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// The pinned files, in the order they were pinned.
    pub fn files(&self) -> &[PinnedFile] {
        &self.files
    }

    /// Checks that the files of the repository still are the ones the preview was computed on.
    ///
    /// The observed list holds the path and the digest of every file the caller wants to check. A file that is gone
    /// and a file whose digest moved are the same failure: the base moved (`STALE_BASE`).
    pub fn verify(&self, observed: &[(String, String)]) -> Result<()> {
        for file in &self.files {
            match observed.iter().find(|(path, _)| path == &file.path) {
                None => {
                    return Err(TaseError::stale_base(format!(
                        "the file {} is not there any more",
                        file.path
                    )));
                }
                Some((_, digest)) if digest != &file.sha256 => {
                    return Err(TaseError::stale_base(format!(
                        "the file {} changed after the preview",
                        file.path
                    )));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The digest of the pinned workspace: the identity of the base the preview was computed on.
    ///
    /// The digest is `sha256:` and the SHA-256 of the lines `path` and the digest of the file, in the order the
    /// files were pinned. It is not the digest of a Git tree: this repository has no tree to hash, and the digest
    /// only has to tell two bases apart.
    pub fn digest(&self) -> String {
        let mut text = String::new();
        for file in &self.files {
            text.push_str(&file.path);
            text.push('\u{0}');
            text.push_str(&file.sha256);
            text.push('\n');
        }
        format!("sha256:{}", sha256_hex(&text))
    }
}

/// Checks that a path is relative to the root of the repository.
///
/// The rules are the ones of the sandbox of detailed design §7.2, minus the ones that need the file system (the
/// symbolic links and the hard links): a path of this crate must not be empty, must not be absolute, must not carry
/// a drive letter or a UNC prefix, must use `/` as the separator, must not hold `.` or `..` and must not name a
/// device of Windows.
pub fn validate_repo_relative_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Err(TaseError::invalid_argument("the path is empty"));
    }
    if path.starts_with('/') {
        return Err(TaseError::invalid_argument(format!(
            "the path {path} is absolute"
        )));
    }
    if path.contains('\\') {
        return Err(TaseError::invalid_argument(format!(
            "the path {path} uses a backslash; the separator of a repository path is /"
        )));
    }
    if path.contains(':') {
        return Err(TaseError::invalid_argument(format!(
            "the path {path} carries a drive letter or a stream"
        )));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(TaseError::invalid_argument(format!(
                "the path {path} holds an empty segment"
            )));
        }
        if segment == "." || segment == ".." {
            return Err(TaseError::invalid_argument(format!(
                "the path {path} holds a relative segment"
            )));
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(TaseError::invalid_argument(format!(
                "the path {path} holds a segment that ends with a dot or a space"
            )));
        }
        let stem = segment.split('.').next().unwrap_or(segment);
        if RESERVED_NAMES
            .iter()
            .any(|name| stem.eq_ignore_ascii_case(name))
        {
            return Err(TaseError::invalid_argument(format!(
                "the path {path} names a device of Windows"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::ErrorCode;

    #[test]
    fn a_path_that_leaves_the_workspace_is_refused() {
        for bad in [
            "",
            "/etc/passwd",
            "C:\\Windows\\system32",
            "C:/Windows",
            "\\\\server\\share\\file",
            "src\\main.rs",
            "../outside.rs",
            "src/../../outside.rs",
            "./same.rs",
            "src//main.rs",
            "src/",
            "src/.",
            "src/..",
            "CON",
            "src/nul.txt",
            "src/main.rs ",
            "src/main.rs.",
        ] {
            let error = validate_repo_relative_path(bad).unwrap_err();
            assert_eq!(
                error.code,
                ErrorCode::InvalidArgument,
                "the path {bad:?} must be refused"
            );
        }
    }

    #[test]
    fn the_paths_of_a_repository_are_accepted() {
        for good in [
            "src/main.rs",
            "README.md",
            "a/b/c/d.txt",
            "src/lib.rs",
            "crates/tase-executor/src/lib.rs",
            "ファイル.rs",
        ] {
            assert!(
                validate_repo_relative_path(good).is_ok(),
                "the path {good:?} must be accepted"
            );
        }
    }

    #[test]
    fn the_snapshot_hashes_the_text_it_carries() {
        let snapshot = DocumentSnapshot::new("src/main.rs", 7, "fn main() {}\n");
        assert_eq!(snapshot.version, 7);
        assert_eq!(snapshot.sha256, sha256_hex("fn main() {}\n"));
        assert!(!snapshot.dirty);
        assert!(snapshot.with_dirty(true).dirty);
    }

    #[test]
    fn the_workspace_pins_the_files_of_the_base() {
        let mut workspace = WorkspaceSnapshot::new("commit-1", "sha256:plan");
        workspace.pin("src/main.rs", "sha256:a").expect("the file is pinned");
        workspace.pin("src/lib.rs", "sha256:b").expect("the file is pinned");
        assert_eq!(workspace.files().len(), 2);
        assert_eq!(
            workspace.pinned("src/main.rs").map(|f| f.sha256.as_str()),
            Some("sha256:a")
        );
        // the digest of the workspace is stable and depends on the files
        let digest = workspace.digest();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest, workspace.digest());
        let mut other = WorkspaceSnapshot::new("commit-1", "sha256:plan");
        other.pin("src/lib.rs", "sha256:b").expect("the file is pinned");
        other.pin("src/main.rs", "sha256:a").expect("the file is pinned");
        assert_ne!(digest, other.digest(), "the order of the pins is part of the digest");
    }

    #[test]
    fn a_file_that_is_pinned_twice_is_refused() {
        let mut workspace = WorkspaceSnapshot::new("commit-1", "sha256:plan");
        workspace.pin("src/main.rs", "sha256:a").expect("the file is pinned");
        let error = workspace.pin("src/main.rs", "sha256:b").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn two_paths_that_differ_only_in_the_case_are_one_path() {
        let mut workspace = WorkspaceSnapshot::new("commit-1", "sha256:plan");
        workspace.pin("src/Main.rs", "sha256:a").expect("the file is pinned");
        let error = workspace.pin("src/main.rs", "sha256:b").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("collides"));
    }

    #[test]
    fn the_workspace_finds_the_file_that_moved() {
        let mut workspace = WorkspaceSnapshot::new("commit-1", "sha256:plan");
        workspace.pin("src/main.rs", "sha256:a").expect("the file is pinned");
        workspace.pin("src/lib.rs", "sha256:b").expect("the file is pinned");

        // nothing moved
        workspace
            .verify(&[
                ("src/main.rs".to_string(), "sha256:a".to_string()),
                ("src/lib.rs".to_string(), "sha256:b".to_string()),
            ])
            .expect("the base is the same");

        // a file changed
        let error = workspace
            .verify(&[
                ("src/main.rs".to_string(), "sha256:a".to_string()),
                ("src/lib.rs".to_string(), "sha256:changed".to_string()),
            ])
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);

        // a file is gone
        let error = workspace
            .verify(&[("src/main.rs".to_string(), "sha256:a".to_string())])
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);
    }
}
