//! Operating-system error codes -> the worker's stable tokens (pure, so it is unit-tested without Windows).
//!
//! The numbers are the documented WinHTTP error codes (`ERROR_WINHTTP_*`). The transport passes the Win32 code and, for
//! a TLS failure, what the WinHTTP status callback reported; this module never sees a message text.
#![forbid(unsafe_code)]

use crate::error::{Code, Failure};

pub const ERROR_WINHTTP_TIMEOUT: u32 = 12002;
pub const ERROR_WINHTTP_INTERNAL_ERROR: u32 = 12004;
pub const ERROR_WINHTTP_INVALID_URL: u32 = 12005;
pub const ERROR_WINHTTP_UNRECOGNIZED_SCHEME: u32 = 12006;
pub const ERROR_WINHTTP_NAME_NOT_RESOLVED: u32 = 12007;
pub const ERROR_WINHTTP_OPERATION_CANCELLED: u32 = 12017;
pub const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
pub const ERROR_WINHTTP_CONNECTION_ERROR: u32 = 12030;
pub const ERROR_WINHTTP_SECURE_CERT_DATE_INVALID: u32 = 12037;
pub const ERROR_WINHTTP_SECURE_CERT_CN_INVALID: u32 = 12038;
pub const ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED: u32 = 12044;
pub const ERROR_WINHTTP_SECURE_INVALID_CA: u32 = 12045;
pub const ERROR_WINHTTP_SECURE_CERT_REV_FAILED: u32 = 12057;
pub const ERROR_WINHTTP_REDIRECT_FAILED: u32 = 12156;
pub const ERROR_WINHTTP_SECURE_CHANNEL_ERROR: u32 = 12157;
pub const ERROR_WINHTTP_BAD_AUTO_PROXY_SCRIPT: u32 = 12166;
pub const ERROR_WINHTTP_UNABLE_TO_DOWNLOAD_SCRIPT: u32 = 12167;
pub const ERROR_WINHTTP_SECURE_INVALID_CERT: u32 = 12169;
pub const ERROR_WINHTTP_SECURE_CERT_REVOKED: u32 = 12170;
pub const ERROR_WINHTTP_SECURE_FAILURE: u32 = 12175;
pub const ERROR_WINHTTP_SCRIPT_EXECUTION_ERROR: u32 = 12177;
pub const ERROR_WINHTTP_AUTO_PROXY_SERVICE_ERROR: u32 = 12178;
pub const ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE: u32 = 12179;
pub const ERROR_WINHTTP_AUTODETECTION_FAILED: u32 = 12180;
pub const ERROR_WINHTTP_HEADER_COUNT_EXCEEDED: u32 = 12181;
pub const ERROR_WINHTTP_HEADER_SIZE_OVERFLOW: u32 = 12182;
pub const ERROR_WINHTTP_CHUNKED_ENCODING_HEADER_SIZE_OVERFLOW: u32 = 12183;
pub const ERROR_WINHTTP_RESPONSE_DRAIN_OVERFLOW: u32 = 12184;
pub const ERROR_WINHTTP_INVALID_SERVER_RESPONSE: u32 = 12152;

// WINHTTP_CALLBACK_STATUS_FLAG_*: the reasons a status callback reports for a TLS failure
pub const FLAG_CERT_REV_FAILED: u32 = 0x0000_0001;
pub const FLAG_INVALID_CERT: u32 = 0x0000_0002;
pub const FLAG_CERT_REVOKED: u32 = 0x0000_0004;
pub const FLAG_INVALID_CA: u32 = 0x0000_0008;
pub const FLAG_CERT_CN_INVALID: u32 = 0x0000_0010;
pub const FLAG_CERT_DATE_INVALID: u32 = 0x0000_0020;
pub const FLAG_CERT_WRONG_USAGE: u32 = 0x0000_0040;
pub const FLAG_SECURITY_CHANNEL_ERROR: u32 = 0x8000_0000;

/// What the TLS layer reported through the status callback before the call failed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecureFailure {
    pub flags: u32,
    /// `Some(true)` when the server certificate's subject equals its issuer (a self-signed leaf), `None` when the
    /// certificate could not be inspected.
    pub self_issued: Option<bool>,
}

/// The token for a TLS failure. The order puts "who vouches for this certificate" first, because that is what a user
/// can act on (an untrusted root, typically a corporate TLS-inspection proxy, versus a wrong host name or a date).
pub fn secure_failure_code(f: SecureFailure) -> Code {
    if f.flags & FLAG_INVALID_CA != 0 {
        return if f.self_issued == Some(true) {
            Code::DepthZeroSelfSignedCert
        } else {
            Code::CertUntrusted
        };
    }
    if f.flags & FLAG_CERT_CN_INVALID != 0 {
        return Code::CertHostnameMismatch;
    }
    if f.flags & FLAG_CERT_DATE_INVALID != 0 {
        return Code::CertExpired;
    }
    if f.flags & FLAG_CERT_REVOKED != 0 {
        return Code::CertRevoked;
    }
    if f.flags & FLAG_CERT_REV_FAILED != 0 {
        return Code::CertRevocationUnknown;
    }
    if f.flags & (FLAG_INVALID_CERT | FLAG_CERT_WRONG_USAGE) != 0 {
        return Code::CertInvalid;
    }
    Code::TlsFailed
}

/// Maps a Win32/WinHTTP error code to a failure. `secure` is what the status callback captured for this request, if
/// anything. Unknown codes become `TRANSPORT_ERROR` carrying the numeric code (a number cannot leak text).
pub fn map_win32_error(code: u32, secure: Option<SecureFailure>) -> Failure {
    let token = match code {
        ERROR_WINHTTP_TIMEOUT => Code::Timeout,
        ERROR_WINHTTP_OPERATION_CANCELLED => Code::Cancelled,
        ERROR_WINHTTP_NAME_NOT_RESOLVED => Code::Dns,
        ERROR_WINHTTP_CANNOT_CONNECT => Code::ConnectFailed,
        ERROR_WINHTTP_CONNECTION_ERROR => Code::ConnectionReset,
        ERROR_WINHTTP_SECURE_FAILURE => match secure {
            Some(s) => secure_failure_code(s),
            None => Code::TlsFailed,
        },
        ERROR_WINHTTP_SECURE_INVALID_CA
        | ERROR_WINHTTP_SECURE_CERT_CN_INVALID
        | ERROR_WINHTTP_SECURE_CERT_DATE_INVALID
        | ERROR_WINHTTP_SECURE_CERT_REVOKED
        | ERROR_WINHTTP_SECURE_CERT_REV_FAILED
        | ERROR_WINHTTP_SECURE_INVALID_CERT
        | ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE => {
            let own = match code {
                ERROR_WINHTTP_SECURE_INVALID_CA => FLAG_INVALID_CA,
                ERROR_WINHTTP_SECURE_CERT_CN_INVALID => FLAG_CERT_CN_INVALID,
                ERROR_WINHTTP_SECURE_CERT_DATE_INVALID => FLAG_CERT_DATE_INVALID,
                ERROR_WINHTTP_SECURE_CERT_REVOKED => FLAG_CERT_REVOKED,
                ERROR_WINHTTP_SECURE_CERT_REV_FAILED => FLAG_CERT_REV_FAILED,
                ERROR_WINHTTP_SECURE_INVALID_CERT => FLAG_INVALID_CERT,
                _ => FLAG_CERT_WRONG_USAGE,
            };
            let s = secure.unwrap_or_default();
            secure_failure_code(SecureFailure {
                flags: s.flags | own,
                self_issued: s.self_issued,
            })
        }
        ERROR_WINHTTP_SECURE_CHANNEL_ERROR | ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED => {
            Code::TlsFailed
        }
        ERROR_WINHTTP_AUTODETECTION_FAILED
        | ERROR_WINHTTP_AUTO_PROXY_SERVICE_ERROR
        | ERROR_WINHTTP_BAD_AUTO_PROXY_SCRIPT
        | ERROR_WINHTTP_UNABLE_TO_DOWNLOAD_SCRIPT
        | ERROR_WINHTTP_SCRIPT_EXECUTION_ERROR => Code::ProxyError,
        ERROR_WINHTTP_REDIRECT_FAILED => Code::RedirectBlocked,
        ERROR_WINHTTP_INVALID_SERVER_RESPONSE
        | ERROR_WINHTTP_HEADER_COUNT_EXCEEDED
        | ERROR_WINHTTP_HEADER_SIZE_OVERFLOW
        | ERROR_WINHTTP_CHUNKED_ENCODING_HEADER_SIZE_OVERFLOW
        | ERROR_WINHTTP_RESPONSE_DRAIN_OVERFLOW => Code::BadResponse,
        ERROR_WINHTTP_INVALID_URL | ERROR_WINHTTP_UNRECOGNIZED_SCHEME => Code::UrlNotAllowed,
        _ => {
            let mut f = Failure::new(Code::TransportError);
            f.os_code = Some(code);
            return f;
        }
    };
    let mut f = Failure::new(token);
    if token == Code::TlsFailed {
        f.os_code = Some(code);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(win: u32, secure: Option<SecureFailure>) -> Code {
        map_win32_error(win, secure).code
    }

    #[test]
    fn plain_transport_errors() {
        assert_eq!(code_of(ERROR_WINHTTP_TIMEOUT, None), Code::Timeout);
        assert_eq!(
            code_of(ERROR_WINHTTP_OPERATION_CANCELLED, None),
            Code::Cancelled
        );
        assert_eq!(code_of(ERROR_WINHTTP_NAME_NOT_RESOLVED, None), Code::Dns);
        assert_eq!(
            code_of(ERROR_WINHTTP_CANNOT_CONNECT, None),
            Code::ConnectFailed
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_CONNECTION_ERROR, None),
            Code::ConnectionReset
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_AUTODETECTION_FAILED, None),
            Code::ProxyError
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_BAD_AUTO_PROXY_SCRIPT, None),
            Code::ProxyError
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_INVALID_SERVER_RESPONSE, None),
            Code::BadResponse
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_RESPONSE_DRAIN_OVERFLOW, None),
            Code::BadResponse
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_REDIRECT_FAILED, None),
            Code::RedirectBlocked
        );
    }

    #[test]
    fn an_unknown_code_is_reported_by_number_only() {
        let f = map_win32_error(5, None);
        assert_eq!((f.code, f.os_code), (Code::TransportError, Some(5)));
        let f = map_win32_error(ERROR_WINHTTP_INTERNAL_ERROR, None);
        assert_eq!((f.code, f.os_code), (Code::TransportError, Some(12004)));
    }

    #[test]
    fn a_self_signed_leaf_is_named_like_the_node_error_users_know() {
        let s = SecureFailure {
            flags: FLAG_INVALID_CA,
            self_issued: Some(true),
        };
        assert_eq!(
            code_of(ERROR_WINHTTP_SECURE_FAILURE, Some(s)),
            Code::DepthZeroSelfSignedCert
        );
    }

    #[test]
    fn an_untrusted_root_that_is_not_self_signed_is_not_called_self_signed() {
        for self_issued in [Some(false), None] {
            let s = SecureFailure {
                flags: FLAG_INVALID_CA,
                self_issued,
            };
            assert_eq!(
                code_of(ERROR_WINHTTP_SECURE_FAILURE, Some(s)),
                Code::CertUntrusted
            );
        }
        assert_eq!(
            code_of(ERROR_WINHTTP_SECURE_INVALID_CA, None),
            Code::CertUntrusted
        );
    }

    #[test]
    fn certificate_problems_have_their_own_tokens() {
        for (flags, want) in [
            (FLAG_CERT_CN_INVALID, Code::CertHostnameMismatch),
            (FLAG_CERT_DATE_INVALID, Code::CertExpired),
            (FLAG_CERT_REVOKED, Code::CertRevoked),
            (FLAG_CERT_REV_FAILED, Code::CertRevocationUnknown),
            (FLAG_INVALID_CERT, Code::CertInvalid),
            (FLAG_CERT_WRONG_USAGE, Code::CertInvalid),
            (FLAG_SECURITY_CHANNEL_ERROR, Code::TlsFailed),
            (0, Code::TlsFailed),
        ] {
            let s = SecureFailure {
                flags,
                self_issued: None,
            };
            assert_eq!(
                code_of(ERROR_WINHTTP_SECURE_FAILURE, Some(s)),
                want,
                "{flags:#x}"
            );
        }
        assert_eq!(code_of(ERROR_WINHTTP_SECURE_FAILURE, None), Code::TlsFailed);
        assert_eq!(
            code_of(ERROR_WINHTTP_SECURE_CERT_CN_INVALID, None),
            Code::CertHostnameMismatch
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_SECURE_CERT_DATE_INVALID, None),
            Code::CertExpired
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_SECURE_CERT_REVOKED, None),
            Code::CertRevoked
        );
        assert_eq!(
            code_of(ERROR_WINHTTP_SECURE_CHANNEL_ERROR, None),
            Code::TlsFailed
        );
    }

    #[test]
    fn the_untrusted_authority_outranks_the_other_flags() {
        let s = SecureFailure {
            flags: FLAG_INVALID_CA | FLAG_CERT_CN_INVALID | FLAG_CERT_DATE_INVALID,
            self_issued: Some(true),
        };
        assert_eq!(secure_failure_code(s), Code::DepthZeroSelfSignedCert);
        let s = SecureFailure {
            flags: FLAG_CERT_CN_INVALID | FLAG_CERT_DATE_INVALID,
            self_issued: None,
        };
        assert_eq!(secure_failure_code(s), Code::CertHostnameMismatch);
    }

    #[test]
    fn mapped_failures_carry_no_text() {
        let f = map_win32_error(ERROR_WINHTTP_SECURE_FAILURE, None);
        assert_eq!(f.field, None);
        assert_eq!(f.os_code, Some(ERROR_WINHTTP_SECURE_FAILURE));
    }
}
