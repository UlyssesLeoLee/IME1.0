//! The stable, safe error vocabulary of the worker.
//!
//! Every failure that leaves the worker is one of these tokens (the `error` field of a reply). A token is a fixed
//! string chosen by this crate: it never contains text from a request, a response, a certificate or the operating
//! system, so it can never carry a key, a request body or a host name.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    // framing and request shape (same names and meaning as in pime-core)
    InvalidJson,
    InvalidRequest,
    UnknownOperation,
    PayloadTooLarge,
    Internal,
    // worker policy
    Busy,
    UrlNotAllowed,
    HeaderNotAllowed,
    NoCredential,
    CredentialOriginMismatch,
    VaultError,
    // transport
    Timeout,
    Cancelled,
    Dns,
    ConnectFailed,
    ConnectionReset,
    DepthZeroSelfSignedCert,
    CertUntrusted,
    CertHostnameMismatch,
    CertExpired,
    CertRevoked,
    CertRevocationUnknown,
    CertInvalid,
    TlsFailed,
    ProxyError,
    RedirectBlocked,
    ResponseTooLarge,
    BadResponse,
    TransportError,
}

impl Code {
    pub const fn as_str(self) -> &'static str {
        match self {
            Code::InvalidJson => "INVALID_JSON",
            Code::InvalidRequest => "INVALID_REQUEST",
            Code::UnknownOperation => "UNKNOWN_OPERATION",
            Code::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Code::Internal => "INTERNAL",
            Code::Busy => "BUSY",
            Code::UrlNotAllowed => "URL_NOT_ALLOWED",
            Code::HeaderNotAllowed => "HEADER_NOT_ALLOWED",
            Code::NoCredential => "NO_CREDENTIAL",
            Code::CredentialOriginMismatch => "CREDENTIAL_ORIGIN_MISMATCH",
            Code::VaultError => "VAULT_ERROR",
            Code::Timeout => "TIMEOUT",
            Code::Cancelled => "CANCELLED",
            Code::Dns => "DNS",
            Code::ConnectFailed => "CONNECT_FAILED",
            Code::ConnectionReset => "CONNECTION_RESET",
            Code::DepthZeroSelfSignedCert => "DEPTH_ZERO_SELF_SIGNED_CERT",
            Code::CertUntrusted => "CERT_UNTRUSTED",
            Code::CertHostnameMismatch => "CERT_HOSTNAME_MISMATCH",
            Code::CertExpired => "CERT_EXPIRED",
            Code::CertRevoked => "CERT_REVOKED",
            Code::CertRevocationUnknown => "CERT_REVOCATION_UNKNOWN",
            Code::CertInvalid => "CERT_INVALID",
            Code::TlsFailed => "TLS_FAILED",
            Code::ProxyError => "PROXY_ERROR",
            Code::RedirectBlocked => "REDIRECT_BLOCKED",
            Code::ResponseTooLarge => "RESPONSE_TOO_LARGE",
            Code::BadResponse => "BAD_RESPONSE",
            Code::TransportError => "TRANSPORT_ERROR",
        }
    }

    pub const ALL: [Code; 29] = [
        Code::InvalidJson,
        Code::InvalidRequest,
        Code::UnknownOperation,
        Code::PayloadTooLarge,
        Code::Internal,
        Code::Busy,
        Code::UrlNotAllowed,
        Code::HeaderNotAllowed,
        Code::NoCredential,
        Code::CredentialOriginMismatch,
        Code::VaultError,
        Code::Timeout,
        Code::Cancelled,
        Code::Dns,
        Code::ConnectFailed,
        Code::ConnectionReset,
        Code::DepthZeroSelfSignedCert,
        Code::CertUntrusted,
        Code::CertHostnameMismatch,
        Code::CertExpired,
        Code::CertRevoked,
        Code::CertRevocationUnknown,
        Code::CertInvalid,
        Code::TlsFailed,
        Code::ProxyError,
        Code::RedirectBlocked,
        Code::ResponseTooLarge,
        Code::BadResponse,
        Code::TransportError,
    ];
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failed operation as it is reported to the caller: a token, optionally the name of the offending request field
/// (only names from this crate or short identifier-like names, see `Failure::invalid`) and, for transport errors,
/// the numeric operating-system error code (a number cannot carry text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: Code,
    pub field: Option<String>,
    pub os_code: Option<u32>,
}

impl Failure {
    pub fn new(code: Code) -> Failure {
        Failure {
            code,
            field: None,
            os_code: None,
        }
    }

    /// `INVALID_REQUEST` naming a field. A field name that came from the caller is echoed only when it is a short
    /// identifier; anything else (for example a key pasted as a field name) is reported as `?`.
    pub fn invalid(field: &str) -> Failure {
        Failure::with_field(Code::InvalidRequest, field)
    }

    pub fn with_field(code: Code, field: &str) -> Failure {
        let safe = !field.is_empty()
            && field.len() <= 24
            && field.starts_with(|c: char| c.is_ascii_alphabetic())
            && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        Failure {
            code,
            field: Some(if safe { field } else { "?" }.to_string()),
            os_code: None,
        }
    }
}

impl From<Code> for Failure {
    fn from(code: Code) -> Failure {
        Failure::new(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_upper_snake_case_and_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for c in Code::ALL {
            let s = c.as_str();
            assert!(
                s.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_'),
                "{s}"
            );
            assert!(seen.insert(s), "duplicate token {s}");
        }
        assert_eq!(seen.len(), 29);
    }

    #[test]
    fn caller_supplied_field_names_are_only_echoed_when_harmless() {
        assert_eq!(
            Failure::invalid("timeoutMs").field.as_deref(),
            Some("timeoutMs")
        );
        assert_eq!(
            Failure::invalid("sk-proj-abcdefghijklmnop1234")
                .field
                .as_deref(),
            Some("?")
        );
        assert_eq!(Failure::invalid("has space").field.as_deref(), Some("?"));
        assert_eq!(Failure::invalid("").field.as_deref(), Some("?"));
        assert_eq!(
            Failure::invalid("A234567890123456789012345")
                .field
                .as_deref(),
            Some("?"),
            "25 characters is too long to be a field name"
        );
    }
}
