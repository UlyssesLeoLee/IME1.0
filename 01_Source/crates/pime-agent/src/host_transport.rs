//! The real transport of the host: the bridge of the input method.
//!
//! The executor of TASE does not know how the answer of the host travels: it knows the wire format of the bridge
//! (`pime_core::agent_proto`) and nothing else. This module is the other half of that door: it carries the request
//! of the executor to the text service of the focused process (the `WM_COPYDATA` bridge of `crate::bridge`) and
//! brings the answer back.
//!
//! The product side is the only layer that may talk to the focused window, so this is where the transport of the
//! host adapter lives: the executor of `tase-executor` holds the adapter and the plan, and this crate holds the
//! way the answer travels.
//!
//! # What an answer that says "no" looks like
//!
//! The bridge answers a request that the host refused with a code and a line for the user (`NO_IME`,
//! `SECURE_FIELD`, `REFUSED`, `BUSY`, ...). The client of the bridge reports such an answer as a failure of the
//! protocol, so this transport turns it back into the answer it is: the code travels unchanged, and the host that
//! is not there is never reported as something else.

use pime_core::agent_proto::{Error, Reply, Request, SourceIdentity};
use std::sync::Arc;
use tase_contract::{ErrorCode, TaseError};
use tase_executor::host::HostTransport;

use crate::{AgentError, ImeBridge};

/// The transport of the host that talks to the text service of the focused window.
pub struct BridgeTransport {
    bridge: Arc<dyn ImeBridge>,
    source: SourceIdentity,
}

impl BridgeTransport {
    /// A transport over a bridge, with the identity of the client that writes.
    ///
    /// The identity is stamped on every request that goes out: the input method draws the mouth of light of that
    /// agent, so the user sees at once which agent writes into the window.
    pub fn new(bridge: Arc<dyn ImeBridge>, source: SourceIdentity) -> BridgeTransport {
        BridgeTransport { bridge, source }
    }
}

impl HostTransport for BridgeTransport {
    /// Carries a request to the text service and waits for its answer.
    ///
    /// The failure of the client of the bridge that carries a code of the bridge (`NO_IME`, `SECURE_FIELD`, ...)
    /// becomes the answer it stands for: the code travels unchanged, so the layer above can name it again.
    fn call(&self, request: &Request) -> Result<Reply, TaseError> {
        let mut request = request.clone();
        request.source = self.source.clone();
        match self.bridge.send(&request) {
            Ok(reply) => Ok(reply),
            Err(AgentError::Protocol { code, message }) => Ok(Reply::failed(code, &message)),
            Err(error) => Err(tase_error_of(&error)),
        }
    }
}

/// The failure of the agent, as a failure of the contract.
///
/// The mapping keeps the meaning of the bridge: a host that is not there is a worker that is not available, a host
/// that refuses a write (a password field, a document that says no) is a policy that refuses it, and a request that
/// is not one of ours is an invalid argument. The message of the failure keeps the words of the host, and the code
/// of the bridge is kept in the details.
pub fn tase_error_of(error: &AgentError) -> TaseError {
    match error {
        AgentError::Protocol { code, message } => TaseError::new(code_of(*code), message.clone())
            .with_details(format!("the bridge answered {}", code.as_str())),
        AgentError::CoreUnavailable { message } => TaseError::worker_unavailable(message.clone()),
    }
}

/// The contract code of a code of the bridge.
fn code_of(code: Error) -> ErrorCode {
    match code {
        Error::NoIme => ErrorCode::WorkerUnavailable,
        Error::NoContext => ErrorCode::NotFound,
        Error::Refused => ErrorCode::PolicyRejected,
        Error::Secure => ErrorCode::PolicyRejected,
        Error::Busy => ErrorCode::WorkerUnavailable,
        Error::TooLarge => ErrorCode::InvalidArgument,
        Error::Malformed => ErrorCode::InvalidArgument,
        Error::Unsupported => ErrorCode::InvalidArgument,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::agent_proto::Op;
    use std::sync::Mutex;

    /// A bridge that answers what the test says and keeps every request it was given.
    struct RecordingBridge {
        reply: Result<Reply, AgentError>,
        seen: Arc<Mutex<Vec<Request>>>,
    }

    impl ImeBridge for RecordingBridge {
        fn send(&self, request: &Request) -> Result<Reply, AgentError> {
            if let Ok(mut slot) = self.seen.lock() {
                slot.push(request.clone());
            }
            self.reply.clone()
        }
    }

    fn transport(
        reply: Result<Reply, AgentError>,
        seen: &Arc<Mutex<Vec<Request>>>,
    ) -> BridgeTransport {
        BridgeTransport::new(
            Arc::new(RecordingBridge {
                reply,
                seen: Arc::clone(seen),
            }),
            SourceIdentity::agent("anthropic", "claude-code", "0123456789abcdef"),
        )
    }

    #[test]
    fn the_request_that_goes_out_carries_the_identity_of_the_client() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let transport = transport(Ok(Reply::ok()), &seen);
        transport
            .call(&Request::new(Op::Read))
            .expect("the host answers");
        let seen = seen.lock().expect("the lock");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].op, Op::Read);
        assert_eq!(
            seen[0].source.key(),
            "agent:anthropic:claude-code:0123456789abcdef",
            "the input method draws the mouth of the agent that writes"
        );
    }

    #[test]
    fn a_host_that_is_not_there_keeps_its_code() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let transport = transport(
            Err(AgentError::protocol(
                Error::NoIme,
                "the input method is not running in the focused window",
            )),
            &seen,
        );
        let reply = transport
            .call(&Request::new(Op::Read))
            .expect("the answer of the host");
        assert!(!reply.ok);
        assert_eq!(
            reply.error,
            Some(Error::NoIme),
            "there is no input method: the answer says so"
        );
        assert!(reply.message.contains("not running"));
    }

    #[test]
    fn the_failure_of_the_agent_keeps_its_meaning() {
        let error = tase_error_of(&AgentError::protocol(
            Error::NoIme,
            "the input method is not running in the focused window",
        ));
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
        assert!(
            error.message.contains("not running"),
            "the words of the host are kept: {}",
            error.message
        );
        assert!(error.details.is_some_and(|d| d.contains("NO_IME")));

        let error = tase_error_of(&AgentError::protocol(
            Error::Secure,
            "the field is a password field",
        ));
        assert_eq!(error.code, ErrorCode::PolicyRejected);
        assert!(error.message.contains("password field"));

        let error = tase_error_of(&AgentError::core("the core is not there"));
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
        assert_eq!(error.message, "the core is not there");
    }
}
