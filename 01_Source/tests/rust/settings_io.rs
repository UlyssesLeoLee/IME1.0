//! The atomic settings file on the REAL file system (UT-073, NFR-401): write, `.bak` recovery, a failing write that
//! leaves the last good file, concurrent writers, and a migration through a file. Everything happens in a directory
//! below the build's own temp folder (`CARGO_TARGET_TMPDIR`); nothing outside of it is touched. The fault injection
//! with an in-memory file system is in the unit tests of `settings_io.rs`.

use pime_core::json::{Json, Limits, parse};
use pime_core::settings_io::{FileStore, StdFs, read_json_recovering, write_file_atomic};
use pime_core::settings_migrate::{SettingsStore, migrate_store};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "settings-io-{}-{name}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

const GOOD: &str = r#"{"schema_version":1,"x":"old"}"#;
const NEW: &str = r#"{"schema_version":1,"x":"new"}"#;

#[test]
fn a_write_replaces_the_file_keeps_the_previous_one_as_bak_and_leaves_no_temp_file() {
    let dir = scratch("write");
    let file = dir.join("settings.json");
    write_file_atomic(&StdFs, &file, GOOD).unwrap();
    assert_eq!(
        names(&dir),
        ["settings.json"],
        "nothing to back up on the first write"
    );
    write_file_atomic(&StdFs, &file, NEW).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), NEW);
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.bak")).unwrap(),
        GOOD
    );
    assert_eq!(names(&dir), ["settings.json", "settings.json.bak"]);
    // big files and non-ASCII text survive unchanged
    let big = format!("{{\"text\":\"{}\"}}", "漢字é😀".repeat(100_000));
    write_file_atomic(&StdFs, &file, &big).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), big);
}

#[test]
fn a_failing_write_leaves_the_last_good_file_and_removes_its_temp_file() {
    let dir = scratch("fail");
    let file = dir.join("settings.json");
    write_file_atomic(&StdFs, &file, GOOD).unwrap();
    // the target's folder is gone: the temp file cannot even be created
    let orphan = dir.join("missing-folder").join("settings.json");
    assert!(write_file_atomic(&StdFs, &orphan, NEW).is_err());
    assert_eq!(names(&dir), ["settings.json"]);
    // the target is a directory: the temp file is written, the rename fails, the temp file is removed again
    let blocked = dir.join("blocked.json");
    std::fs::create_dir(&blocked).unwrap();
    assert!(write_file_atomic(&StdFs, &blocked, NEW).is_err());
    assert_eq!(
        names(&dir),
        ["blocked.json", "settings.json"],
        "no .tmp- file is left behind"
    );
    // the good file was never touched
    assert_eq!(std::fs::read_to_string(&file).unwrap(), GOOD);
}

#[test]
fn a_corrupt_file_is_recovered_from_the_bak_and_the_bak_survives_the_next_write() {
    let dir = scratch("recover");
    let file = dir.join("settings.json");
    write_file_atomic(&StdFs, &file, GOOD).unwrap();
    write_file_atomic(&StdFs, &file, NEW).unwrap();
    std::fs::write(&file, "{\"schema_ver").unwrap(); // power loss in the middle of a write
    let valid = |v: &Json| v.get("schema_version").and_then(Json::as_i64) == Some(1);
    let rec = read_json_recovering(&StdFs, &file, &valid);
    assert!(rec.recovered);
    assert_eq!(
        rec.value.unwrap().get("x").and_then(Json::as_str),
        Some("old")
    );
    // saving again must not copy the corrupt file over the good backup
    write_file_atomic(&StdFs, &file, NEW).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.bak")).unwrap(),
        GOOD
    );
    let rec = read_json_recovering(&StdFs, &file, &valid);
    assert!(!rec.recovered);
    assert_eq!(
        rec.value.unwrap().get("x").and_then(Json::as_str),
        Some("new")
    );
    assert!(
        read_json_recovering(&StdFs, &dir.join("nothing.json"), &valid)
            .value
            .is_none()
    );
}

#[test]
fn writers_in_parallel_never_expose_a_partial_file() {
    let dir = scratch("parallel");
    let file = dir.join("settings.json");
    write_file_atomic(&StdFs, &file, GOOD).unwrap();
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let file = file.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    let text = format!(
                        "{{\"schema_version\":1,\"writer\":{w},\"i\":{i},\"pad\":\"{}\"}}",
                        "x".repeat(20_000)
                    );
                    // a writer may lose the race for the rename on Windows (sharing violation): that is an error, never a torn file
                    let _ = write_file_atomic(&StdFs, &file, &text);
                    if let Ok(read) = std::fs::read_to_string(&file) {
                        assert!(
                            parse(
                                &read,
                                Limits {
                                    max_bytes: 1 << 20,
                                    max_depth: 8
                                }
                            )
                            .is_ok(),
                            "a torn settings file was visible"
                        );
                    }
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    let rec = read_json_recovering(&StdFs, &file, &|_| true);
    assert!(rec.value.is_some() && !rec.recovered);
    assert!(
        names(&dir).iter().all(|n| !n.contains(".tmp-")),
        "no temp file is left behind: {:?}",
        names(&dir)
    );
}

#[test]
fn a_migration_through_a_file_keeps_the_old_file_when_it_cannot_write() {
    let v0 = r#"{"provider":"openai-compatible","model":"m1","effort":"max","userRules":[{"id":"mine"}]}"#;
    let dir = scratch("migrate");
    let file = dir.join("settings.json");
    std::fs::write(&file, v0).unwrap();
    let mut store = FileStore {
        fs: &StdFs,
        path: file.clone(),
    };
    let done = migrate_store(&mut store, 1_793_361_600_000.0).unwrap();
    assert_eq!(done.conflicts, Json::Arr(vec![]));
    let written = parse(&std::fs::read_to_string(&file).unwrap(), Limits::DEFAULT).unwrap();
    assert_eq!(
        written.get("schema_version").and_then(Json::as_i64),
        Some(1)
    );
    assert_eq!(
        written
            .get("scenarios")
            .unwrap()
            .get("realtime")
            .unwrap()
            .get("effort")
            .and_then(Json::as_str),
        Some("max")
    );
    assert_eq!(
        written.get("userRules").unwrap().to_json_string(),
        r#"[{"id":"mine"}]"#
    );
    assert!(written.get("model").is_none());
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.bak")).unwrap(),
        v0,
        "the v0.1 file stays recoverable"
    );
    // a store on a path that cannot be written keeps whatever is there and says so
    let blocked = dir.join("blocked.json");
    std::fs::create_dir(&blocked).unwrap();
    let mut store = FileStore {
        fs: &StdFs,
        path: blocked,
    };
    let e = migrate_store(&mut store, 0.0).err().unwrap();
    assert_eq!(e.code, "SETTINGS_INVALID");
    assert!(e.message.contains("previous settings are intact"));
    assert_eq!(
        store.read(),
        Json::Obj(vec![]),
        "an unreadable store reads as a fresh install"
    );
}
