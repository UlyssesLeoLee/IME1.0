//! What is on the machine already.
//!
//! The setup is not only an installer: when the program is already on the machine, the window says so and offers to
//! repair the install, to install it again or to remove it - the way the setup of another program does it.
//!
//! The search is read-only and never changes anything. It looks at three places, in this order:
//!
//! 1. the install manifest of a folder (`install-manifest.json`): the folder was installed by this installer and it
//!    says exactly what it put there;
//! 2. the entry of "Apps & Features" (`HKLM\...\Uninstall\ProgrammingIME`): the program was installed, even if the
//!    manifest is gone;
//! 3. the class of the text service (`HKLM\Software\Classes\CLSID\{...}\InprocServer32`): the DLL is registered, so
//!    the program is on the machine even if nothing else is.

use std::path::{Path, PathBuf};

use crate::backend::{Backend, RegValue};
use crate::plan::{self, Manifest, MANIFEST_NAME};

/// The key of the entry of "Apps & Features", relative to `HKEY_LOCAL_MACHINE`.
const ARP_KEY: &str = plan::ARP_KEY;

/// The key of the class of the text service, relative to `HKEY_LOCAL_MACHINE`. `regsvr32` writes the path of the DLL
/// there as the default value of the key.
const COM_KEY: &str = concat!(
    "Software\\Classes\\CLSID\\",
    "{43F95ED1-95B4-421D-8F5A-837128C2D854}\\InprocServer32"
);

/// How the install that is already there was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The install manifest of the folder was there.
    Manifest,
    /// The entry of "Apps & Features" was there.
    Arp,
    /// The text service was registered.
    Registered,
}

/// The install that is already on the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Existing {
    /// The folder the program is in.
    pub prefix: PathBuf,
    /// The version that is installed, if it is known.
    pub version: Option<String>,
    /// How it was found.
    pub found: Found,
    /// The manifest of the install, if it could be read. Without it there is nothing to remove file by file, so the
    /// window cannot offer to uninstall.
    pub manifest: Option<Manifest>,
}

impl Existing {
    /// Whether the program can be removed file by file: only the manifest says what was put there.
    pub fn can_uninstall(&self) -> bool {
        self.manifest.is_some()
    }
}

/// Reads the manifest of a folder, if it has one that is valid.
fn read_manifest(b: &dyn Backend, prefix: &Path) -> Option<Manifest> {
    let path = prefix.join(MANIFEST_NAME);
    if !b.exists(&path) {
        return None;
    }
    let text = b
        .read_file(&path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    // A manifest that does not belong to the folder it was found in is refused (it was copied or edited): it would
    // point the uninstall at files of somebody else.
    Manifest::parse(&text, prefix).ok()
}

/// The folders that are looked at, in the order they are looked at.
pub fn candidates(
    chosen: &Path,
    system: &crate::plan::SystemPaths,
    running: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|q| plan::same_path(&p, q)) {
            out.push(p);
        }
    };
    // The folder the program goes to when the product folder is appended to the one that was chosen. It comes first:
    // that is the folder the program really lives in.
    if let Ok(resolved) = plan::resolve_install_dir(chosen, system) {
        push(resolved);
    }
    push(chosen.to_path_buf());
    // The folder this program runs from: the uninstaller is a copy of the setup that lives in the install folder.
    if let Some(dir) = running.and_then(|p| p.parent()) {
        push(dir.to_path_buf());
    }
    push(system.default_prefix());
    // The folder of the name the product had before the rename is **not** looked at: it is a different product name,
    // and a repair there would put the program into a folder inside it. The window tells the person to remove that
    // folder by hand instead (see `wizard::legacy_hint`).
    out
}

/// Looks for the install that is already on the machine.
///
/// `folders` are the places that are looked at, in order. The first one that has a manifest wins. If no folder has
/// one, the entry of "Apps & Features" is asked, and after that the registration of the text service.
pub fn detect(folders: &[PathBuf], b: &dyn Backend) -> Option<Existing> {
    for folder in folders {
        if let Some(manifest) = read_manifest(b, folder) {
            return Some(Existing {
                prefix: folder.clone(),
                version: Some(manifest.version.clone()),
                found: Found::Manifest,
                manifest: Some(manifest),
            });
        }
    }
    // The entry of "Apps & Features" says where the program was installed.
    if let Ok(Some(values)) = b.reg_read_key(ARP_KEY)
        && let Some((_, RegValue::Str(location))) = values.iter().find(|(n, _)| n == "InstallLocation")
    {
        let prefix = PathBuf::from(location);
        if !prefix.as_os_str().is_empty() {
            let manifest = read_manifest(b, &prefix);
            let version = manifest.as_ref().map(|m| m.version.clone());
            return Some(Existing {
                prefix,
                version,
                found: Found::Arp,
                manifest,
            });
        }
    }
    // The text service is registered: the DLL is somewhere on the machine.
    if let Ok(Some(values)) = b.reg_read_key(COM_KEY)
        && let Some((_, RegValue::Str(dll))) = values.iter().find(|(n, _)| n.is_empty())
    {
        let dll = PathBuf::from(dll);
        if let Some(folder) = dll.parent() {
            let prefix = folder.to_path_buf();
            let manifest = read_manifest(b, &prefix);
            let version = manifest.as_ref().map(|m| m.version.clone());
            return Some(Existing {
                prefix,
                version,
                found: Found::Registered,
                manifest,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::plan::{Manifest, SystemPaths};

    fn system() -> SystemPaths {
        SystemPaths {
            system_root: PathBuf::from(r"C:\Windows"),
            system_dir: PathBuf::from(r"C:\Windows\System32"),
            program_files: PathBuf::from(r"C:\Program Files"),
            program_files_x86: None,
            program_data: None,
            user_profile: None,
            local_time: crate::plan::LocalTime {
                year: 2026,
                month: 11,
                day: 4,
                hour: 12,
                minute: 0,
                second: 0,
            },
        }
    }

    fn manifest_of(prefix: &Path) -> Manifest {
        Manifest {
            version: "0.2.0".into(),
            prefix: prefix.to_path_buf(),
            files: vec![prefix.join("pime_tsf.dll")],
            directories: vec![prefix.to_path_buf()],
            registry_keys: vec![format!("HKLM\\{}", ARP_KEY)],
            text_service_dlls: vec![prefix.join("pime_tsf.dll")],
            vscode_extension: None,
        }
    }

    #[test]
    fn nothing_on_the_machine_is_reported_as_nothing() {
        let b = FakeBackend::new();
        assert_eq!(detect(&[PathBuf::from(r"D:\IME1.0")], &b), None);
    }

    #[test]
    fn the_manifest_of_a_folder_is_the_install_that_is_there() {
        let mut b = FakeBackend::new();
        let prefix = PathBuf::from(r"D:\IME1.0");
        b.put(
            &prefix.join(MANIFEST_NAME).display().to_string(),
            manifest_of(&prefix).to_json().as_bytes(),
        );
        let found = detect(std::slice::from_ref(&prefix), &b).expect("the install is found");
        assert_eq!(found.prefix, prefix);
        assert_eq!(found.version.as_deref(), Some("0.2.0"));
        assert_eq!(found.found, Found::Manifest);
        assert!(found.can_uninstall());
    }
    #[test]
    fn the_entry_of_apps_and_features_is_found_without_a_manifest() {
        let mut b = FakeBackend::new();
        b.state.registry.insert(
            ARP_KEY.to_lowercase(),
            vec![(
                "InstallLocation".to_string(),
                RegValue::Str(r"D:\IME1.0".to_string()),
            )],
        );
        let found = detect(&[], &b).expect("the install is found");
        assert_eq!(found.prefix, PathBuf::from(r"D:\IME1.0"));
        assert_eq!(found.found, Found::Arp);
        assert!(found.version.is_none());
        // without a manifest there is nothing to remove file by file
        assert!(!found.can_uninstall());
    }

    #[test]
    fn the_registered_text_service_is_found_without_anything_else() {
        let mut b = FakeBackend::new();
        b.state.registry.insert(
            COM_KEY.to_lowercase(),
            vec![(
                String::new(),
                RegValue::Str(r"D:\IME1.0\pime_tsf.dll".to_string()),
            )],
        );
        let found = detect(&[], &b).expect("the install is found");
        assert_eq!(found.prefix, PathBuf::from(r"D:\IME1.0"));
        assert_eq!(found.found, Found::Registered);
    }

    #[test]
    fn a_manifest_that_belongs_to_another_folder_is_refused() {
        let mut b = FakeBackend::new();
        let prefix = PathBuf::from(r"D:\IME1.0");
        // the manifest was copied from another folder: it must not point the uninstall at somebody else's files
        let other = PathBuf::from(r"D:\Other");
        b.put(
            &prefix.join(MANIFEST_NAME).display().to_string(),
            manifest_of(&other).to_json().as_bytes(),
        );
        assert!(detect(&[prefix], &b).is_none());
    }

    #[test]
    fn the_folders_that_are_looked_at_are_the_chosen_one_the_default_one_and_the_folder_of_this_program() {
        let system = system();
        let folders = candidates(
            Path::new(r"C:\Program Files"),
            &system,
            Some(Path::new(r"D:\IME1.0\uninstall.exe")),
        );
        assert_eq!(folders[0], PathBuf::from(r"C:\Program Files\IME1.0"));
        assert!(folders.contains(&PathBuf::from(r"D:\IME1.0")));
        assert!(folders.contains(&system.default_prefix()));
        // the folder of the old name is not looked at: a repair there would land in a folder inside it
        assert!(!folders.contains(&system.legacy_prefix()));
    }
}
