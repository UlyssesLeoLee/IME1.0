//! The credential store seam. The real implementation is `wincred::WinCredVault` (Windows Credential Manager).
//!
//! `read` is deliberately not reachable from the protocol: no operation returns a key. It exists for the engine, which
//! injects the key into one request and wipes it.
#![forbid(unsafe_code)]

use crate::secret::Secret;
use std::collections::HashMap;
use std::sync::Mutex;

/// The store failed (access denied, service unavailable, unreadable entry...). It carries the numeric Win32 error code
/// (0 = none) and no text on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultError(pub u32);

/// A stored credential: the key and the origin (`host:port`) it may be sent to.
pub struct Stored {
    pub secret: Secret,
    pub origin: String,
}

pub trait Vault: Send + Sync {
    /// Creates or replaces the credential `target`.
    fn set(&self, target: &str, origin: &str, secret: &Secret) -> Result<(), VaultError>;
    /// The credential, or `None` when there is none.
    fn read(&self, target: &str) -> Result<Option<Stored>, VaultError>;
    /// Whether a credential exists and the origin it is bound to; the key is not kept.
    fn exists(&self, target: &str) -> Result<Option<String>, VaultError> {
        Ok(self.read(target)?.map(|s| s.origin))
    }
    /// Removes the credential; `false` when there was none.
    fn delete(&self, target: &str) -> Result<bool, VaultError>;
}

/// An in-memory store for tests (and nothing else: it is the only vault that automated Rust tests may use).
#[derive(Default)]
pub struct MemVault {
    entries: Mutex<HashMap<String, (Vec<u8>, String)>>,
    fail: Mutex<bool>,
}

impl MemVault {
    pub fn new() -> MemVault {
        MemVault::default()
    }

    /// Makes every following call fail, like a locked-down or broken store.
    pub fn set_failing(&self, failing: bool) {
        *self.fail.lock().unwrap() = failing;
    }

    pub fn targets(&self) -> Vec<String> {
        let mut t: Vec<String> = self.entries.lock().unwrap().keys().cloned().collect();
        t.sort();
        t
    }

    fn check(&self) -> Result<(), VaultError> {
        if *self.fail.lock().unwrap() {
            Err(VaultError(0))
        } else {
            Ok(())
        }
    }
}

impl Vault for MemVault {
    fn set(&self, target: &str, origin: &str, secret: &Secret) -> Result<(), VaultError> {
        self.check()?;
        self.entries.lock().unwrap().insert(
            target.to_string(),
            (secret.expose().to_vec(), origin.to_string()),
        );
        Ok(())
    }

    fn read(&self, target: &str) -> Result<Option<Stored>, VaultError> {
        self.check()?;
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(target)
            .map(|(bytes, origin)| Stored {
                secret: Secret::from_bytes(bytes.clone()),
                origin: origin.clone(),
            }))
    }

    fn delete(&self, target: &str) -> Result<bool, VaultError> {
        self.check()?;
        Ok(self.entries.lock().unwrap().remove(target).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Secret {
        Secret::parse("mock-key-test-123".to_string()).unwrap()
    }

    #[test]
    fn the_memory_vault_round_trips_and_deletes() {
        let v = MemVault::new();
        assert!(v.read("ns/openai").unwrap().is_none());
        assert_eq!(v.exists("ns/openai").unwrap(), None);
        v.set("ns/openai", "api.openai.com:443", &key()).unwrap();
        let stored = v.read("ns/openai").unwrap().unwrap();
        assert_eq!(stored.secret.expose(), b"mock-key-test-123");
        assert_eq!(stored.origin, "api.openai.com:443");
        assert_eq!(v.exists("ns/openai").unwrap().as_deref(), Some("api.openai.com:443"));
        assert_eq!(v.targets(), vec!["ns/openai".to_string()]);
        assert!(v.delete("ns/openai").unwrap());
        assert!(!v.delete("ns/openai").unwrap());
        assert!(v.targets().is_empty());
    }

    #[test]
    fn a_failing_store_reports_an_error_without_text() {
        let v = MemVault::new();
        v.set_failing(true);
        assert_eq!(v.set("ns/x", "h:443", &key()), Err(VaultError(0)));
        assert!(v.read("ns/x").is_err());
        assert!(v.exists("ns/x").is_err());
        assert!(v.delete("ns/x").is_err());
        assert_eq!(format!("{:?}", VaultError(5)), "VaultError(5)");
    }

    #[test]
    fn set_replaces_an_existing_credential() {
        let v = MemVault::new();
        v.set("ns/p", "a.example:443", &key()).unwrap();
        let other = Secret::parse("another-key-value-9".to_string()).unwrap();
        v.set("ns/p", "b.example:443", &other).unwrap();
        let s = v.read("ns/p").unwrap().unwrap();
        assert_eq!((s.secret.expose(), s.origin.as_str()), (b"another-key-value-9".as_slice(), "b.example:443"));
    }
}
