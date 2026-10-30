//! Identifier generation for sessions, snapshots, proposals, plans, tokens, grants and budget tickets.
//!
//! LIMITATION (documented on purpose): the core has no entropy source of its own, because it has no third-party
//! dependencies and `std` offers no portable CSPRNG. The caller (the VS Code bridge) therefore hands over at least
//! 32 bytes of cryptographically random `entropy` in `session.open` (from `crypto.randomBytes`). Identifiers are
//! derived from it with an HMAC-SHA-256 counter construction:
//!
//! ```text
//! key   = SHA-256("pime-id-v1" || entropy)
//! id(n) = first N bytes of HMAC-SHA-256(key, domain || 0x00 || n as u64 big endian)      n = 0, 1, 2, ...
//! ```
//!
//! The counter makes ids unique inside a session (a different `domain` separates the id families), and the secret
//! key makes them unpredictable to any other process: without `entropy` the next id cannot be computed, and an
//! observed id reveals nothing about the key (HMAC is a PRF). The strength is exactly the strength of the supplied
//! entropy; a weak or leaked seed makes every id predictable. Phase P2 replaces this with the operating system
//! random generator (no caller-supplied seed).

use crate::sha256::{Sha256, hmac_sha256, sha256, to_hex};

/// Minimum accepted seed size in bytes.
pub const MIN_ENTROPY_BYTES: usize = 32;
/// Maximum accepted seed size in bytes (the seed is hashed, more than this adds nothing).
pub const MAX_ENTROPY_BYTES: usize = 512;

pub struct IdGen {
    key: [u8; 32],
    counter: u64,
}

impl IdGen {
    /// `entropy` must come from a CSPRNG; see the module documentation. Weak seeds (fewer than 8 distinct byte
    /// values, e.g. all zeros) are refused as a guard against an accidentally empty buffer.
    pub fn new(entropy: &[u8]) -> Option<IdGen> {
        if !(MIN_ENTROPY_BYTES..=MAX_ENTROPY_BYTES).contains(&entropy.len()) {
            return None;
        }
        let mut seen = [false; 256];
        for b in entropy {
            seen[*b as usize] = true;
        }
        if seen.iter().filter(|s| **s).count() < 8 {
            return None;
        }
        let mut h = Sha256::new();
        h.update(b"pime-id-v1");
        h.update(entropy);
        Some(IdGen {
            key: h.finalize(),
            counter: 0,
        })
    }

    /// A fresh identifier of `bytes` random-looking bytes (hex encoded, `2 * bytes` characters, `bytes <= 32`).
    pub fn next(&mut self, domain: &str, bytes: usize) -> String {
        debug_assert!(bytes <= 32);
        let mut msg = Vec::with_capacity(domain.len() + 9);
        msg.extend_from_slice(domain.as_bytes());
        msg.push(0);
        msg.extend_from_slice(&self.counter.to_be_bytes());
        self.counter += 1;
        to_hex(&hmac_sha256(&self.key, &msg)[..bytes.min(32)])
    }

    /// Fingerprint of the key, used only to detect that two sessions were opened with the same seed.
    pub fn fingerprint(&self) -> [u8; 32] {
        sha256(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(fill: u8) -> Vec<u8> {
        (0..32u8)
            .map(|i| i.wrapping_mul(7).wrapping_add(fill))
            .collect()
    }

    #[test]
    fn ids_are_unique_and_have_the_requested_length() {
        let mut g = IdGen::new(&seed(1)).unwrap();
        let mut seen = std::collections::HashSet::new();
        for i in 0..2000 {
            let domain = ["plan", "token", "grant"][i % 3];
            let id = g.next(domain, 16);
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(seen.insert(id), "duplicate id");
        }
        assert_eq!(g.next("grant", 12).len(), 24);
    }

    #[test]
    fn the_same_entropy_gives_the_same_sequence_and_another_seed_a_different_one() {
        let mut a = IdGen::new(&seed(1)).unwrap();
        let mut b = IdGen::new(&seed(1)).unwrap();
        let mut c = IdGen::new(&seed(2)).unwrap();
        let (x, y, z) = (a.next("plan", 16), b.next("plan", 16), c.next("plan", 16));
        assert_eq!(x, y);
        assert_ne!(x, z);
        // domains separate id families even at the same counter value
        let mut d = IdGen::new(&seed(1)).unwrap();
        assert_ne!(d.next("token", 16), x);
    }

    #[test]
    fn weak_or_short_entropy_is_refused() {
        assert!(IdGen::new(&[7u8; 32]).is_none(), "constant buffer");
        assert!(IdGen::new(&[0u8; 64]).is_none(), "all zero");
        assert!(IdGen::new(&seed(1)[..31]).is_none(), "too short");
        assert!(IdGen::new(&vec![1u8; 600]).is_none(), "too long");
        assert!(IdGen::new(&seed(1)).is_some());
    }
}
