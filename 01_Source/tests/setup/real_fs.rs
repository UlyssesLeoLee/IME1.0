//! Integration tests against the REAL filesystem, in a temporary folder only, and only for plans that have no registry
//! and no process step (`--no-register --no-arp --no-vscode`). Nothing here registers a COM server, writes HKLM/HKCU,
//! runs regsvr32 or `code`, or touches Program Files; the elevation decision for these plans is asserted first, and the
//! tests stand down if the test runner itself is elevated.
//!
//! These belong with the other integration tests under 01_Source/tests/ (see docs/INSTALLER.md); they live here because
//! the installer crate is self-contained and tests/rust is wired to pime-core only.

use pime_setup::backend::Backend;
use pime_setup::exec::{Inputs, execute};
use pime_setup::payload::{self, Payload};
use pime_setup::plan::{
    InstallOptions, MANIFEST_NAME, Manifest, UninstallOptions, VscodeMode, install_plan,
    uninstall_plan,
};
use pime_setup::real::{RealBackend, is_elevated, system_paths};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_FLAGS, LoadLibraryExW};
use windows::core::PCWSTR;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("pime-setup-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // only ever a folder this test created below the temp directory
        if self.0.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn entries(version: &str) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (payload::DLL, format!("dll-{version}").into_bytes()),
        (payload::CORE, format!("core-{version}").into_bytes()),
        (payload::LICENSE, b"MIT".to_vec()),
        (
            payload::META,
            format!(r#"{{"version":"{version}","extension_id":"pub.name"}}"#).into_bytes(),
        ),
        (payload::VSIX, format!("vsix-{version}").into_bytes()),
    ]
}

fn overlay(version: &str) -> Vec<u8> {
    let e = entries(version);
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, d)| (*n, d.as_slice())).collect();
    payload::pack(&refs)
}

/// The test binary of `pime-setup` plus a payload: what scripts/build-installer.js produces.
fn installer_exe(dir: &Path, version: &str) -> PathBuf {
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_pime-setup")).unwrap();
    bytes.extend(overlay(version));
    let path = dir.join(format!("setup-{version}.exe"));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn offline_options(version: &str) -> InstallOptions {
    let payload = Payload::from_exe(&overlay(version)).unwrap().unwrap();
    InstallOptions {
        register: false,
        arp: false,
        vscode: VscodeMode::Skip,
        release: payload.release().unwrap(),
        system: system_paths().unwrap(),
    }
}

#[test]
fn st_035_real_folder_install_upgrade_and_uninstall_with_spaces_and_non_ascii() {
    let tmp = TempDir::new("lib");
    let prefix = tmp.0.join("Programming IME 日本語");
    let mut backend = RealBackend::without_reboot_schedule();
    let run = |backend: &mut RealBackend, version: &str| {
        let options = offline_options(version);
        let plan = install_plan(&prefix, &options).unwrap();
        assert!(
            !plan.requires_admin,
            "a plain temp-folder install must not need administrator rights"
        );
        let payload = Payload::from_exe(&overlay(version)).unwrap().unwrap();
        execute(
            &plan,
            &Inputs {
                payload: &payload,
                self_exe: b"self-exe",
            },
            backend,
            &mut |_| {},
        )
        .unwrap()
    };

    let out = run(&mut backend, "0.1.0");
    assert!(out.warnings.is_empty() && !out.restart_needed);
    assert_eq!(
        std::fs::read(prefix.join("pime_tsf.dll")).unwrap(),
        b"dll-0.1.0"
    );
    assert_eq!(
        std::fs::read(prefix.join("uninstall.exe")).unwrap(),
        b"self-exe"
    );
    let manifest = Manifest::parse(
        &std::fs::read_to_string(prefix.join(MANIFEST_NAME)).unwrap(),
        &prefix,
    )
    .unwrap();
    assert_eq!(manifest.files.len(), 6);
    assert!(
        manifest.files.iter().all(|f| f.is_file()),
        "the manifest lists exactly files that exist"
    );
    assert_eq!(
        manifest.directories,
        std::slice::from_ref(&prefix),
        "the install folder was created by the installer and is recorded; the pre-existing temp folder is not"
    );
    assert!(
        manifest.registry_keys.is_empty()
            && manifest.text_service_dlls.is_empty()
            && manifest.vscode_extension.is_none()
    );

    // re-install of the same version and an upgrade leave no side files behind
    run(&mut backend, "0.1.0");
    run(&mut backend, "0.2.0");
    assert_eq!(
        std::fs::read(prefix.join("pime_tsf.dll")).unwrap(),
        b"dll-0.2.0"
    );
    let names: Vec<String> = std::fs::read_dir(&prefix)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.contains(".pime-")), "{names:?}");
    assert_eq!(names.len(), 6, "{names:?}");

    // a file the user put there survives the uninstall; the manifest's files do not
    std::fs::write(prefix.join("my notes.txt"), "keep me").unwrap();
    let manifest = Manifest::parse(
        &std::fs::read_to_string(prefix.join(MANIFEST_NAME)).unwrap(),
        &prefix,
    )
    .unwrap();
    let plan = uninstall_plan(
        &manifest,
        &UninstallOptions {
            vscode: VscodeMode::Skip,
            system: system_paths().unwrap(),
        },
    )
    .unwrap();
    assert!(!plan.requires_admin);
    let none = Payload {
        entries: Vec::new(),
    };
    let out = execute(
        &plan,
        &Inputs {
            payload: &none,
            self_exe: &[],
        },
        &mut backend,
        &mut |_| {},
    )
    .unwrap();
    assert!(out.warnings.iter().any(|w| w.contains("was kept")));
    assert_eq!(
        std::fs::read(prefix.join("my notes.txt")).unwrap(),
        b"keep me"
    );
    assert_eq!(std::fs::read_dir(&prefix).unwrap().count(), 1);

    // a folder the installer itself created (here: with a missing parent) goes too; one that pre-existed would stay
    let deep = tmp.0.join("new parent").join("Programming IME");
    let options = offline_options("0.2.0");
    let plan = install_plan(&deep, &options).unwrap();
    let payload = Payload::from_exe(&overlay("0.2.0")).unwrap().unwrap();
    execute(
        &plan,
        &Inputs {
            payload: &payload,
            self_exe: b"self-exe",
        },
        &mut backend,
        &mut |_| {},
    )
    .unwrap();
    let manifest = Manifest::parse(
        &std::fs::read_to_string(deep.join(MANIFEST_NAME)).unwrap(),
        &deep,
    )
    .unwrap();
    assert_eq!(
        manifest.directories,
        [tmp.0.join("new parent"), deep.clone()]
    );
    let plan = uninstall_plan(
        &manifest,
        &UninstallOptions {
            vscode: VscodeMode::Skip,
            system: system_paths().unwrap(),
        },
    )
    .unwrap();
    execute(
        &plan,
        &Inputs {
            payload: &none,
            self_exe: &[],
        },
        &mut backend,
        &mut |_| {},
    )
    .unwrap();
    assert!(!deep.exists() && !tmp.0.join("new parent").exists());
}

fn setup(exe: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(exe).args(args).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn st_035_dry_run_prints_the_plan_and_changes_nothing() {
    let tmp = TempDir::new("dry");
    let exe = installer_exe(&tmp.0, "0.3.0");
    let prefix = tmp.0.join("would-be-installed-here");
    let (code, text) = setup(&exe, &["--dry-run", "--prefix", prefix.to_str().unwrap()]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("Programming IME 0.3.0 - install plan (dry run: nothing is changed)"),
        "{text}"
    );
    assert!(
        text.contains("payload: 4 files, version 0.3.0, SHA-256 of every file verified"),
        "{text}"
    );
    assert!(
        text.contains("regsvr32.exe /s") && text.contains("Add/Remove Programs"),
        "{text}"
    );
    assert!(!prefix.exists(), "a dry run creates nothing");
    // and nothing was written next to the exe either (no log, no temp copy)
    let names: Vec<_> = std::fs::read_dir(&tmp.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["setup-0.3.0.exe"]);

    // --no-vscode / --no-register are visible in the plan
    let (_, text) = setup(
        &exe,
        &[
            "--dry-run",
            "--prefix",
            prefix.to_str().unwrap(),
            "--no-register",
            "--no-arp",
            "--no-vscode",
        ],
    );
    assert!(
        !text.contains("regsvr32")
            && !text.contains("Add/Remove")
            && text.contains("administrator rights needed: no"),
        "{text}"
    );
}

#[test]
fn st_035_unsafe_prefixes_and_bad_arguments_exit_with_2_before_anything_happens() {
    let tmp = TempDir::new("refuse");
    let exe = installer_exe(&tmp.0, "0.3.0");
    for prefix in [
        r"C:\Windows",
        r"C:\",
        r"C:\Program Files\..\Windows",
        "relative",
        r"\\server\share\x",
    ] {
        // --dry-run only: were the check ever to regress, a real run here would ask for administrator rights and write
        // into the Windows folder. The refusal happens while the plan is built, which a dry run does as well.
        let (code, text) = setup(&exe, &["--dry-run", "--prefix", prefix]);
        assert_eq!(code, 2, "{prefix}: {text}");
        assert!(
            text.contains("refusing this install folder"),
            "{prefix}: {text}"
        );
    }
    let (code, text) = setup(&exe, &["--bogus"]);
    assert_eq!(code, 2);
    assert!(text.contains("unknown option --bogus") && text.contains("Usage:"));
    let (code, text) = setup(&exe, &["--help"]);
    assert_eq!(code, 0);
    assert!(text.contains("--dry-run") && text.contains("--uninstall"));
}

#[test]
fn st_035_the_bare_stub_refuses_to_install_but_says_why() {
    let tmp = TempDir::new("stub");
    let stub = tmp.0.join("stub.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_pime-setup"), &stub).unwrap();
    let prefix = tmp.0.join("p");
    let (code, text) = setup(
        &stub,
        &[
            "--silent",
            "--no-register",
            "--no-arp",
            "--no-vscode",
            "--prefix",
            prefix.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1);
    assert!(text.contains("carries no payload"), "{text}");
    assert!(!prefix.exists());
}

#[test]
fn st_035_the_real_exe_installs_and_uninstalls_in_a_temp_folder_with_all_three_no_flags() {
    if is_elevated() {
        eprintln!("skipped: do not run the installer tests from an elevated shell");
        return;
    }
    let tmp = TempDir::new("exe");
    let exe = installer_exe(&tmp.0, "0.3.0");
    let prefix = tmp.0.join("Programming IME");
    let prefix_arg = prefix.to_str().unwrap();
    let flags = [
        "--silent",
        "--no-register",
        "--no-arp",
        "--no-vscode",
        "--prefix",
        prefix_arg,
    ];

    let (code, text) = setup(&exe, &flags);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("DONE: Programming IME 0.3.0 installed"),
        "{text}"
    );
    assert!(
        !text.contains("UAC"),
        "a temp-folder install never asks for administrator rights: {text}"
    );
    assert_eq!(
        std::fs::read(prefix.join("pime-core.exe")).unwrap(),
        b"core-0.3.0"
    );
    assert_eq!(
        std::fs::read(prefix.join("uninstall.exe")).unwrap(),
        std::fs::read(&exe).unwrap(),
        "the uninstaller is a copy of the setup"
    );

    // installing again is a no-op that still succeeds
    let (code, text) = setup(&exe, &flags);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("unchanged:"), "{text}");

    let (code, text) = setup(
        &exe,
        &[
            "--uninstall",
            "--silent",
            "--no-vscode",
            "--prefix",
            prefix_arg,
        ],
    );
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("DONE: Programming IME 0.3.0 uninstalled"),
        "{text}"
    );
    assert!(!prefix.exists(), "everything the installer created is gone");

    // nothing left to uninstall: a clear error, not a guess
    let (code, text) = setup(
        &exe,
        &[
            "--uninstall",
            "--silent",
            "--no-vscode",
            "--prefix",
            prefix_arg,
        ],
    );
    assert_eq!(code, 1);
    assert!(text.contains("no install manifest"), "{text}");
}

// ---------------------------------------------------------------------------------------------------------------------
// Review finding 1: a file that programs have mapped cannot be deleted, but it can be renamed, and only the renamed
// file may be handed to Windows for deletion at the next restart.
// ---------------------------------------------------------------------------------------------------------------------

fn system32() -> PathBuf {
    system_paths().unwrap().system_dir
}

/// A DLL mapped into this process, like pime_tsf.dll in the programs that used the input method.
struct LoadedDll(HMODULE);

impl LoadedDll {
    fn load(path: &Path) -> LoadedDll {
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let module = unsafe { LoadLibraryExW(PCWSTR(name.as_ptr()), HANDLE::default(), LOAD_LIBRARY_FLAGS(0)) }
            .expect("a copy of a system DLL in a temp folder can be loaded (endpoint protection may block this)");
        LoadedDll(module)
    }
}

impl Drop for LoadedDll {
    fn drop(&mut self) {
        // SAFETY: the handle came from LoadLibraryExW and is freed once.
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn st_035_review_1_the_real_rename_never_replaces_an_existing_file() {
    let tmp = TempDir::new("rename");
    let (a, b) = (tmp.0.join("a.txt"), tmp.0.join("b.txt"));
    std::fs::write(&a, "a").unwrap();
    std::fs::write(&b, "b").unwrap();
    let mut backend = RealBackend::without_reboot_schedule();
    assert!(
        backend.rename(&a, &b).is_err(),
        "an existing destination is an error"
    );
    assert_eq!(std::fs::read(&a).unwrap(), b"a");
    assert_eq!(std::fs::read(&b).unwrap(), b"b");
    let c = tmp.0.join("c.txt");
    backend.rename(&a, &c).unwrap();
    assert!(!a.exists() && std::fs::read(&c).unwrap() == b"a");
}

#[test]
fn st_035_review_1_a_mapped_dll_is_renamed_aside_and_its_original_path_is_never_scheduled() {
    let tmp = TempDir::new("mapped");
    let prefix = tmp.0.join("Programming IME");
    std::fs::create_dir_all(&prefix).unwrap();
    let dll = prefix.join("pime_tsf.dll");
    std::fs::copy(system32().join("version.dll"), &dll).unwrap();
    let mapped = LoadedDll::load(&dll);
    let mut backend = RealBackend::without_reboot_schedule();
    // control: this is what a mapped image does to a delete
    assert!(
        std::fs::remove_file(&dll).is_err(),
        "a mapped DLL cannot be deleted"
    );

    let manifest = Manifest {
        version: "0.0.0".into(),
        prefix: prefix.clone(),
        files: vec![dll.clone()],
        directories: Vec::new(),
        registry_keys: Vec::new(),
        text_service_dlls: Vec::new(),
        vscode_extension: None,
    };
    let plan = uninstall_plan(
        &manifest,
        &UninstallOptions {
            vscode: VscodeMode::Skip,
            system: system_paths().unwrap(),
        },
    )
    .unwrap();
    let none = Payload {
        entries: Vec::new(),
    };
    let out = execute(
        &plan,
        &Inputs {
            payload: &none,
            self_exe: &[],
        },
        &mut backend,
        &mut |_| {},
    )
    .unwrap();

    // the original name is free for a new install; the old image waits under a name nothing else will use
    assert!(!dll.exists(), "{:?}", names_in(&prefix));
    let aside: Vec<String> = names_in(&prefix);
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert!(aside[0].starts_with("pime_tsf.dll.pime-del-"), "{aside:?}");
    // this backend cannot schedule (it never touches HKLM): that is reported, naming the renamed file
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains(&aside[0]) && w.contains("could not be scheduled")),
        "{:?}",
        out.warnings
    );
    assert!(!out.restart_needed);
    drop(mapped);
}

// ---------------------------------------------------------------------------------------------------------------------
// Review finding 2: the system folders come from the Windows APIs, never from environment variables.
// ---------------------------------------------------------------------------------------------------------------------

fn describe(p: &pime_setup::plan::SystemPaths) -> String {
    format!(
        "RESOLVED|{}|{}|{}|{}|{:?}|{:?}|{:?}",
        p.system_root.display(),
        p.system_dir.display(),
        p.regsvr32().display(),
        p.program_files.display(),
        p.program_files_x86,
        p.program_data,
        p.user_profile
    )
}

const CHILD_GATE: &str = "PIME_SETUP_TEST_RESOLVE_CHILD";

/// Runs itself twice: once as the test, once (gated by an environment variable) as a child that was started with bogus
/// `SystemRoot` / `ProgramFiles` values. No `set_var` in this process, so no race with the other tests.
#[test]
fn st_035_review_2_the_system_paths_do_not_come_from_environment_variables() {
    if std::env::var_os(CHILD_GATE).is_some() {
        println!("{}", describe(&system_paths().unwrap()));
        return;
    }
    let ours = system_paths().unwrap();
    let evil = r"C:\pime-evil";
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "st_035_review_2_the_system_paths_do_not_come_from_environment_variables",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_GATE, "1")
        .env("SystemRoot", format!(r"{evil}\root"))
        .env("windir", format!(r"{evil}\root"))
        .env("ProgramFiles", format!(r"{evil}\pf"))
        .env("ProgramW6432", format!(r"{evil}\pf"))
        .env("ProgramFiles(x86)", format!(r"{evil}\pf86"))
        .env("ProgramData", format!(r"{evil}\pd"))
        .env("USERPROFILE", format!(r"{evil}\user"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    // (the harness prints "test <name> ... " in front of the first line of captured output)
    let line = text
        .lines()
        .find_map(|l| l.find("RESOLVED|").map(|at| &l[at..]))
        .unwrap_or_else(|| {
            panic!(
                "the child printed no result: {text}{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    assert_eq!(
        line,
        describe(&ours),
        "bogus environment variables changed the answer"
    );
    assert!(!line.contains("pime-evil"), "{line}");
    // and what it answered is the real thing: absolute, existing, regsvr32 in the folder GetSystemDirectoryW names
    assert!(ours.regsvr32().is_file(), "{}", ours.regsvr32().display());
    assert!(ours.system_dir.starts_with(&ours.system_root));
    assert!(ours.program_files.is_dir() && ours.program_files.is_absolute());
}
