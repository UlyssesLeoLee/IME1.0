//! Key material that is wiped when it is dropped.
//!
//! This is best effort and says so: the volatile writes stop the compiler from removing the wipe, but they cannot reach
//! copies the operating system, the pipe buffers or WinHTTP made, nor the intermediate buffers of a growing `String`
//! that were freed before the wipe. What it does guarantee is that the buffers this crate owns do not keep a key after
//! the request that needed it.
#![allow(unsafe_code)]

use std::fmt;
use std::sync::atomic::{Ordering, compiler_fence};

/// Overwrites every element with its default value in a way the optimizer may not drop.
pub fn wipe<T: Copy + Default>(buf: &mut [T]) {
    for slot in buf.iter_mut() {
        // SAFETY: `slot` is a valid, aligned, exclusive reference to an initialized element.
        unsafe { std::ptr::write_volatile(slot, T::default()) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// Wipes a vector's initialized elements and its spare capacity, then empties it.
pub fn wipe_vec<T: Copy + Default>(v: &mut Vec<T>) {
    wipe(v.as_mut_slice());
    for slot in v.spare_capacity_mut() {
        // SAFETY: writing a plain value into spare capacity is always allowed (the slot is `MaybeUninit`).
        unsafe { std::ptr::write_volatile(slot.as_mut_ptr(), T::default()) };
    }
    compiler_fence(Ordering::SeqCst);
    v.clear();
}

/// Wipes the bytes of a string and empties it.
pub fn wipe_string(s: &mut String) {
    // SAFETY: the bytes are overwritten with zeros (valid UTF-8) and the string is cleared right away.
    wipe_vec(unsafe { s.as_mut_vec() });
}

pub const MIN_SECRET_LEN: usize = 8;
/// A Credential Manager blob holds at most 2560 bytes and the key is stored as UTF-16, so at most 1280 characters fit;
/// the limit leaves room and is far above any real API key.
pub const MAX_SECRET_LEN: usize = 1024;

/// An API key. There is no `Clone` and the `Debug` output hides the content.
pub struct Secret(Vec<u8>);

impl Secret {
    /// Accepts printable ASCII without space, quote and backslash (every real API key qualifies). That keeps the key
    /// valid inside an HTTP header and makes an exact-match scrub of responses reliable. On failure the input is wiped.
    pub fn parse(mut text: String) -> Result<Secret, SecretError> {
        let ok = (MIN_SECRET_LEN..=MAX_SECRET_LEN).contains(&text.len())
            && text
                .bytes()
                .all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'\\');
        if !ok {
            wipe_string(&mut text);
            return Err(SecretError);
        }
        Ok(Secret(text.into_bytes()))
    }

    /// Builds a secret from bytes already validated elsewhere (the credential store reads the blob and validates it).
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Secret {
        Secret(bytes)
    }

    pub(crate) fn expose(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe_vec(&mut self.0);
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The rejected key is never described (not even its length).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretError;

/// A UTF-16 buffer (the header block handed to WinHTTP) that is wiped when it is dropped.
pub struct WideSecret(pub Vec<u16>);

impl Drop for WideSecret {
    fn drop(&mut self) {
        wipe_vec(&mut self.0);
    }
}

impl fmt::Debug for WideSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WideSecret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_overwrites_every_element() {
        let mut bytes = b"mock-key-test-123".to_vec();
        wipe(&mut bytes);
        assert!(bytes.iter().all(|&b| b == 0));
        let mut wide: Vec<u16> = "Authorization: Bearer abc".encode_utf16().collect();
        wipe(&mut wide);
        assert!(wide.iter().all(|&c| c == 0));
    }

    #[test]
    fn wipe_vec_clears_the_spare_capacity_too() {
        let mut v: Vec<u8> = Vec::with_capacity(64);
        v.extend_from_slice(b"sk-secret-secret");
        v.truncate(4); // 12 stale key bytes now sit in the spare capacity
        let base = v.as_ptr();
        wipe_vec(&mut v);
        assert!(v.is_empty());
        // SAFETY: the allocation is still owned by `v` (capacity 64); only the first 16 bytes are inspected.
        let raw = unsafe { std::slice::from_raw_parts(base, 16) };
        assert!(raw.iter().all(|&b| b == 0), "stale bytes must be wiped");
    }

    #[test]
    fn wipe_string_empties_and_zeroes() {
        let mut s = String::from("sk-ant-api03-AAAA");
        let base = s.as_ptr();
        wipe_string(&mut s);
        assert!(s.is_empty());
        // SAFETY: the allocation is still owned by `s`; 17 bytes were written before.
        let raw = unsafe { std::slice::from_raw_parts(base, 17) };
        assert!(raw.iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_accepts_realistic_keys_and_rejects_everything_else() {
        for ok in [
            "mock-key-test-123",
            "sk-proj-AbCdEf0123456789_-AbCdEf0123456789",
            "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "a.b/c+d=e~f12345",
        ] {
            assert!(Secret::parse(ok.to_string()).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "short",
            "has space in it",
            "tab\tinside-key-1",
            "new\nline-in-key",
            "quote\"in-the-key",
            "back\\slash-in-key",
            "caf\u{e9}-key-123456",
            "nul\0inside-key1",
        ] {
            assert!(Secret::parse(bad.to_string()).is_err(), "{bad:?}");
        }
        assert!(Secret::parse("k".repeat(MAX_SECRET_LEN)).is_ok());
        assert!(Secret::parse("k".repeat(MAX_SECRET_LEN + 1)).is_err());
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let s = Secret::parse("mock-key-test-123".to_string()).unwrap();
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert_eq!(format!("{:?}", Some(&s)), "Some(Secret(<redacted>))");
        let w = WideSecret("mock-key-test-123".encode_utf16().collect());
        assert!(!format!("{w:?}").contains("mock"));
        assert!(!format!("{:?}", SecretError).contains("mock"));
    }
}
