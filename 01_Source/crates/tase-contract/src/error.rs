//! The error codes of the contract (design §4.3).
//!
//! Every failure of a TASE call is one of twelve business codes. The codes are stable, short and safe to show to a
//! user; the transport maps them to a status code (HTTP) or keeps them in `data.business_code` (JSON-RPC), but the
//! code itself never changes with the transport.
//!
//! A code also says whether trying again can change the answer: only a missing credential, a reached quota and a
//! recoverable infrastructure failure are worth a second try.

use pime_core::json::Json;
use std::fmt;

/// The business codes of TASE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The schema of the request or one of its parameters is not valid.
    InvalidArgument,
    /// There is no valid credential.
    Unauthenticated,
    /// The tenant, the repository or the branch is not allowed.
    PermissionDenied,
    /// The template, the symbol or the job does not exist.
    NotFound,
    /// The same idempotency key was sent with a different digest.
    IdempotencyMismatch,
    /// The base of the plan is not the commit that is checked out any more.
    StaleBase,
    /// The symbol or a shared dependency changed in between.
    SymbolConflict,
    /// The template or the dangerous operation was refused by the policy.
    PolicyRejected,
    /// The format, the compile or the test did not pass.
    ValidationFailed,
    /// The quota or the concurrency limit is reached.
    RateLimited,
    /// The infrastructure failed in a way that can be recovered from.
    WorkerUnavailable,
    /// An internal failure that is not classified.
    InternalError,
}

impl ErrorCode {
    /// Every code of the table of the design book.
    pub const ALL: [ErrorCode; 12] = [
        ErrorCode::InvalidArgument,
        ErrorCode::Unauthenticated,
        ErrorCode::PermissionDenied,
        ErrorCode::NotFound,
        ErrorCode::IdempotencyMismatch,
        ErrorCode::StaleBase,
        ErrorCode::SymbolConflict,
        ErrorCode::PolicyRejected,
        ErrorCode::ValidationFailed,
        ErrorCode::RateLimited,
        ErrorCode::WorkerUnavailable,
        ErrorCode::InternalError,
    ];

    /// The code on the wire (`INVALID_ARGUMENT`, `NOT_FOUND`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "INVALID_ARGUMENT",
            ErrorCode::Unauthenticated => "UNAUTHENTICATED",
            ErrorCode::PermissionDenied => "PERMISSION_DENIED",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::IdempotencyMismatch => "IDEMPOTENCY_MISMATCH",
            ErrorCode::StaleBase => "STALE_BASE",
            ErrorCode::SymbolConflict => "SYMBOL_CONFLICT",
            ErrorCode::PolicyRejected => "POLICY_REJECTED",
            ErrorCode::ValidationFailed => "VALIDATION_FAILED",
            ErrorCode::RateLimited => "RATE_LIMITED",
            ErrorCode::WorkerUnavailable => "WORKER_UNAVAILABLE",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }

    /// The status of the code in HTTP.
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::InvalidArgument => 400,
            ErrorCode::Unauthenticated => 401,
            ErrorCode::PermissionDenied => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::IdempotencyMismatch => 409,
            ErrorCode::StaleBase => 409,
            ErrorCode::SymbolConflict => 409,
            ErrorCode::PolicyRejected => 422,
            ErrorCode::ValidationFailed => 422,
            ErrorCode::RateLimited => 429,
            ErrorCode::WorkerUnavailable => 503,
            ErrorCode::InternalError => 500,
        }
    }

    /// Whether trying again can change the answer.
    ///
    /// Only the three codes of the design book that name a recovery are worth a second try: a missing credential (the
    /// client authenticates again), a reached quota (the client waits) and a recoverable infrastructure failure (the
    /// worker comes back). Everything else needs a different request, not the same one again.
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::Unauthenticated | ErrorCode::RateLimited | ErrorCode::WorkerUnavailable
        )
    }

    /// The code of a wire value; an unknown value is refused.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(code: &str) -> Option<ErrorCode> {
        match code {
            "INVALID_ARGUMENT" => Some(ErrorCode::InvalidArgument),
            "UNAUTHENTICATED" => Some(ErrorCode::Unauthenticated),
            "PERMISSION_DENIED" => Some(ErrorCode::PermissionDenied),
            "NOT_FOUND" => Some(ErrorCode::NotFound),
            "IDEMPOTENCY_MISMATCH" => Some(ErrorCode::IdempotencyMismatch),
            "STALE_BASE" => Some(ErrorCode::StaleBase),
            "SYMBOL_CONFLICT" => Some(ErrorCode::SymbolConflict),
            "POLICY_REJECTED" => Some(ErrorCode::PolicyRejected),
            "VALIDATION_FAILED" => Some(ErrorCode::ValidationFailed),
            "RATE_LIMITED" => Some(ErrorCode::RateLimited),
            "WORKER_UNAVAILABLE" => Some(ErrorCode::WorkerUnavailable),
            "INTERNAL_ERROR" => Some(ErrorCode::InternalError),
            _ => None,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ErrorCode {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ErrorCode::from_str(s)
            .ok_or_else(|| TaseError::invalid_argument(format!("unknown business code {s:?}")))
    }
}

/// A failure of a TASE call: the business code, a message for the log and, when there is one, the details of the
/// failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaseError {
    /// The business code of the failure.
    pub code: ErrorCode,
    /// What went wrong, in one line.
    pub message: String,
    /// The details of the failure (the name of the field, the job that failed, ...).
    pub details: Option<String>,
}

impl TaseError {
    /// A failure with a code and a message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        TaseError {
            code,
            message: message.into(),
            details: None,
        }
    }

    /// The schema of the request or one of its parameters is not valid.
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// There is no valid credential.
    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthenticated, message)
    }

    /// The tenant, the repository or the branch is not allowed.
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    /// The template, the symbol or the job does not exist.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// The same idempotency key was sent with a different digest.
    pub fn idempotency_mismatch(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::IdempotencyMismatch, message)
    }

    /// The base of the plan is not the commit that is checked out any more.
    pub fn stale_base(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StaleBase, message)
    }

    /// The symbol or a shared dependency changed in between.
    pub fn symbol_conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::SymbolConflict, message)
    }

    /// The template or the dangerous operation was refused by the policy.
    pub fn policy_rejected(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PolicyRejected, message)
    }

    /// The format, the compile or the test did not pass.
    pub fn validation_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ValidationFailed, message)
    }

    /// The quota or the concurrency limit is reached.
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::RateLimited, message)
    }

    /// The infrastructure failed in a way that can be recovered from.
    pub fn worker_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::WorkerUnavailable, message)
    }

    /// An internal failure that is not classified.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }

    /// Adds the details of the failure.
    pub fn with_details(self, details: impl Into<String>) -> Self {
        TaseError {
            details: Some(details.into()),
            ..self
        }
    }

    /// The failure as the JSON that goes into the body of the answer.
    ///
    /// The business code is written twice on purpose: `code` for the transports that read a flat body, and
    /// `business_code` for the JSON-RPC transports that nest it into `data` (design §4.3).
    pub fn to_json(&self) -> String {
        Json::obj(vec![
            ("code", Json::str(self.code.as_str())),
            ("message", Json::str(&self.message)),
            ("business_code", Json::str(self.code.as_str())),
            ("retryable", Json::Bool(self.code.retryable())),
            (
                "details",
                match &self.details {
                    Some(details) => Json::str(details),
                    None => Json::Null,
                },
            ),
        ])
        .to_json_string()
    }
}

impl fmt::Display for TaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)?;
        if let Some(details) = &self.details {
            write!(f, " ({details})")?;
        }
        Ok(())
    }
}

impl std::error::Error for TaseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json::{Limits, parse};

    /// The table of the design book §4.3: code, status, retryable.
    const TABLE: [(ErrorCode, &str, u16, bool); 12] = [
        (ErrorCode::InvalidArgument, "INVALID_ARGUMENT", 400, false),
        (ErrorCode::Unauthenticated, "UNAUTHENTICATED", 401, true),
        (ErrorCode::PermissionDenied, "PERMISSION_DENIED", 403, false),
        (ErrorCode::NotFound, "NOT_FOUND", 404, false),
        (
            ErrorCode::IdempotencyMismatch,
            "IDEMPOTENCY_MISMATCH",
            409,
            false,
        ),
        (ErrorCode::StaleBase, "STALE_BASE", 409, false),
        (ErrorCode::SymbolConflict, "SYMBOL_CONFLICT", 409, false),
        (ErrorCode::PolicyRejected, "POLICY_REJECTED", 422, false),
        (ErrorCode::ValidationFailed, "VALIDATION_FAILED", 422, false),
        (ErrorCode::RateLimited, "RATE_LIMITED", 429, true),
        (
            ErrorCode::WorkerUnavailable,
            "WORKER_UNAVAILABLE",
            503,
            true,
        ),
        (ErrorCode::InternalError, "INTERNAL_ERROR", 500, false),
    ];

    #[test]
    fn the_table_of_the_design_book_is_the_table_of_the_code() {
        assert_eq!(ErrorCode::ALL.len(), TABLE.len());
        for (code, name, status, retryable) in TABLE {
            assert_eq!(code.as_str(), name, "{code:?}");
            assert_eq!(code.http_status(), status, "{code:?}");
            assert_eq!(code.retryable(), retryable, "{code:?}");
        }
    }

    #[test]
    fn every_code_comes_back_from_its_name() {
        for code in ErrorCode::ALL {
            assert_eq!(ErrorCode::from_str(code.as_str()), Some(code), "{code:?}");
            assert_eq!(code.as_str().parse::<ErrorCode>().unwrap(), code);
            assert_eq!(code.to_string(), code.as_str());
        }
        assert_eq!(ErrorCode::from_str("NOPE"), None);
        assert!("NOPE".parse::<ErrorCode>().is_err());
    }

    #[test]
    fn only_the_three_recoverable_codes_are_retryable() {
        let retryable: Vec<&str> = ErrorCode::ALL
            .iter()
            .filter(|c| c.retryable())
            .map(|c| c.as_str())
            .collect();
        assert_eq!(
            retryable,
            ["UNAUTHENTICATED", "RATE_LIMITED", "WORKER_UNAVAILABLE"]
        );
    }

    #[test]
    fn every_constructor_keeps_its_code() {
        let cases: Vec<(TaseError, ErrorCode)> = vec![
            (TaseError::invalid_argument("a"), ErrorCode::InvalidArgument),
            (TaseError::unauthenticated("a"), ErrorCode::Unauthenticated),
            (
                TaseError::permission_denied("a"),
                ErrorCode::PermissionDenied,
            ),
            (TaseError::not_found("a"), ErrorCode::NotFound),
            (
                TaseError::idempotency_mismatch("a"),
                ErrorCode::IdempotencyMismatch,
            ),
            (TaseError::stale_base("a"), ErrorCode::StaleBase),
            (TaseError::symbol_conflict("a"), ErrorCode::SymbolConflict),
            (TaseError::policy_rejected("a"), ErrorCode::PolicyRejected),
            (
                TaseError::validation_failed("a"),
                ErrorCode::ValidationFailed,
            ),
            (TaseError::rate_limited("a"), ErrorCode::RateLimited),
            (
                TaseError::worker_unavailable("a"),
                ErrorCode::WorkerUnavailable,
            ),
            (TaseError::internal("a"), ErrorCode::InternalError),
        ];
        for (error, code) in cases {
            assert_eq!(error.code, code);
            assert_eq!(error.message, "a");
            assert!(error.details.is_none());
        }
    }

    #[test]
    fn the_json_of_an_error_carries_the_code_twice_and_the_details() {
        let error = TaseError::stale_base("the base moved").with_details("plan_001");
        let value = parse(&error.to_json(), Limits::DEFAULT).expect("valid JSON");
        assert_eq!(value.get("code").unwrap().as_str(), Some("STALE_BASE"));
        assert_eq!(
            value.get("business_code").unwrap().as_str(),
            Some("STALE_BASE")
        );
        assert_eq!(
            value.get("message").unwrap().as_str(),
            Some("the base moved")
        );
        assert_eq!(value.get("details").unwrap().as_str(), Some("plan_001"));
        assert_eq!(value.get("retryable").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn the_json_of_an_error_without_details_holds_a_null() {
        let error = TaseError::not_found("no such template");
        let value = parse(&error.to_json(), Limits::DEFAULT).expect("valid JSON");
        assert!(value.get("details").unwrap().is_null());
        assert_eq!(value.get("retryable").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn an_error_displays_its_code_and_its_message() {
        let error = TaseError::rate_limited("too many jobs");
        assert_eq!(error.to_string(), "RATE_LIMITED: too many jobs");
        assert_eq!(
            error.clone().with_details("q=10").to_string(),
            "RATE_LIMITED: too many jobs (q=10)"
        );
        // the error is a std error, so it can travel through a `Box<dyn Error>`
        let boxed: Box<dyn std::error::Error> = Box::new(error);
        assert_eq!(boxed.to_string(), "RATE_LIMITED: too many jobs");
    }

    #[test]
    fn the_message_may_be_anything_that_becomes_a_string() {
        let error = TaseError::internal(String::from("boom"));
        assert_eq!(error.message, "boom");
        let error = TaseError::internal("boom");
        assert_eq!(error.message, "boom");
    }
}
