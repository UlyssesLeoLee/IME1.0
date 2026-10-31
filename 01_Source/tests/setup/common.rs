//! Helpers shared by the installer's integration test programs (real_fs.rs, real_links.rs): a temp folder that is
//! removed again, and the fake payload that scripts/build-installer.js would produce.
#![allow(dead_code)]

use pime_setup::payload::{self, Payload};
use pime_setup::plan::{InstallOptions, VscodeMode};
use pime_setup::real::system_paths;
use std::path::{Path, PathBuf};

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> TempDir {
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

pub fn entries(version: &str) -> Vec<(&'static str, Vec<u8>)> {
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

pub fn overlay(version: &str) -> Vec<u8> {
    let e = entries(version);
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, d)| (*n, d.as_slice())).collect();
    payload::pack(&refs)
}

/// The test binary of `pime-setup` plus a payload: what scripts/build-installer.js produces.
pub fn installer_exe(dir: &Path, version: &str) -> PathBuf {
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_pime-setup")).unwrap();
    bytes.extend(overlay(version));
    let path = dir.join(format!("setup-{version}.exe"));
    std::fs::write(&path, bytes).unwrap();
    path
}

pub fn offline_options(version: &str) -> InstallOptions {
    let payload = Payload::from_exe(&overlay(version)).unwrap().unwrap();
    InstallOptions {
        register: false,
        arp: false,
        vscode: VscodeMode::Skip,
        release: payload.release().unwrap(),
        system: system_paths().unwrap(),
    }
}

pub fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}
