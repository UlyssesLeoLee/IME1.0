//! The payload carried by the installer: an overlay appended to the finished exe.
//!
//! Why an overlay and not `include_bytes!`: the Rust stub is built once (and its tests run) without any payload, and
//! `scripts/build-installer.js` turns it into the installer by appending the files. Nothing has to be rebuilt per
//! payload, and the crate compiles on a machine that has never built the DLL. Windows ignores bytes after the last
//! section, and the layout survives Authenticode signing (the signature table is appended after the overlay; see
//! [`find_footer`]).
//!
//! ```text
//! blob   := u32 entry_count | entry*
//! entry  := u16 name_len | name (UTF-8) | u64 data_len | sha256(data) [32] | data
//! footer := "PIME-PAYLOAD-V1\0" [16] | u64 blob_len | sha256(blob) [32]        (all integers little endian)
//! file   := PE stub | blob | footer [| zero padding | Authenticode table]
//! ```
//!
//! The hashes detect truncation and corruption. They are NOT a signature: whoever can modify the file can also fix
//! the hashes. Authenticity needs the code signing that this build does not have.

use crate::plan::{PayloadFile, Release};
use pime_core::json::{self, Json};
use pime_core::sha256::sha256;
use std::fmt;

pub const MAGIC: &[u8; 16] = b"PIME-PAYLOAD-V1\0";
pub const FOOTER_LEN: usize = 16 + 8 + 32;

pub const DLL: &str = "pime_tsf.dll";
pub const CORE: &str = "pime-core.exe";
pub const LICENSE: &str = "LICENSE";
pub const VSIX: &str = "programming-ime.vsix";
pub const META: &str = "meta.json";
/// Entries that must be present, and every entry that is allowed at all.
const REQUIRED: [&str; 4] = [DLL, CORE, LICENSE, META];
const ALLOWED: [&str; 5] = [DLL, CORE, LICENSE, META, VSIX];

#[derive(Debug, PartialEq, Eq)]
pub enum PayloadError {
    /// The structure is damaged (magic found, but sizes or hashes do not match).
    Corrupt(String),
    /// The payload is intact but does not describe a complete release.
    Incomplete(String),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadError::Corrupt(m) => write!(f, "payload is damaged: {m}"),
            PayloadError::Incomplete(m) => write!(f, "payload is incomplete: {m}"),
        }
    }
}

impl std::error::Error for PayloadError {}

pub struct Entry {
    pub name: String,
    pub sha256: [u8; 32],
    pub data: Vec<u8>,
}

pub struct Payload {
    pub entries: Vec<Entry>,
}

/// Entry names are plain file names: this is what keeps a crafted payload from writing outside the install folder.
pub fn valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Serializes `entries` as blob + footer (the format `scripts/build-installer.js` writes as well).
pub fn pack(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, data) in entries {
        blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
        blob.extend_from_slice(name.as_bytes());
        blob.extend_from_slice(&(data.len() as u64).to_le_bytes());
        blob.extend_from_slice(&sha256(data));
        blob.extend_from_slice(data);
    }
    let mut out = blob.clone();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
    out.extend_from_slice(&sha256(&blob));
    out
}

/// File offset where the Authenticode certificate table starts (PE32+ only), or `None` for an unsigned file.
fn certificate_table_offset(exe: &[u8]) -> Option<usize> {
    let u32_at = |o: usize| {
        exe.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let pe = u32_at(0x3c)? as usize;
    if exe.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let optional = pe + 24;
    let magic = exe.get(optional..optional + 2)?;
    if magic != [0x0b, 0x02] {
        return None; // not PE32+
    }
    // data directory 4 = security (the "address" is a file offset, not an RVA)
    let entry = optional + 112 + 4 * 8;
    let offset = u32_at(entry)? as usize;
    let size = u32_at(entry + 4)? as usize;
    (offset != 0 && size != 0 && offset <= exe.len()).then_some(offset)
}

/// Finds the footer. For a signed file the payload ends where the certificate table starts; signtool pads the data
/// in front of the table to an 8-byte boundary, so up to 7 zero bytes may follow the footer.
fn find_footer(exe: &[u8]) -> Option<usize> {
    let end = certificate_table_offset(exe).unwrap_or(exe.len());
    (0..8usize).find_map(|pad| {
        let start = end.checked_sub(pad + FOOTER_LEN)?;
        (exe.get(start..start + 16)? == MAGIC).then_some(start)
    })
}

impl Payload {
    /// `Ok(None)`: the exe carries no payload (the bare stub). `Err`: it carries one, but damaged.
    pub fn from_exe(exe: &[u8]) -> Result<Option<Payload>, PayloadError> {
        let Some(footer) = find_footer(exe) else {
            return Ok(None);
        };
        let blob_len = u64_le(&exe[footer + 16..footer + 24]) as usize;
        let expected = &exe[footer + 24..footer + FOOTER_LEN];
        let start = footer.checked_sub(blob_len).ok_or_else(|| {
            PayloadError::Corrupt("the length in the footer exceeds the file".into())
        })?;
        let blob = &exe[start..footer];
        if sha256(blob) != expected {
            return Err(PayloadError::Corrupt(
                "SHA-256 of the payload does not match the footer (the file is truncated or was modified)".into(),
            ));
        }
        parse_blob(blob).map(Some)
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Validates that this is a complete release and describes it (name, size and hash of every installed file).
    pub fn release(&self) -> Result<Release, PayloadError> {
        for e in &self.entries {
            if !ALLOWED.contains(&e.name.as_str()) {
                return Err(PayloadError::Incomplete(format!(
                    "unexpected entry {}",
                    e.name
                )));
            }
        }
        for name in REQUIRED {
            if self.get(name).is_none() {
                return Err(PayloadError::Incomplete(format!("missing entry {name}")));
            }
        }
        let meta = self
            .get(META)
            .map(|e| e.data.as_slice())
            .unwrap_or_default();
        let meta = std::str::from_utf8(meta)
            .ok()
            .and_then(|t| json::parse(t, json::Limits::DEFAULT).ok())
            .ok_or_else(|| PayloadError::Incomplete(format!("{META} is not valid JSON")))?;
        let version = meta
            .get("version")
            .and_then(Json::as_str)
            .filter(|v| valid_token(v, 32, b".+-"))
            .ok_or_else(|| PayloadError::Incomplete(format!("{META} has no valid \"version\"")))?;
        let extension_id = match (
            self.get(VSIX),
            meta.get("extension_id").and_then(Json::as_str),
        ) {
            (Some(_), Some(id)) if valid_token(id, 128, b".-_") => Some(id.to_string()),
            (Some(_), _) => {
                return Err(PayloadError::Incomplete(format!(
                    "{VSIX} is present but {META} has no valid \"extension_id\""
                )));
            }
            (None, _) => None,
        };
        let files = self
            .entries
            .iter()
            .filter(|e| e.name != META)
            .map(|e| PayloadFile {
                name: e.name.clone(),
                size: e.data.len() as u64,
                sha256: e.sha256,
            })
            .collect();
        Ok(Release {
            version: version.to_string(),
            extension_id,
            files,
        })
    }
}

fn valid_token(s: &str, max: usize, extra: &[u8]) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

fn u64_le(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(b);
    u64::from_le_bytes(a)
}

fn parse_blob(blob: &[u8]) -> Result<Payload, PayloadError> {
    let corrupt = |m: &str| PayloadError::Corrupt(m.to_string());
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], PayloadError> {
        let end = at.checked_add(n).filter(|e| *e <= blob.len());
        let slice = end
            .map(|e| &blob[at..e])
            .ok_or_else(|| corrupt("an entry runs past the end of the payload"))?;
        at += n;
        Ok(slice)
    };
    let count =
        u32::from_le_bytes(take(4)?.try_into().map_err(|_| corrupt("short header"))?) as usize;
    if count > 64 {
        return Err(corrupt("implausible entry count"));
    }
    let mut entries: Vec<Entry> = Vec::new();
    for _ in 0..count {
        let name_len =
            u16::from_le_bytes(take(2)?.try_into().map_err(|_| corrupt("short entry"))?) as usize;
        let name = std::str::from_utf8(take(name_len)?)
            .map_err(|_| corrupt("entry name is not UTF-8"))?
            .to_string();
        if !valid_entry_name(&name) {
            return Err(PayloadError::Corrupt(format!(
                "illegal entry name {name:?}"
            )));
        }
        if entries.iter().any(|e| e.name == name) {
            return Err(PayloadError::Corrupt(format!("duplicate entry {name}")));
        }
        let data_len = usize::try_from(u64_le(take(8)?)).map_err(|_| corrupt("entry too large"))?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(take(32)?);
        let data = take(data_len)?.to_vec();
        if sha256(&data) != hash {
            return Err(PayloadError::Corrupt(format!(
                "SHA-256 of {name} does not match"
            )));
        }
        entries.push(Entry {
            name,
            sha256: hash,
            data,
        });
    }
    if at != blob.len() {
        return Err(corrupt("bytes remain after the last entry"));
    }
    Ok(Payload { entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(version: &str) -> Vec<u8> {
        format!(r#"{{"version":"{version}","extension_id":"pub.name"}}"#).into_bytes()
    }

    fn full_entries(m: &[u8]) -> Vec<(&str, &[u8])> {
        vec![
            (DLL, b"DLL"),
            (CORE, b"CORE"),
            (LICENSE, b"MIT"),
            (META, m),
            (VSIX, b"VSIX"),
        ]
    }

    fn stub() -> Vec<u8> {
        b"MZ stub bytes".to_vec()
    }

    /// A minimal PE32+ header with a security directory entry, enough for `certificate_table_offset`.
    fn fake_pe(cert_offset: u32, cert_size: u32) -> Vec<u8> {
        let mut pe = vec![0u8; 0x200];
        pe[0] = b'M';
        pe[1] = b'Z';
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x80 + 24..0x80 + 26].copy_from_slice(&[0x0b, 0x02]);
        let dir = 0x80 + 24 + 112 + 4 * 8;
        pe[dir..dir + 4].copy_from_slice(&cert_offset.to_le_bytes());
        pe[dir + 4..dir + 8].copy_from_slice(&cert_size.to_le_bytes());
        pe
    }

    #[test]
    fn st_035_a_stub_without_overlay_has_no_payload() {
        assert!(Payload::from_exe(&stub()).unwrap().is_none());
        assert!(Payload::from_exe(&[]).unwrap().is_none());
    }

    #[test]
    fn st_035_pack_and_parse_round_trip_with_a_complete_release() {
        let m = meta("0.2.0");
        let mut exe = stub();
        exe.extend(pack(&full_entries(&m)));
        let payload = Payload::from_exe(&exe).unwrap().expect("payload");
        assert_eq!(payload.entries.len(), 5);
        assert_eq!(payload.get(DLL).unwrap().data, b"DLL");
        let release = payload.release().unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.extension_id.as_deref(), Some("pub.name"));
        let names: Vec<_> = release.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [DLL, CORE, LICENSE, VSIX]);
        assert_eq!(release.files[0].sha256, sha256(b"DLL"));
        assert_eq!(release.files[0].size, 3);
    }

    #[test]
    fn st_035_every_flipped_bit_region_is_detected() {
        let m = meta("0.2.0");
        let mut exe = stub();
        let base = exe.len();
        exe.extend(pack(&full_entries(&m)));
        for at in [base + 6, base + 40, exe.len() - 20, exe.len() - 40] {
            let mut bad = exe.clone();
            bad[at] ^= 0x01;
            assert!(
                matches!(
                    Payload::from_exe(&bad),
                    Err(PayloadError::Corrupt(_)) | Ok(None)
                ),
                "flip at {at} must not parse as a valid payload"
            );
        }
        // the usual real-world damage: a truncated download
        let truncated = &exe[..exe.len() - 10];
        assert!(!matches!(Payload::from_exe(truncated), Ok(Some(_))));
    }

    #[test]
    fn st_035_payload_survives_a_signature_table_with_alignment_padding() {
        let m = meta("0.2.0");
        let mut exe = fake_pe(0, 0);
        exe.extend(pack(&full_entries(&m)));
        exe.extend([0u8; 5]); // signtool pads to an 8-byte boundary before the table
        let cert_at = exe.len();
        exe.extend([0xAB; 64]); // the certificate table
        let dir = 0x80 + 24 + 112 + 4 * 8;
        exe[dir..dir + 4].copy_from_slice(&(cert_at as u32).to_le_bytes());
        exe[dir + 4..dir + 8].copy_from_slice(&64u32.to_le_bytes());
        let payload = Payload::from_exe(&exe)
            .unwrap()
            .expect("found before the signature");
        assert_eq!(payload.release().unwrap().version, "0.2.0");
    }

    #[test]
    fn st_035_names_must_be_plain_file_names() {
        for bad in [
            "",
            "..",
            "a/b",
            "a\\b",
            ".hidden",
            "x:y",
            "evil.",
            "é.dll",
            &"n".repeat(65),
        ] {
            assert!(!valid_entry_name(bad), "{bad:?}");
        }
        assert!(valid_entry_name("pime_tsf.dll") && valid_entry_name("programming-ime.vsix"));
        // and a crafted payload carrying such a name is rejected while parsing
        let evil = pack(&[("..\\evil.dll", b"x".as_slice())]);
        assert!(matches!(
            Payload::from_exe(&evil),
            Err(PayloadError::Corrupt(_))
        ));
    }

    #[test]
    fn st_035_release_requires_the_standard_set_and_a_valid_meta() {
        let m = meta("0.2.0");
        let mut exe = stub();
        exe.extend(pack(&[(DLL, b"x".as_slice()), (META, &m)]));
        let p = Payload::from_exe(&exe).unwrap().unwrap();
        assert!(
            matches!(p.release(), Err(PayloadError::Incomplete(msg)) if msg.contains("missing"))
        );

        let unknown = pack(&[
            (DLL, b"x".as_slice()),
            (CORE, b"x"),
            (LICENSE, b"x"),
            (META, &m),
            ("extra.bin", b"x"),
        ]);
        let p = Payload::from_exe(&unknown).unwrap().unwrap();
        assert!(
            matches!(p.release(), Err(PayloadError::Incomplete(msg)) if msg.contains("unexpected"))
        );

        for bad_meta in [
            &b"not json"[..],
            br#"{"version":""}"#,
            br#"{"version":"1 2"}"#,
            br#"{}"#,
        ] {
            let e = pack(&[
                (DLL, b"x".as_slice()),
                (CORE, b"x"),
                (LICENSE, b"x"),
                (META, bad_meta),
            ]);
            let p = Payload::from_exe(&e).unwrap().unwrap();
            assert!(
                p.release().is_err(),
                "{}",
                String::from_utf8_lossy(bad_meta)
            );
        }
        // a vsix needs the extension id to be known (the uninstaller removes it by id)
        let no_id = br#"{"version":"0.2.0"}"#;
        let e = pack(&[
            (DLL, b"x".as_slice()),
            (CORE, b"x"),
            (LICENSE, b"x"),
            (META, no_id),
            (VSIX, b"x"),
        ]);
        assert!(Payload::from_exe(&e).unwrap().unwrap().release().is_err());
    }
}
