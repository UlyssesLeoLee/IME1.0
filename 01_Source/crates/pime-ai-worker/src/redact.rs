//! Redaction: the last line of defence for text that leaves the worker.
//!
//! Two different tools, used in different places:
//! - `scrub_exact` removes the known key from provider output (a provider may echo the key in an error message);
//! - `redact_key_shaped` masks anything that merely *looks* like a credential, for the free-form diagnostics the worker
//!   itself writes (stderr). The worker has no log file, and the diagnostics path is the only place that uses it.

pub const MASK: &str = "[redacted]";

/// Replaces every occurrence of `secret` in `haystack` with [`MASK`]. Returns the new bytes and the number of
/// replacements. An empty secret never matches (it would match everywhere).
pub fn scrub_exact(haystack: &[u8], secret: &[u8]) -> (Vec<u8>, usize) {
    if secret.is_empty() || haystack.len() < secret.len() {
        return (haystack.to_vec(), 0);
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut count = 0;
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(secret) {
            out.extend_from_slice(MASK.as_bytes());
            i += secret.len();
            count += 1;
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    (out, count)
}

const SECRET_WORDS: [&str; 8] = [
    "authorization",
    "x-api-key",
    "api-key",
    "api_key",
    "apikey",
    "secret",
    "password",
    "bearer",
];

const KNOWN_PREFIXES: [&str; 8] = ["sk-", "sk_", "ghp_", "gho_", "xox", "AKIA", "eyJ", "AIza"];

fn is_token_char(c: char) -> bool {
    // `=` is a separator here (`api_key=value`); base64 padding is left behind when the token itself is masked
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/')
}

fn looks_like_key(token: &str) -> bool {
    if token.len() >= 12 && KNOWN_PREFIXES.iter().any(|p| token.starts_with(p)) {
        return true;
    }
    token.len() >= 20
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().any(|c| c.is_ascii_alphabetic())
}

/// Masks key-shaped text: values after a credential-looking word (`Authorization: ...`, `api_key=...`, `Bearer ...`),
/// well-known key prefixes and long mixed letter/digit tokens. It errs on the side of masking: an ordinary long
/// identifier with digits is masked too, which is acceptable for diagnostics.
pub fn redact_key_shaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut mask_next_token = false;
    while !rest.is_empty() {
        let c = rest.chars().next().unwrap();
        if is_token_char(c) {
            let end = rest
                .find(|ch: char| !is_token_char(ch))
                .unwrap_or(rest.len());
            let token = &rest[..end];
            if SECRET_WORDS.contains(&token.to_ascii_lowercase().as_str()) {
                // the word stays visible, its value (the next token) does not: `Authorization: Bearer <key>`
                out.push_str(token);
                mask_next_token = true;
            } else if mask_next_token || looks_like_key(token) {
                out.push_str(MASK);
                mask_next_token = false;
            } else {
                out.push_str(token);
            }
            rest = &rest[end..];
        } else {
            // separators between a credential word and its value (`: `, `=`, a space, quotes) keep the flag alive
            if mask_next_token && !matches!(c, ':' | '=' | ' ' | '"' | '\'' | '\t') {
                mask_next_token = false;
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The one way the worker writes a line to stderr: prefixed, one line, key-shaped text masked. Every message passed in
/// is fixed text today; the masking is the guard for the day someone formats a value into one.
pub fn diagnostic(message: &str) -> String {
    let one_line = message.replace(['\r', '\n'], " ");
    format!("pime-ai-worker: {}", redact_key_shaped(&one_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_are_one_line_prefixed_and_masked() {
        let line = diagnostic("failed with sk-proj-AbCdEf0123456789AbCdEf\nsecond line");
        assert_eq!(line, "pime-ai-worker: failed with [redacted] second line");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn scrub_exact_replaces_every_occurrence_and_counts_them() {
        let (out, n) = scrub_exact(
            b"Invalid key mock-key-test-123 (again: mock-key-test-123)",
            b"mock-key-test-123",
        );
        assert_eq!(n, 2);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Invalid key [redacted] (again: [redacted])"
        );
    }

    #[test]
    fn scrub_exact_handles_edges() {
        assert_eq!(scrub_exact(b"", b"abcdefgh"), (Vec::new(), 0));
        assert_eq!(scrub_exact(b"abc", b"abcdefgh"), (b"abc".to_vec(), 0));
        assert_eq!(scrub_exact(b"abc", b""), (b"abc".to_vec(), 0));
        let (out, n) = scrub_exact(b"abcdefghabcdefgh", b"abcdefgh");
        assert_eq!((out.as_slice(), n), (b"[redacted][redacted]".as_slice(), 2));
        // overlapping candidates: the first match wins, the remainder is scanned normally
        let (out, n) = scrub_exact(b"aaaaaaaaa", b"aaaaaaaa");
        assert_eq!(n, 1);
        assert_eq!(out, b"[redacted]a");
    }

    #[test]
    fn scrub_exact_works_on_non_utf8_bodies() {
        let mut body = vec![0xff, 0xfe, 0x00];
        body.extend_from_slice(b"mock-key-test-123");
        body.push(0x80);
        let (out, n) = scrub_exact(&body, b"mock-key-test-123");
        assert_eq!(n, 1);
        assert_eq!(&out[..3], &[0xff, 0xfe, 0x00]);
        assert_eq!(out.last(), Some(&0x80));
    }

    #[test]
    fn key_shaped_values_after_credential_words_are_masked() {
        assert_eq!(
            redact_key_shaped("Authorization: Bearer abc123"),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(
            redact_key_shaped("x-api-key: shortv"),
            "x-api-key: [redacted]"
        );
        assert_eq!(redact_key_shaped("api_key=hunter2"), "api_key=[redacted]");
        assert_eq!(
            redact_key_shaped("password \"p4ss\" rest"),
            "password \"[redacted]\" rest"
        );
    }

    #[test]
    fn well_known_prefixes_and_long_mixed_tokens_are_masked() {
        for key in [
            "sk-proj-AbCdEf0123456789AbCdEf",
            "sk-ant-api03-AAAAAAAAAAAAAAAA",
            "ghp_0123456789abcdefABCDEF012345",
            "AKIAIOSFODNN7EXAMPLE",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcDEF123",
            "mock-key-test-1234567890",
        ] {
            let out = redact_key_shaped(&format!("failed with {key} at start"));
            assert_eq!(out, "failed with [redacted] at start", "{key}");
            assert!(!out.contains(key));
        }
    }

    #[test]
    fn ordinary_text_survives() {
        let text = "request 12 failed: connect error on port 443 (code 12029)";
        assert_eq!(redact_key_shaped(text), text);
        assert_eq!(redact_key_shaped(""), "");
        assert_eq!(
            redact_key_shaped("token budget exceeded"),
            "token budget exceeded"
        );
    }

    #[test]
    fn non_ascii_text_is_not_split_inside_a_character() {
        let text = "caf\u{e9} \u{3053}\u{3093}\u{306b}\u{3061}\u{306f} key=abc";
        assert_eq!(
            redact_key_shaped(text),
            "caf\u{e9} \u{3053}\u{3093}\u{306b}\u{3061}\u{306f} key=abc"
        );
    }
}
