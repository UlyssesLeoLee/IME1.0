//! Review finding 3, against the REAL file system: junctions and symbolic links inside the install folder are never
//! followed. A separate test program on purpose: making a junction is the one thing endpoint protection on some machines
//! blocks (and then holds up for a minute and a half), which must not disturb the tests that start the installer exe.
//! Temp folder only; no registry, no regsvr32 (a plan with a DLL to unregister is only tested against the fake backend).
//! Where this machine does not let a test make a junction, those cases print `SKIPPED` and stand down.

mod common;

use common::{TempDir, names_in, offline_options, overlay};
use pime_setup::exec::{Inputs, execute};
use pime_setup::payload::Payload;
use pime_setup::plan::{Manifest, UninstallOptions, VscodeMode, install_plan, uninstall_plan};
use pime_setup::real::{RealBackend, plain_final, plain_path_check, system_paths};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::PCWSTR;

/// A junction (what `mklink /J <link> <target>` makes) created in this process with FSCTL_SET_REPARSE_POINT: no
/// child process, no administrator rights. Removed again (only the junction, never its target) when dropped.
struct Junction(PathBuf);

impl Junction {
    /// `Err` when this machine does not let the test make a junction (endpoint protection, a policy). That is asked
    /// once per test program: a machine that refuses can hold the first attempt up for a minute and a half.
    fn try_new(link: &Path, target: &Path) -> Result<Junction, String> {
        static PROBE: OnceLock<Result<(), String>> = OnceLock::new();
        PROBE
            .get_or_init(|| {
                let tmp = TempDir::new("probe");
                let target = tmp.0.join("target");
                std::fs::create_dir(&target).map_err(|e| e.to_string())?;
                Junction::create(&tmp.0.join("link"), &target).map(drop)
            })
            .clone()?;
        Junction::create(link, target)
    }

    fn create(link: &Path, target: &Path) -> Result<Junction, String> {
        const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
        const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
        std::fs::create_dir(link).map_err(|e| format!("create_dir {}: {e}", link.display()))?;
        let utf16 = |s: &OsStr| -> Vec<u16> { s.encode_wide().collect() };
        let print = utf16(target.as_os_str());
        let mut substitute = utf16(OsStr::new(r"\??\"));
        substitute.extend(&print);
        // MountPointReparseBuffer: offsets and lengths in bytes, both names NUL-terminated
        let (sub_len, print_len) = (substitute.len() * 2, print.len() * 2);
        let mut data: Vec<u8> = Vec::new();
        for v in [0u16, sub_len as u16, (sub_len + 2) as u16, print_len as u16] {
            data.extend(v.to_le_bytes());
        }
        for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
            data.extend(unit.to_le_bytes());
        }
        let mut buffer: Vec<u8> = Vec::new();
        buffer.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buffer.extend((data.len() as u16).to_le_bytes());
        buffer.extend(0u16.to_le_bytes());
        buffer.extend(data);
        let name: Vec<u16> = link.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `name` is NUL-terminated and outlives the call; no security attributes, no template.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                HANDLE::default(),
            )
        };
        let made = handle
            .map_err(|e| format!("open {}: {e}", link.display()))
            .and_then(|h| {
                let mut returned = 0u32;
                // SAFETY: `h` is an open handle to the new folder; `buffer` is a complete reparse buffer.
                let r = unsafe {
                    DeviceIoControl(
                        h,
                        FSCTL_SET_REPARSE_POINT,
                        Some(buffer.as_ptr().cast()),
                        buffer.len() as u32,
                        None,
                        0,
                        Some(&mut returned),
                        None,
                    )
                };
                // SAFETY: closed exactly once.
                let _ = unsafe { CloseHandle(h) };
                r.map_err(|e| format!("FSCTL_SET_REPARSE_POINT on {}: {e}", link.display()))
            });
        match made {
            Ok(()) => Ok(Junction(link.to_path_buf())),
            Err(e) => {
                let _ = std::fs::remove_dir(link);
                Err(e)
            }
        }
    }
}

/// The junction tests need to make a junction; where a machine forbids that they say so and stand down (the fake
/// backend tests in src/exec.rs are the proof there).
macro_rules! junction {
    ($link:expr, $target:expr) => {
        match Junction::try_new($link, $target) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("SKIPPED: this machine does not let a test make a junction: {e}");
                return;
            }
        }
    };
}

impl Drop for Junction {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0); // RemoveDirectory on a junction removes the junction itself
    }
}

#[test]
fn st_035_review_3_the_real_check_accepts_plain_paths_and_refuses_every_kind_of_link() {
    let tmp = TempDir::new("links");
    let root = tmp.0.join("Programming IME");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.dll"), "a").unwrap();
    std::fs::write(root.join("sub").join("b.dll"), "b").unwrap();
    let outside = tmp.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("o.txt"), "o").unwrap();
    let ok = |p: &Path| plain_path_check(&root, p);

    // plain paths, missing paths, another spelling of the same names
    ok(&root).unwrap();
    ok(&root.join("a.dll")).unwrap();
    ok(&root.join("sub").join("b.dll")).unwrap();
    ok(&root.join("missing").join("x.dll")).unwrap();
    let shouted = PathBuf::from(root.to_string_lossy().to_uppercase())
        .join("SUB")
        .join("B.DLL");
    ok(&shouted).unwrap();
    assert!(ok(&outside.join("o.txt")).is_err(), "not inside the folder");
    assert!(
        ok(&root.join("..").join("outside")).is_err(),
        "no .. components"
    );

    // a junction in the middle, as the last component, and as the folder itself (each case stands down on its own
    // where the machine does not let a test make a junction)
    let jn = Junction::try_new(&root.join("jn"), &outside)
        .map_err(|e| eprintln!("SKIPPED (junction in the folder): {e}"))
        .ok();
    if jn.is_some() {
        let e = ok(&root.join("jn").join("o.txt")).unwrap_err();
        assert!(e.contains("reparse point") && e.contains("jn"), "{e}");
        assert!(ok(&root.join("jn")).unwrap_err().contains("reparse point"));
    }
    let rootlink = Junction::try_new(&tmp.0.join("rootlink"), &root)
        .map_err(|e| eprintln!("SKIPPED (junction as the folder): {e}"))
        .ok();
    if let Some(rootlink) = &rootlink {
        assert!(
            plain_path_check(&rootlink.0, &rootlink.0.join("a.dll"))
                .unwrap_err()
                .contains("reparse point")
        );
    }
    // a link ABOVE the folder is not a link inside it: the folder resolves to where it is and is fine
    let above = Junction::try_new(&tmp.0.join("above"), &tmp.0)
        .map_err(|e| eprintln!("SKIPPED (junction above the folder): {e}"))
        .ok();
    if let Some(above) = &above {
        let via = above.0.join("Programming IME");
        plain_path_check(&via, &via.join("a.dll")).unwrap();
    }

    // a symbolic link needs a privilege (Developer Mode or administrator): checked when this account has it
    match std::os::windows::fs::symlink_file(root.join("a.dll"), root.join("sl.dll")) {
        Ok(()) => {
            assert!(
                ok(&root.join("sl.dll"))
                    .unwrap_err()
                    .contains("reparse point")
            );
            std::fs::remove_file(root.join("sl.dll")).unwrap();
        }
        Err(e) => eprintln!("symbolic link case skipped: {e}"),
    }
    drop((jn, rootlink, above));
}

/// Windows ships a few links of its own (`C:\Documents and Settings` is a junction to `C:\Users`, `C:\Users\All Users`
/// a link to `C:\ProgramData`, ...). They are real reparse points that need no privilege to look at: this opens each
/// for its attributes only and writes nothing, so it also runs where this test cannot make a junction of its own.
#[test]
fn st_035_review_3_the_real_check_refuses_the_links_windows_itself_ships() {
    let system_root = system_paths().unwrap().system_root;
    let drive = system_root.ancestors().last().unwrap().to_path_buf();
    let mut seen = 0;
    for link in [
        drive.join("Documents and Settings"),
        drive.join("Users").join("Default User"),
        drive.join("Users").join("All Users"),
    ] {
        if !std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
            continue;
        }
        seen += 1;
        // as the folder itself, as a step on the way down from the drive, and with something below it
        for (root, path) in [
            (link.clone(), link.clone()),
            (drive.clone(), link.clone()),
            (drive.clone(), link.join("anything")),
        ] {
            let e = plain_path_check(&root, &path).unwrap_err();
            assert!(
                e.contains("reparse point"),
                "{} under {}: {e}",
                path.display(),
                root.display()
            );
        }
    }
    if seen == 0 {
        eprintln!("SKIPPED: this Windows has none of the usual compatibility links");
    }
    // and a plain system folder is accepted from the same drive root
    plain_path_check(&drive, &system_root).unwrap();
}

#[test]
fn st_035_review_3_final_paths_on_network_or_volume_locations_are_refused() {
    assert_eq!(plain_final(r"\\?\C:\a\b").unwrap(), r"C:\a\b");
    assert_eq!(plain_final(r"C:\a\b").unwrap(), r"C:\a\b");
    for bad in [
        r"\\?\UNC\server\share\x",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\x",
        r"\\?\GLOBALROOT\Device\x",
    ] {
        assert!(
            plain_final(bad).unwrap_err().contains("local drive"),
            "{bad}"
        );
    }
}

fn manifest_for(prefix: &Path, files: Vec<PathBuf>) -> Manifest {
    Manifest {
        version: "0.0.0".into(),
        prefix: prefix.to_path_buf(),
        files,
        directories: vec![prefix.to_path_buf()],
        registry_keys: Vec::new(),
        text_service_dlls: Vec::new(),
        vscode_extension: None,
    }
}

fn run_uninstall(m: &Manifest) -> pime_setup::exec::Outcome {
    let plan = uninstall_plan(
        m,
        &UninstallOptions {
            vscode: VscodeMode::Skip,
            system: system_paths().unwrap(),
        },
    )
    .unwrap();
    assert!(
        !plan.requires_admin
            && plan.steps.iter().all(|s| matches!(
                s,
                pime_setup::plan::Step::RemoveFile(_) | pime_setup::plan::Step::RemoveDir(_)
            ))
    );
    let none = Payload {
        entries: Vec::new(),
    };
    execute(
        &plan,
        &Inputs {
            payload: &none,
            self_exe: &[],
        },
        &mut RealBackend::without_reboot_schedule(),
        &mut |_| {},
    )
    .unwrap()
}

#[test]
fn st_035_review_3_an_uninstall_over_a_junction_deletes_nothing_outside_the_install_folder() {
    let tmp = TempDir::new("uninst-link");
    // 1. a junction inside the install folder that points at somebody else's folder
    let prefix = tmp.0.join("Programming IME");
    let other = tmp.0.join("other folder");
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(prefix.join("a.txt"), "ours").unwrap();
    std::fs::write(other.join("keep.txt"), "not ours").unwrap();
    let jn = junction!(&prefix.join("sub"), &other);
    let m = manifest_for(
        &prefix,
        vec![prefix.join("a.txt"), prefix.join("sub").join("keep.txt")],
    );
    let out = run_uninstall(&m);
    assert_eq!(std::fs::read(other.join("keep.txt")).unwrap(), b"not ours");
    assert!(
        !prefix.join("a.txt").exists(),
        "the real file of the install is removed"
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("keep.txt") && w.contains("reparse point")),
        "{:?}",
        out.warnings
    );
    drop(jn);

    // 2. the install folder itself became a junction
    let real = tmp.0.join("real folder");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("a.txt"), "not ours either").unwrap();
    let link = tmp.0.join("PIME link");
    let jn = junction!(&link, &real);
    let m = manifest_for(&link, vec![link.join("a.txt")]);
    let out = run_uninstall(&m);
    assert_eq!(
        std::fs::read(real.join("a.txt")).unwrap(),
        b"not ours either"
    );
    assert!(link.exists(), "the junction itself is left alone too");
    assert!(
        out.warnings.iter().any(|w| w.contains("reparse point")),
        "{:?}",
        out.warnings
    );
    drop(jn);
}

#[test]
fn st_035_review_3_an_install_into_a_junction_writes_nothing_through_it() {
    let tmp = TempDir::new("inst-link");
    let target = tmp.0.join("target folder");
    std::fs::create_dir_all(&target).unwrap();
    let prefix = tmp.0.join("Programming IME");
    let jn = junction!(&prefix, &target);
    let options = offline_options("0.3.0");
    let plan = install_plan(&prefix, &options).unwrap();
    assert!(!plan.requires_admin);
    let payload = Payload::from_exe(&overlay("0.3.0")).unwrap().unwrap();
    let err = execute(
        &plan,
        &Inputs {
            payload: &payload,
            self_exe: b"self-exe",
        },
        &mut RealBackend::without_reboot_schedule(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("reparse point"), "{err}");
    assert_eq!(
        names_in(&target),
        Vec::<String>::new(),
        "nothing was written through the junction"
    );
    drop(jn);
}
