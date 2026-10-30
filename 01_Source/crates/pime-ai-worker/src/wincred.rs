//! Windows Credential Manager as the credential store (`CredWriteW` / `CredReadW` / `CredDeleteW`).
//!
//! A credential is a generic credential: `TargetName` = `<namespace>/<provider>`, `UserName` = the origin the key may be
//! sent to, `CredentialBlob` = the key as UTF-16 (the convention of the Credential Manager UI and `cmdkey`, so a key
//! stored here can be inspected and replaced in the Control Panel). Persistence is "local machine" for the product and
//! "session" for the tests (a session entry disappears at log-off even if a test was killed before its cleanup).
//!
//! Honest limit: any process of the same Windows user can read a generic credential with `CredReadW`. The protection
//! this module provides is that the key is not in the extension host, not in settings and not in any file; it is not a
//! defence against malware running as the same user (see docs/AI_WORKER.md).
#![allow(unsafe_code)]

use crate::secret::{Secret, WideSecret, wipe};
use crate::vault::{Stored, Vault, VaultError};
use std::ffi::c_void;
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST, CRED_PERSIST_LOCAL_MACHINE, CRED_PERSIST_SESSION,
    CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};

pub struct WinCredVault {
    persist: CRED_PERSIST,
}

impl WinCredVault {
    /// `session = true` is for tests only: the entries live until log-off.
    pub fn new(session: bool) -> WinCredVault {
        WinCredVault {
            persist: if session {
                CRED_PERSIST_SESSION
            } else {
                CRED_PERSIST_LOCAL_MACHINE
            },
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The Win32 error code inside a `windows` error (an HRESULT with the Win32 facility carries it in the low 16 bits).
fn win32_of(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr >> 16 == 0x8007 { hr & 0xffff } else { hr }
}

/// Owns a credential returned by `CredReadW`: wipes the blob and frees the buffer when dropped.
struct ReadCredential(*mut CREDENTIALW);

impl ReadCredential {
    fn blob(&self) -> &[u8] {
        // SAFETY: `self.0` is the non-null pointer `CredReadW` returned and stays valid until `CredFree` in `drop`;
        // the blob pointer and size are the API's own and describe one allocation.
        unsafe {
            let c = &*self.0;
            if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
            }
        }
    }

    fn origin(&self) -> String {
        // SAFETY: as above; `UserName` is either null or a NUL-terminated UTF-16 string owned by the same allocation.
        unsafe {
            let c = &*self.0;
            if c.UserName.is_null() {
                String::new()
            } else {
                c.UserName.to_string().unwrap_or_default()
            }
        }
    }
}

impl Drop for ReadCredential {
    fn drop(&mut self) {
        // SAFETY: the pointer is valid until `CredFree`; the blob memory belongs to this process and is writable, and it
        // is wiped before the buffer is handed back so the key does not linger in freed memory.
        unsafe {
            let c = &*self.0;
            if !c.CredentialBlob.is_null() && c.CredentialBlobSize > 0 {
                wipe(std::slice::from_raw_parts_mut(
                    c.CredentialBlob,
                    c.CredentialBlobSize as usize,
                ));
            }
            CredFree(self.0 as *const c_void);
        }
    }
}

/// The blob is UTF-16 text; a key is ASCII, so anything else is rejected by `Secret::parse` (which wipes what it got).
fn secret_from_blob(blob: &[u8]) -> Result<Secret, VaultError> {
    let (pairs, rest) = blob.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(VaultError(0));
    }
    let mut units: Vec<u16> = pairs.iter().map(|b| u16::from_le_bytes(*b)).collect();
    while units.last() == Some(&0) {
        units.pop();
    }
    let mut bytes = Vec::with_capacity(units.len());
    let mut ascii = true;
    for &u in &units {
        match u8::try_from(u) {
            Ok(b) if b.is_ascii() => bytes.push(b),
            _ => {
                ascii = false;
                break;
            }
        }
    }
    crate::secret::wipe_vec(&mut units);
    if !ascii {
        crate::secret::wipe_vec(&mut bytes);
        return Err(VaultError(0));
    }
    // ASCII bytes are valid UTF-8, so this does not copy
    let text = String::from_utf8(bytes).map_err(|_| VaultError(0))?;
    Secret::parse(text).map_err(|_| VaultError(0))
}

impl Vault for WinCredVault {
    fn set(&self, target: &str, origin: &str, secret: &Secret) -> Result<(), VaultError> {
        let target_w = wide(target);
        let mut origin_w = wide(origin);
        let blob = WideSecret(secret.expose().iter().map(|&b| u16::from(b)).collect());
        let blob_bytes = blob.0.len() * 2;
        if blob_bytes > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(VaultError(0));
        }
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_w.as_ptr() as *mut u16),
            UserName: PWSTR(origin_w.as_mut_ptr()),
            CredentialBlobSize: blob_bytes as u32,
            CredentialBlob: blob.0.as_ptr() as *mut u8,
            Persist: self.persist,
            ..Default::default()
        };
        // SAFETY: every pointer in `credential` points into a buffer that outlives the call (`target_w`, `origin_w`,
        // `blob`), the strings are NUL-terminated, and `CredWriteW` only reads them (the `*mut` casts satisfy the
        // struct's field types, they are never written through).
        let written = unsafe { CredWriteW(&credential, 0) };
        // `blob` (the key as UTF-16) is wiped when it goes out of scope
        written.map_err(|e| VaultError(win32_of(&e)))
    }

    fn read(&self, target: &str) -> Result<Option<Stored>, VaultError> {
        let target_w = wide(target);
        let mut raw: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target_w` is NUL-terminated and outlives the call; `raw` is a valid out-pointer.
        let result =
            unsafe { CredReadW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, 0, &mut raw) };
        match result {
            Err(e) if win32_of(&e) == ERROR_NOT_FOUND.0 => return Ok(None),
            Err(e) => return Err(VaultError(win32_of(&e))),
            Ok(()) => {}
        }
        if raw.is_null() {
            return Err(VaultError(0));
        }
        let credential = ReadCredential(raw);
        let secret = secret_from_blob(credential.blob())?;
        Ok(Some(Stored {
            secret,
            origin: credential.origin(),
        }))
    }

    fn delete(&self, target: &str) -> Result<bool, VaultError> {
        let target_w = wide(target);
        // SAFETY: `target_w` is NUL-terminated and outlives the call.
        let result = unsafe { CredDeleteW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, 0) };
        match result {
            Ok(()) => Ok(true),
            Err(e) if win32_of(&e) == ERROR_NOT_FOUND.0 => Ok(false),
            Err(e) => Err(VaultError(win32_of(&e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob_of(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn the_blob_decoder_accepts_a_utf16_ascii_key_with_or_without_a_trailing_nul() {
        let s = secret_from_blob(&blob_of("mock-key-test-123")).unwrap();
        assert_eq!(s.expose(), b"mock-key-test-123");
        let mut with_nul = blob_of("mock-key-test-123");
        with_nul.extend_from_slice(&[0, 0]);
        assert_eq!(
            secret_from_blob(&with_nul).unwrap().expose(),
            b"mock-key-test-123"
        );
    }

    #[test]
    fn the_blob_decoder_refuses_everything_that_is_not_a_plausible_key() {
        assert!(secret_from_blob(&[]).is_err());
        assert!(secret_from_blob(b"a").is_err(), "odd length");
        assert!(secret_from_blob(&blob_of("short")).is_err());
        assert!(secret_from_blob(&blob_of("caf\u{e9}-key-1234567")).is_err());
        assert!(
            secret_from_blob(&blob_of(
                "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}-key-1234"
            ))
            .is_err()
        );
        assert!(secret_from_blob(&blob_of("has space in key")).is_err());
        // ASCII bytes stored as plain bytes (not UTF-16) decode to CJK-looking units and are refused
        assert!(secret_from_blob(b"mock-key-test-123").is_err());
    }

    #[test]
    fn win32_codes_are_extracted_from_hresults() {
        let e = windows::core::Error::from(windows::core::HRESULT(0x8007_0490_u32 as i32));
        assert_eq!(win32_of(&e), 1168);
        let e = windows::core::Error::from(windows::core::HRESULT(0x8000_4005_u32 as i32));
        assert_eq!(win32_of(&e), 0x8000_4005);
    }
}
