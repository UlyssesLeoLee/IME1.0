//! The file part of the settings store: atomic writes and `.bak` recovery (NFR-401, UT-073). Port of
//! `writeFileAtomic` / `readJsonRecovering` of `core/settings.js`.
//!
//! This is the one module of the core that touches the file system, and it does so only through the [`Fs`] trait with
//! a path the CALLER names: nothing in the JSON Lines protocol reaches it, so a peer on the pipe cannot ask the core
//! to write a file. The bridge keeps its own file access until the core is given a fixed settings directory (see the
//! migration plan, 13.9). Tests use an in-memory [`Fs`] that fails on demand and, for the real thing, temporary
//! directories.
//!
//! A write is `temp file -> (previous file kept as .bak) -> rename`: a crash, or ENOSPC, before the rename leaves the
//! last good file untouched and the temp file is removed. Two differences from the reference, both stricter: the
//! temp file is flushed to disk before the rename (otherwise a power loss can persist the rename without the data), and
//! the `.bak` copy is only refreshed from a previous file that parses as JSON (the reference copies a corrupt file
//! over the last good backup, so a second failure would lose both).

use crate::json::{self, Json, Limits};
use crate::jsval::to_pretty_string;
use crate::settings_migrate::SettingsStore;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Settings files are small; anything bigger than this is not a settings file.
const FILE_LIMITS: Limits = Limits {
    max_bytes: 4 << 20,
    max_depth: 64,
};

/// The file operations the store needs (the reference's `fsImpl`).
pub trait Fs {
    /// Creates or truncates `path` and makes the data durable before returning.
    fn write(&self, path: &Path, data: &[u8]) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Replaces `to` by `from`.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// A file that is not there is not an error.
    fn remove(&self, path: &Path) -> io::Result<()>;
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
}

/// The real file system.
pub struct StdFs;

impl Fs for StdFs {
    fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(data)?;
        file.sync_all()
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::copy(from, to).map(|_| ())
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        // on Windows a virus scanner or indexer can hold the destination for a moment (access denied / sharing
        // violation); a short bounded retry keeps that from failing a save, a real refusal still surfaces
        let mut delay = std::time::Duration::from_millis(10);
        for _ in 0..4 {
            match std::fs::rename(from, to) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy
                    ) =>
                {
                    std::thread::sleep(delay);
                    delay *= 2;
                }
                done => return done,
            }
        }
        std::fs::rename(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// `<file><suffix>` next to the file.
fn sibling(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn parses(fs: &dyn Fs, file: &Path) -> bool {
    fs.read_to_string(file)
        .is_ok_and(|text| json::parse(&text, FILE_LIMITS).is_ok())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `text` to `file` so that readers see either the old or the new content, never a partial file.
pub fn write_file_atomic(fs: &dyn Fs, file: &Path, text: &str) -> io::Result<()> {
    // process id for readability, a counter so that two writers of one process never share a temp file
    let tmp = sibling(
        file,
        &format!(
            ".tmp-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ),
    );
    let result = (|| {
        fs.write(&tmp, text.as_bytes())?;
        if fs.exists(file) && parses(fs, file) {
            fs.copy(file, &sibling(file, ".bak"))?;
        }
        fs.rename(&tmp, file)
    })();
    if result.is_err() {
        let _ = fs.remove(&tmp); // best effort: the error that matters is the first one
    }
    result
}

pub struct Recovered {
    /// `None` when neither the file nor its backup holds an acceptable document.
    pub value: Option<Json>,
    /// Whether the value came from the `.bak` file.
    pub recovered: bool,
}

/// Reads the settings file; a missing, corrupt or unacceptable file (`validate` says no) is replaced by the backup.
pub fn read_json_recovering(
    fs: &dyn Fs,
    file: &Path,
    validate: &dyn Fn(&Json) -> bool,
) -> Recovered {
    for (candidate, recovered) in [(file.to_path_buf(), false), (sibling(file, ".bak"), true)] {
        if let Ok(text) = fs.read_to_string(&candidate)
            && let Ok(value) = json::parse(&text, FILE_LIMITS)
            && validate(&value)
        {
            return Recovered {
                value: Some(value),
                recovered,
            };
        }
    }
    Recovered {
        value: None,
        recovered: false,
    }
}

/// A [`SettingsStore`] on one file, so that `migrate_store` keeps the previous file when anything fails.
pub struct FileStore<'a> {
    pub fs: &'a dyn Fs,
    pub path: PathBuf,
}

impl SettingsStore for FileStore<'_> {
    fn read(&mut self) -> Json {
        read_json_recovering(self.fs, &self.path, &|_| true)
            .value
            .unwrap_or(Json::Obj(Vec::new()))
    }

    fn write(&mut self, document: &Json) -> Result<(), String> {
        write_file_atomic(self.fs, &self.path, &to_pretty_string(document))
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// An in-memory file system that fails on one chosen operation.
    struct MemFs {
        files: RefCell<BTreeMap<PathBuf, String>>,
        fail_on: Option<&'static str>,
    }

    impl MemFs {
        fn with(initial: &[(&str, &str)], fail_on: Option<&'static str>) -> MemFs {
            MemFs {
                files: RefCell::new(
                    initial
                        .iter()
                        .map(|(p, t)| (PathBuf::from(p), (*t).to_string()))
                        .collect(),
                ),
                fail_on,
            }
        }
        fn get(&self, path: &str) -> Option<String> {
            self.files.borrow().get(Path::new(path)).cloned()
        }
        fn names(&self) -> Vec<String> {
            self.files
                .borrow()
                .keys()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        }
        fn check(&self, op: &str) -> io::Result<()> {
            if self.fail_on == Some(op) {
                Err(io::Error::other(format!("{op} failed (ENOSPC)")))
            } else {
                Ok(())
            }
        }
    }

    impl Fs for MemFs {
        fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
            self.check("write")?;
            self.files.borrow_mut().insert(
                path.to_path_buf(),
                String::from_utf8_lossy(data).into_owned(),
            );
            Ok(())
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.borrow().contains_key(path)
        }
        fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.check("copy")?;
            let text = self
                .files
                .borrow()
                .get(from)
                .cloned()
                .ok_or(io::ErrorKind::NotFound)?;
            self.files.borrow_mut().insert(to.to_path_buf(), text);
            Ok(())
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.check("rename")?;
            let text = self
                .files
                .borrow_mut()
                .remove(from)
                .ok_or(io::ErrorKind::NotFound)?;
            self.files.borrow_mut().insert(to.to_path_buf(), text);
            Ok(())
        }
        fn remove(&self, path: &Path) -> io::Result<()> {
            self.files.borrow_mut().remove(path);
            Ok(())
        }
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            self.files
                .borrow()
                .get(path)
                .cloned()
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }
    }

    const GOOD: &str = r#"{"schema_version":1,"x":"old"}"#;
    const NEW: &str = r#"{"schema_version":1,"x":"new"}"#;

    #[test]
    fn ut_073_a_failure_before_the_rename_leaves_the_last_valid_file_and_no_temp_file() {
        for fail_on in ["write", "copy", "rename"] {
            let fs = MemFs::with(&[("/s.json", GOOD)], Some(fail_on));
            let e = write_file_atomic(&fs, Path::new("/s.json"), NEW).unwrap_err();
            assert!(e.to_string().contains("failed"), "{fail_on}");
            assert_eq!(
                fs.get("/s.json").as_deref(),
                Some(GOOD),
                "{fail_on}: old file untouched"
            );
            assert!(
                fs.names().iter().all(|n| !n.contains(".tmp-")),
                "{fail_on}: temp file removed: {:?}",
                fs.names()
            );
        }
    }

    #[test]
    fn a_successful_write_keeps_the_previous_file_as_bak() {
        let fs = MemFs::with(&[("/s.json", GOOD)], None);
        write_file_atomic(&fs, Path::new("/s.json"), NEW).unwrap();
        assert_eq!(fs.get("/s.json").as_deref(), Some(NEW));
        assert_eq!(fs.get("/s.json.bak").as_deref(), Some(GOOD));
        assert_eq!(fs.names().len(), 2, "{:?}", fs.names());
        // the first write of a new file has nothing to back up
        let fresh = MemFs::with(&[], None);
        write_file_atomic(&fresh, Path::new("/n.json"), NEW).unwrap();
        assert_eq!(fresh.names(), ["/n.json"]);
    }

    #[test]
    fn ut_073_a_corrupt_file_is_recovered_from_the_bak() {
        let fs = MemFs::with(&[("/s.json", GOOD)], None);
        write_file_atomic(&fs, Path::new("/s.json"), NEW).unwrap();
        fs.files
            .borrow_mut()
            .insert(PathBuf::from("/s.json"), "{\"schema_ver".to_string()); // power loss mid-write
        let rec = read_json_recovering(&fs, Path::new("/s.json"), &|v| {
            v.get("schema_version").and_then(Json::as_i64) == Some(1)
        });
        assert!(rec.recovered);
        assert_eq!(
            rec.value.unwrap().get("x").and_then(Json::as_str),
            Some("old")
        );
        // nothing usable at all
        assert!(
            read_json_recovering(&fs, Path::new("/missing.json"), &|_| true)
                .value
                .is_none()
        );
        let both_bad = MemFs::with(&[("/s.json", "{"), ("/s.json.bak", "[")], None);
        let rec = read_json_recovering(&both_bad, Path::new("/s.json"), &|_| true);
        assert!(rec.value.is_none() && !rec.recovered);
        // a file that parses but is not acceptable is replaced by the backup too
        let wrong = MemFs::with(
            &[
                ("/s.json", r#"{"schema_version":9}"#),
                ("/s.json.bak", GOOD),
            ],
            None,
        );
        let rec = read_json_recovering(&wrong, Path::new("/s.json"), &|v| {
            v.get("schema_version").and_then(Json::as_i64) == Some(1)
        });
        assert!(rec.recovered);
    }

    #[test]
    fn a_corrupt_file_never_replaces_the_last_good_backup() {
        let fs = MemFs::with(&[("/s.json", "{\"schema_ver"), ("/s.json.bak", GOOD)], None);
        write_file_atomic(&fs, Path::new("/s.json"), NEW).unwrap();
        assert_eq!(fs.get("/s.json").as_deref(), Some(NEW));
        assert_eq!(
            fs.get("/s.json.bak").as_deref(),
            Some(GOOD),
            "the reference would have copied the corrupt file over it"
        );
    }

    #[test]
    fn a_file_store_migrates_atomically() {
        use crate::settings_migrate::migrate_store;
        let v0 = r#"{"provider":"anthropic","model":"m","theme":"x"}"#;
        let fs = MemFs::with(&[("/s.json", v0)], Some("rename"));
        let mut store = FileStore {
            fs: &fs,
            path: PathBuf::from("/s.json"),
        };
        assert!(migrate_store(&mut store, 0.0).is_err());
        assert_eq!(
            fs.get("/s.json").as_deref(),
            Some(v0),
            "the old configuration is intact"
        );
        let ok = MemFs::with(&[("/s.json", v0)], None);
        let mut store = FileStore {
            fs: &ok,
            path: PathBuf::from("/s.json"),
        };
        migrate_store(&mut store, 0.0).unwrap();
        let written = json::parse(&ok.get("/s.json").unwrap(), Limits::DEFAULT).unwrap();
        assert_eq!(
            written.get("schema_version").and_then(Json::as_i64),
            Some(1)
        );
        assert_eq!(written.get("theme").and_then(Json::as_str), Some("x"));
        assert_eq!(
            ok.get("/s.json.bak").as_deref(),
            Some(v0),
            "the v0.1 document stays recoverable"
        );
        // a fresh install reads as an empty document
        let none = MemFs::with(&[], None);
        let mut store = FileStore {
            fs: &none,
            path: PathBuf::from("/s.json"),
        };
        assert_eq!(store.read(), Json::Obj(Vec::new()));
    }
}
