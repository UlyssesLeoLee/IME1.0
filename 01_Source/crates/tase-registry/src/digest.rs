//! The content digest of an asset (design §3.2, §19.1).
//!
//! The digest of an asset is the SHA-256 of its id, its version and its body. The three parts are framed so
//! that an id can never be read as a part of the body:
//!
//! ```text
//! sha256( id || 0x00 || version || 0x00 || body )
//! ```
//!
//! The digest is written as `sha256:` followed by 64 lower case hex digits.
//!
//! The digest is **not** stored inside the manifest it describes: design §3.2 keeps the digest in the external
//! metadata of the registry, so that the digest never hashes itself. The loader of the registry therefore
//! computes it over the bytes of the manifest and of the knowledge card and writes it into
//! [`crate::manifest::AssetEntry::digest`].

use pime_core::sha256::{sha256, to_hex};

/// The prefix of every digest of the registry.
pub const DIGEST_PREFIX: &str = "sha256:";

/// The number of hex digits of a SHA-256 digest.
pub const DIGEST_HEX_LEN: usize = 64;

/// The framed bytes that are hashed: the id, a zero byte, the version, a zero byte and the body.
fn framed(id: &str, version: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(id.len() + version.len() + body.len() + 2);
    bytes.extend_from_slice(id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(version.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(body);
    bytes
}

/// The digest of the content of an asset: `sha256:` and 64 lower case hex digits.
pub fn asset_digest(id: &str, version: &str, body: &[u8]) -> String {
    format!(
        "{DIGEST_PREFIX}{}",
        to_hex(&sha256(&framed(id, version, body)))
    )
}

/// Whether the text is the shape of a digest: the prefix and 64 lower case hex digits.
pub fn is_digest(text: &str) -> bool {
    match text.strip_prefix(DIGEST_PREFIX) {
        Some(hex) => is_hex64_lower(hex),
        None => false,
    }
}

/// Whether the text is 64 hex digits in lower case.
pub fn is_hex64_lower(text: &str) -> bool {
    text.len() == DIGEST_HEX_LEN
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether the text is 64 hex digits, in any case.
pub fn is_hex64(text: &str) -> bool {
    text.len() == DIGEST_HEX_LEN && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The hex part of a digest: the prefix is optional, the 64 hex digits are not.
///
/// None when the text is not a digest: a wrong length or a character that is not a hex digit.
pub fn hex_of(text: &str) -> Option<&str> {
    let hex = text.strip_prefix(DIGEST_PREFIX).unwrap_or(text);
    if is_hex64(hex) { Some(hex) } else { None }
}

/// Whether the expected digest is the digest of the content.
///
/// The expected text may carry the `sha256:` prefix or not, but it must hold 64 lower case hex digits. A text
/// that is not a digest (a wrong length, a character that is not a hex digit, an upper case digest) is never
/// the digest of anything: the answer is `false` and nothing panics.
pub fn verify_digest(expected: &str, id: &str, version: &str, body: &[u8]) -> bool {
    let hex = expected.strip_prefix(DIGEST_PREFIX).unwrap_or(expected);
    if !is_hex64_lower(hex) {
        return false;
    }
    hex == to_hex(&sha256(&framed(id, version, body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "algorithm.graph.kruskal";
    const VERSION: &str = "1.0.0";
    const BODY: &[u8] = b"the body of the asset";

    #[test]
    fn the_same_content_always_has_the_same_digest() {
        let first = asset_digest(ID, VERSION, BODY);
        let second = asset_digest(ID, VERSION, BODY);
        assert_eq!(first, second);
    }

    #[test]
    fn one_changed_byte_changes_the_digest() {
        let before = asset_digest(ID, VERSION, BODY);
        let mut body = BODY.to_vec();
        body[0] = b'T';
        let after = asset_digest(ID, VERSION, &body);
        assert_ne!(before, after);
    }

    #[test]
    fn the_id_and_the_version_are_part_of_the_digest() {
        let base = asset_digest(ID, VERSION, BODY);
        assert_ne!(base, asset_digest("algorithm.graph.prim", VERSION, BODY));
        assert_ne!(base, asset_digest(ID, "1.0.1", BODY));
    }

    #[test]
    fn a_digest_has_the_prefix_and_64_lower_case_hex_digits() {
        let digest = asset_digest(ID, VERSION, BODY);
        assert!(digest.starts_with(DIGEST_PREFIX));
        assert_eq!(digest.len(), DIGEST_PREFIX.len() + DIGEST_HEX_LEN);
        assert!(is_digest(&digest));
        assert!(is_hex64_lower(hex_of(&digest).unwrap()));
        // the hex part is lower case only
        assert!(digest[ DIGEST_PREFIX.len()..].bytes().all(|b| !b.is_ascii_uppercase()));
    }

    #[test]
    fn a_digest_that_is_not_a_digest_is_refused_without_panicking() {
        // a wrong length
        assert!(!verify_digest("sha256:abc", ID, VERSION, BODY));
        assert!(!verify_digest("", ID, VERSION, BODY));
        // a character that is not a hex digit
        let mut wrong = asset_digest(ID, VERSION, BODY);
        wrong.replace_range(DIGEST_PREFIX.len()..DIGEST_PREFIX.len() + 1, "z");
        assert!(!verify_digest(&wrong, ID, VERSION, BODY));
        // an upper case digest is not the shape the registry writes
        let upper = asset_digest(ID, VERSION, BODY).to_uppercase();
        assert!(!is_digest(&upper));
        assert!(!verify_digest(&upper, ID, VERSION, BODY));
        // the prefix alone
        assert!(!verify_digest(DIGEST_PREFIX, ID, VERSION, BODY));
    }

    #[test]
    fn a_digest_is_verified_against_the_content_it_was_made_of() {
        let digest = asset_digest(ID, VERSION, BODY);
        assert!(verify_digest(&digest, ID, VERSION, BODY));
        // the bare hex part is accepted as well
        let hex = hex_of(&digest).unwrap();
        assert!(verify_digest(hex, ID, VERSION, BODY));
        // another content, another id or another version does not match
        assert!(!verify_digest(&digest, ID, VERSION, b"another body"));
        assert!(!verify_digest(&digest, "algorithm.graph.prim", VERSION, BODY));
        assert!(!verify_digest(&digest, ID, "2.0.0", BODY));
    }

    #[test]
    fn the_hex_of_a_text_that_is_not_a_digest_is_none() {
        assert!(hex_of("sha256:abc").is_none());
        assert!(hex_of("not a digest").is_none());
        assert!(hex_of(&format!("{}{}", DIGEST_PREFIX, "0".repeat(64))).is_some());
    }
}
