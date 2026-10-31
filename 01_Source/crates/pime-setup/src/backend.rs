//! Everything the installer does to the machine goes through [`Backend`]: files, the registry (HKLM only) and child
//! processes. `RealBackend` (in `real.rs`) is the production implementation; `fake::FakeBackend` is an in-memory
//! machine with failure injection, which is how the install, rollback and uninstall logic is tested without ever
//! touching a real system.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Str(String),
    Dword(u32),
}

pub trait Backend {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn dir_is_empty(&self, path: &Path) -> io::Result<bool>;
    /// The parent must exist; an existing folder is an error.
    fn create_dir(&mut self, path: &Path) -> io::Result<()>;
    /// Creates or overwrites.
    fn write_file(&mut self, path: &Path, data: &[u8]) -> io::Result<()>;
    /// The destination must not exist.
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&mut self, path: &Path) -> io::Result<()>;
    /// Only an empty folder is removed.
    fn remove_dir(&mut self, path: &Path) -> io::Result<()>;
    /// `root` is the install folder (or, for a folder above it, that folder itself) and `path` is `root` or below it.
    /// `Ok` when nothing exists at `path`, or when everything from `root` down to `path` is a plain file or folder: no
    /// junction, symbolic link or other reparse point on the way, and every step resolves to where its name says it is
    /// (`GetFinalPathNameByHandle` against the canonical `root`). `Err(reason)` otherwise, and the caller leaves `path`
    /// alone: a link inside a folder that ordinary users can write would make an administrator's delete or `regsvr32`
    /// act on something else. Nothing is followed to find this out, and it must run before `is_dir`, `read_file` and
    /// `dir_is_empty`, which do follow links.
    fn check_plain(&self, root: &Path, path: &Path) -> Result<(), String>;
    /// `MoveFileEx(MOVEFILE_DELAY_UNTIL_REBOOT)`: Windows deletes `path` at the next restart, whatever is at that path
    /// by then. So it is only ever called with a name nothing else will use (a file that was renamed aside first, see
    /// `exec::defer_file_delete`), or with a folder (a folder is only removed at restart while it is still empty).
    fn schedule_delete_on_reboot(&mut self, path: &Path) -> io::Result<()>;
    /// Every path (sources and destinations) in the Session Manager's `PendingFileRenameOperations` list, without the
    /// `\??\` and `!` decorations. A missing value is an empty list.
    fn pending_file_renames(&self) -> io::Result<Vec<String>>;

    /// `key` is relative to `HKEY_LOCAL_MACHINE`. `None`: the key does not exist.
    fn reg_read_key(&self, key: &str) -> io::Result<Option<Vec<(String, RegValue)>>>;
    /// Creates the key and sets the values.
    fn reg_write_key(&mut self, key: &str, values: &[(String, RegValue)]) -> io::Result<()>;
    /// Deletes the key and everything below it; a missing key is not an error.
    fn reg_delete_key(&mut self, key: &str) -> io::Result<()>;

    /// Runs a program to completion without a window and returns its exit code.
    fn run(&mut self, program: &Path, args: &[String]) -> io::Result<i32>;
    /// Path of VS Code's `code` command line launcher, if it is installed.
    fn find_vscode(&self) -> Option<PathBuf>;
}

/// "The file is in use" (sharing violation, lock violation, access denied): worth a delete-at-reboot.
pub fn is_in_use(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33)) || e.kind() == io::ErrorKind::PermissionDenied
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use crate::plan::path_key;
    use std::collections::{BTreeMap, BTreeSet};

    /// Everything a test compares before and after (not the configuration, the command log or the counters).
    #[derive(Clone, Debug, PartialEq, Eq, Default)]
    pub struct State {
        pub files: BTreeMap<String, Vec<u8>>,
        pub dirs: BTreeSet<String>,
        pub registry: BTreeMap<String, Vec<(String, RegValue)>>,
        pub registered: BTreeSet<String>,
        pub pending_delete: Vec<String>,
    }

    #[derive(Clone, Default)]
    pub struct FakeBackend {
        pub state: State,
        /// Files that are mapped into a running program: they cannot be overwritten or deleted, but they can be renamed.
        pub locked: BTreeSet<String>,
        /// Every mutating call increments this; `fail_at = Some(n)` makes the n-th one fail (and have no effect).
        pub ops: usize,
        pub fail_at: Option<usize>,
        pub commands: Vec<String>,
        /// Keys of the files written, and "remove <key>" / "run <program> <args>" in the order they happened.
        pub write_log: Vec<String>,
        pub events: Vec<String>,
        pub vscode: Option<PathBuf>,
        pub register_exit: i32,
        pub unregister_exit: i32,
        pub code_exit: i32,
        pub deny_schedule: bool,
        /// Files that are not only mapped but held open without delete sharing: they cannot even be renamed.
        pub no_rename: BTreeSet<String>,
        /// Entries of the pending-rename list that did not come from this installer (other products, older setups).
        pub foreign_pending: Vec<String>,
        /// How often the pending-rename list was read (the pre-flight check must stay off HKLM for temp-folder installs).
        pub pending_reads: std::cell::Cell<usize>,
        /// Paths that are reparse points (a junction, a symbolic link). The fake does not resolve them: what is "below"
        /// such a path is what the link would expose, and `check_plain` must refuse to go there.
        pub links: BTreeSet<String>,
        /// Every `check_plain` question: (root, path).
        pub checks: std::cell::RefCell<Vec<(String, String)>>,
    }

    fn k(p: &Path) -> String {
        path_key(p)
    }

    fn parent_key(p: &Path) -> Option<String> {
        p.parent().map(k)
    }

    impl FakeBackend {
        pub fn new() -> FakeBackend {
            FakeBackend {
                vscode: Some(PathBuf::from(r"C:\Fake\VS Code\bin\code.cmd")),
                ..Default::default()
            }
        }

        /// A machine with a drive and the usual folders.
        pub fn with_dirs(dirs: &[&str]) -> FakeBackend {
            let mut b = FakeBackend::new();
            for d in dirs {
                b.state.dirs.insert(k(Path::new(d)));
            }
            b
        }

        pub fn put(&mut self, path: &str, data: &[u8]) {
            self.state.files.insert(k(Path::new(path)), data.to_vec());
        }

        pub fn lock(&mut self, path: &str) {
            self.locked.insert(k(Path::new(path)));
        }

        /// Marks `path` as a junction / symbolic link.
        pub fn link(&mut self, path: &str) {
            self.links.insert(k(Path::new(path)));
        }

        /// Mapped into a running program AND not renameable (opened without FILE_SHARE_DELETE).
        pub fn lock_hard(&mut self, path: &str) {
            self.lock(path);
            self.no_rename.insert(k(Path::new(path)));
        }

        /// A restart: Windows works through the scheduled deletions in order (a folder only goes while it is empty),
        /// and nothing is locked any more.
        pub fn reboot(&mut self) {
            let pending = std::mem::take(&mut self.state.pending_delete);
            for key in pending {
                if self.state.files.remove(&key).is_some() {
                    continue;
                }
                let below = |c: &String| c.starts_with(&format!("{key}\\"));
                let empty =
                    !self.state.files.keys().any(below) && !self.state.dirs.iter().any(below);
                if empty {
                    self.state.dirs.remove(&key);
                }
            }
            self.locked.clear();
            self.no_rename.clear();
            self.foreign_pending.clear();
        }

        pub fn file(&self, path: &str) -> Option<&Vec<u8>> {
            self.state.files.get(&k(Path::new(path)))
        }

        fn tick(&mut self) -> io::Result<()> {
            self.ops += 1;
            if self.fail_at == Some(self.ops) {
                return Err(io::Error::other("injected failure"));
            }
            Ok(())
        }

        fn parent_exists(&self, p: &Path) -> bool {
            parent_key(p).is_some_and(|d| self.state.dirs.contains(&d))
        }

        fn in_use() -> io::Error {
            io::Error::from_raw_os_error(32)
        }
    }

    impl Backend for FakeBackend {
        fn exists(&self, path: &Path) -> bool {
            let key = k(path);
            self.state.files.contains_key(&key) || self.state.dirs.contains(&key)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.state.dirs.contains(&k(path))
        }

        fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.state
                .files
                .get(&k(path))
                .cloned()
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }

        fn dir_is_empty(&self, path: &Path) -> io::Result<bool> {
            let key = k(path);
            if !self.state.dirs.contains(&key) {
                return Err(io::ErrorKind::NotFound.into());
            }
            let below = |c: &String| c.starts_with(&format!("{key}\\"));
            Ok(!self.state.files.keys().any(below) && !self.state.dirs.iter().any(below))
        }

        fn create_dir(&mut self, path: &Path) -> io::Result<()> {
            self.tick()?;
            if self.exists(path) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            if !self.parent_exists(path) {
                return Err(io::ErrorKind::NotFound.into());
            }
            self.state.dirs.insert(k(path));
            Ok(())
        }

        fn write_file(&mut self, path: &Path, data: &[u8]) -> io::Result<()> {
            self.tick()?;
            if !self.parent_exists(path) || self.state.dirs.contains(&k(path)) {
                return Err(io::ErrorKind::NotFound.into());
            }
            if self.locked.contains(&k(path)) {
                return Err(Self::in_use());
            }
            self.state.files.insert(k(path), data.to_vec());
            self.write_log.push(k(path));
            Ok(())
        }

        fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
            self.tick()?;
            let (from_key, to_key) = (k(from), k(to));
            if !self.state.files.contains_key(&from_key) {
                return Err(io::ErrorKind::NotFound.into());
            }
            if self.no_rename.contains(&from_key) {
                return Err(Self::in_use());
            }
            if self.exists(to) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            if !self.parent_exists(to) {
                return Err(io::ErrorKind::NotFound.into());
            }
            let data = self.state.files.remove(&from_key).unwrap_or_default();
            self.state.files.insert(to_key.clone(), data);
            if self.locked.remove(&from_key) {
                self.locked.insert(to_key);
            }
            Ok(())
        }

        fn remove_file(&mut self, path: &Path) -> io::Result<()> {
            self.tick()?;
            let key = k(path);
            if !self.state.files.contains_key(&key) {
                return Err(io::ErrorKind::NotFound.into());
            }
            if self.locked.contains(&key) {
                return Err(Self::in_use());
            }
            self.state.files.remove(&key);
            self.events.push(format!("remove {key}"));
            Ok(())
        }

        fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
            self.tick()?;
            if !self.state.dirs.contains(&k(path)) {
                return Err(io::ErrorKind::NotFound.into());
            }
            if !self.dir_is_empty(path)? {
                return Err(io::Error::from_raw_os_error(145)); // ERROR_DIR_NOT_EMPTY
            }
            self.state.dirs.remove(&k(path));
            Ok(())
        }

        fn check_plain(&self, root: &Path, path: &Path) -> Result<(), String> {
            let (root_key, path_key) = (k(root), k(path));
            self.checks
                .borrow_mut()
                .push((root_key.clone(), path_key.clone()));
            let Some(below) = crate::plan::relative_below(root, path) else {
                return Err(format!(
                    "{} is not inside {}",
                    path.display(),
                    root.display()
                ));
            };
            let mut at = root_key;
            let mut steps = vec![at.clone()];
            for comp in below {
                at = format!("{at}\\{}", comp.to_lowercase());
                steps.push(at.clone());
            }
            for step in steps {
                if self.links.contains(&step) {
                    return Err(format!(
                        "{step} is a junction or symbolic link (a reparse point)"
                    ));
                }
                if !self.exists(Path::new(&step)) {
                    break; // nothing below a missing step exists
                }
            }
            Ok(())
        }

        fn schedule_delete_on_reboot(&mut self, path: &Path) -> io::Result<()> {
            self.tick()?;
            if self.deny_schedule {
                return Err(io::Error::from_raw_os_error(5));
            }
            self.state.pending_delete.push(k(path));
            Ok(())
        }

        fn pending_file_renames(&self) -> io::Result<Vec<String>> {
            self.pending_reads.set(self.pending_reads.get() + 1);
            Ok(self
                .state
                .pending_delete
                .iter()
                .chain(&self.foreign_pending)
                .cloned()
                .collect())
        }

        fn reg_read_key(&self, key: &str) -> io::Result<Option<Vec<(String, RegValue)>>> {
            Ok(self.state.registry.get(&key.to_lowercase()).cloned())
        }

        fn reg_write_key(&mut self, key: &str, values: &[(String, RegValue)]) -> io::Result<()> {
            self.tick()?;
            let entry = self.state.registry.entry(key.to_lowercase()).or_default();
            for (name, value) in values {
                match entry.iter_mut().find(|(n, _)| n == name) {
                    Some(slot) => slot.1 = value.clone(),
                    None => entry.push((name.clone(), value.clone())),
                }
            }
            Ok(())
        }

        fn reg_delete_key(&mut self, key: &str) -> io::Result<()> {
            self.tick()?;
            self.state.registry.remove(&key.to_lowercase());
            Ok(())
        }

        fn run(&mut self, program: &Path, args: &[String]) -> io::Result<i32> {
            self.tick()?;
            self.commands
                .push(format!("{} {}", program.display(), args.join(" ")));
            let name = program
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            self.events
                .push(format!("run {name} {}", args.join(" ").to_lowercase()));
            if name == "regsvr32.exe" {
                let unregister = args.iter().any(|a| a == "/u");
                let dll = args
                    .iter()
                    .rfind(|a| !a.starts_with('/'))
                    .map(|a| k(Path::new(a)))
                    .unwrap_or_default();
                let exit = if unregister {
                    self.unregister_exit
                } else {
                    self.register_exit
                };
                if exit == 0 {
                    if unregister {
                        self.state.registered.remove(&dll);
                    } else {
                        self.state.registered.insert(dll);
                    }
                }
                return Ok(exit);
            }
            Ok(self.code_exit)
        }

        fn find_vscode(&self) -> Option<PathBuf> {
            self.vscode.clone()
        }
    }
}
