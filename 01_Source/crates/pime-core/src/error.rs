//! Shared error model (DD section 2.2 / 12.2). Errors carry a stable code and a message that never contains
//! source text, prompts or credentials.

use std::fmt;

/// Stable error codes (mirror of `errors.js` CODES).
pub const CODES: &[&str] = &[
    "PROFILE_NOT_REGISTERED",
    "IPC_TIMEOUT",
    "SECURE_INPUT",
    "CONSENT_REQUIRED",
    "MODEL_UNSUPPORTED",
    "AI_RATE_LIMIT",
    "AI_BAD_REQUEST",
    "AI_AUTH_FAILED",
    "AI_QUOTA",
    "AI_SERVER_ERROR",
    "AI_TIMEOUT",
    "AI_NETWORK",
    "AI_BUDGET_EXCEEDED",
    "AI_DISABLED",
    "AI_CANCELLED",
    "STALE_RESULT",
    "STALE_DOCUMENT",
    "STALE_SELECTION",
    "PATCH_INVALID",
    "EDIT_UNKNOWN",
    "EDIT_REJECTED",
    "BYTE_ENCODING_UNRESOLVED",
    "VAULT_ERROR",
    "OUTPUT_INVALID",
    "TARGET_RANGE_INVALID",
    "PAYLOAD_TOO_LARGE",
    "CONTEXT_UNAVAILABLE",
    "FULL_CONTEXT_REQUIRED",
    "CROSS_FILE_DENIED",
    "VALIDATION_FAILED",
    "GRANT_DENIED",
    "TOKEN_INVALID",
    "USER_CONFIRMATION_REQUIRED",
    "SENSITIVE_BLOCKED",
    "DOCUMENT_CLOSED",
    "RULEPACK_INVALID",
    "SETTINGS_INVALID",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PimeError {
    pub code: &'static str,
    pub message: String,
}

impl PimeError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        debug_assert!(CODES.contains(&code), "unknown error code {code}");
        PimeError {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for PimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PimeError {}

pub type Result<T> = std::result::Result<T, PimeError>;

pub fn fail<T>(code: &'static str, message: impl Into<String>) -> Result<T> {
    Err(PimeError::new(code, message))
}
