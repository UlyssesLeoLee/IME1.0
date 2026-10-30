//! One error type for the whole crate. Every variant maps to a design error code (`IpcError::code`), which is also what
//! goes on the wire in `{"ok":false,"error":"<CODE>"}` frames, so the two ends agree on the vocabulary.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcError {
    /// A deadline passed (`IPC_TIMEOUT`, DD 12.2).
    Timeout,
    /// The peer closed the connection between two frames.
    Closed,
    /// The peer closed the connection in the middle of a frame.
    Truncated,
    /// A frame is above the limit of its class (`PAYLOAD_TOO_LARGE`, DD 2.1).
    PayloadTooLarge { len: usize, max: usize },
    /// A frame or handshake message violates the format; the reason is a fixed string (never peer data).
    Malformed(&'static str),
    /// Different major protocol version (`PROTOCOL_MISMATCH`, DD 2.1).
    ProtocolMismatch,
    /// The peer failed the handshake: unknown role, wrong proof, wrong server (`IDENTITY_REJECTED`).
    IdentityRejected,
    /// The authenticated peer asked for capabilities its role does not have.
    CapabilityDenied,
    /// A client nonce was seen before inside the replay window, or the window is full.
    Replay,
    /// The challenge was answered after its lifetime.
    StaleChallenge,
    /// The client lives in another logon session than the server (DD 2.1: other users and sessions are refused).
    WrongSession,
    /// No server is listening on the pipe (`ERROR_FILE_NOT_FOUND`): callers fall back to Direct at once.
    Unavailable,
    /// The pipe name is already taken, i.e. `FILE_FLAG_FIRST_PIPE_INSTANCE` failed: possible squatting.
    NameInUse,
    /// The listener was stopped through its `StopHandle`.
    Stopped,
    /// The connection saw an error earlier (partial frame, timeout mid-exchange) and must be dropped.
    Broken,
    /// A caller-side argument is invalid (bad key length, bad role name, limits above the hard maximum ...).
    InvalidArgument(&'static str),
    /// A Win32 call failed unexpectedly.
    Os { op: &'static str, code: u32 },
}

impl IpcError {
    /// The error code of the design documents (DD 12.2 / 2.1) for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            IpcError::Timeout => "IPC_TIMEOUT",
            IpcError::Closed | IpcError::Truncated | IpcError::Broken | IpcError::Stopped => {
                "IPC_CLOSED"
            }
            IpcError::PayloadTooLarge { .. } => "PAYLOAD_TOO_LARGE",
            IpcError::Malformed(_) => "MALFORMED_FRAME",
            IpcError::ProtocolMismatch => "PROTOCOL_MISMATCH",
            IpcError::IdentityRejected
            | IpcError::WrongSession
            | IpcError::Replay
            | IpcError::StaleChallenge => "IDENTITY_REJECTED",
            IpcError::CapabilityDenied => "CAPABILITY_DENIED",
            IpcError::Unavailable => "IPC_UNAVAILABLE",
            IpcError::NameInUse => "IPC_NAME_IN_USE",
            IpcError::InvalidArgument(_) => "INVALID_ARGUMENT",
            IpcError::Os { .. } => "IPC_OS_ERROR",
        }
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcError::PayloadTooLarge { len, max } => {
                write!(f, "{}: frame of {len} bytes exceeds {max}", self.code())
            }
            IpcError::Malformed(why) | IpcError::InvalidArgument(why) => {
                write!(f, "{}: {why}", self.code())
            }
            IpcError::Os { op, code } => {
                write!(f, "{}: {op} failed with Win32 error {code}", self.code())
            }
            other => write!(f, "{}", other.code()),
        }
    }
}

impl std::error::Error for IpcError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ut_ipc_error_codes_are_the_design_codes() {
        assert_eq!(IpcError::Timeout.code(), "IPC_TIMEOUT");
        assert_eq!(
            IpcError::PayloadTooLarge { len: 2, max: 1 }.code(),
            "PAYLOAD_TOO_LARGE"
        );
        assert_eq!(IpcError::ProtocolMismatch.code(), "PROTOCOL_MISMATCH");
        // every identity-related failure looks the same to a peer
        for e in [
            IpcError::IdentityRejected,
            IpcError::Replay,
            IpcError::StaleChallenge,
            IpcError::WrongSession,
        ] {
            assert_eq!(e.code(), "IDENTITY_REJECTED");
        }
        assert_eq!(
            IpcError::PayloadTooLarge {
                len: 70_000,
                max: 65_536
            }
            .to_string(),
            "PAYLOAD_TOO_LARGE: frame of 70000 bytes exceeds 65536"
        );
        // no variant displays data taken from a peer: reasons are fixed strings
        assert_eq!(
            IpcError::Malformed("bad frame magic").to_string(),
            "MALFORMED_FRAME: bad frame magic"
        );
    }
}
