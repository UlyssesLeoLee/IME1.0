//! The door of the Hub: who may talk to it.
//!
//! The rule of the design books is one line, and it is the same on both sides of it (detailed design
//! §17.2, basic design §16.2): **only a process of the same user, of the same login session and of the
//! same terminal-services session may talk to the Hub.** Everything else is refused.
//!
//! Two levels of that rule exist, and both must hold:
//!
//! | level | what it does | who enforces it |
//! |---|---|---|
//! | the DACL of the pipe | only the logon SID may open the pipe | Windows, before a byte is read |
//! | [`authorize`] | only the same user of the same session may talk to the Hub | the Hub, on the identity of the peer |
//!
//! The DACL is the door and the policy is the room behind it: a peer that got in (because it is of the
//! same user) may still be of another terminal-services session, and the Hub refuses it.
//!
//! **Known gap (mapping §7-G4, decisions D-47 to D-49).** The pipe of the Hub is created by
//! `pime-ipc`, whose DACL is built from the logon SID (that part is right), but the token of the peer
//! process is not handed to the server: `pime-ipc` authenticates the peer with a shared secret of the
//! user, not with the token of the peer. So the Hub cannot *read* the logon SID of the peer process;
//! it relies on the DACL of the pipe for that, and it judges the rest (the user and the
//! terminal-services session) itself. The identity of the peer is therefore the identity the pipe
//! hands over, and a peer whose identity is unknown is refused ([`ErrorCode::Unauthenticated`]) rather
//! than taken for a friend.

use std::sync::atomic::{AtomicU64, Ordering};

use tase_contract::{AgentId, SessionId, TaseError};
use tase_local_ipc::{AclPolicy, PeerIdentity};

use crate::session::AgentSession;

/// The counter of the sessions; it is the only source of the ids of the sessions.
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The next number of a session.
fn next_number() -> u64 {
    SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Checks that a peer may talk to the Hub, and gives it a session.
///
/// The session that comes back is the *skeleton* of the session: it has its ids, and the provider, the
/// instance and the capabilities are filled by the registration (`tase.agent.register`), because the
/// peer does not get to name itself: the name of a client is not an identity (detailed design §17.3).
///
/// The failure is [`ErrorCode::PermissionDenied`], and the reason says which of the three rules the
/// peer broke - the reason is a fixed string of the IPC crate, so it never carries what the peer said.
pub fn authorize(acl: &AclPolicy, peer: &PeerIdentity) -> Result<AgentSession, TaseError> {
    if let Some(reason) = tase_local_ipc::deny_reason(acl, peer) {
        return Err(TaseError::permission_denied(format!(
            "the peer may not talk to the hub: {reason}"
        ))
        .with_details(reason));
    }
    let number = next_number();
    Ok(AgentSession::new(
        SessionId::new(format!("sess_{number:012}"))?,
        AgentId::new(format!("agent_{number:012}"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::ErrorCode;

    const LOGON: &str = "S-1-5-5-1000-1000";
    const USER: &str = "S-1-5-21-1-2-3-1001";
    const OTHER_LOGON: &str = "S-1-5-5-2000-2000";
    const OTHER_USER: &str = "S-1-5-21-1-2-3-1002";

    fn policy() -> AclPolicy {
        AclPolicy::new(LOGON, USER, 3)
    }

    fn peer() -> PeerIdentity {
        PeerIdentity::new(USER, LOGON, 3, 4242)
    }

    #[test]
    fn the_peer_of_the_same_session_is_authorized() {
        let session = authorize(&policy(), &peer()).expect("the peer is authorized");
        assert!(session.session_id.as_str().starts_with("sess_"));
        assert!(session.agent.as_str().starts_with("agent_"));
        assert!(!session.is_registered());
    }

    #[test]
    fn two_authorizations_give_two_sessions() {
        let first = authorize(&policy(), &peer()).expect("the peer is authorized");
        let second = authorize(&policy(), &peer()).expect("the peer is authorized");
        assert_ne!(first.session_id, second.session_id);
        assert_ne!(first.agent, second.agent);
    }

    #[test]
    fn another_logon_session_is_refused() {
        let mut other = peer();
        other.logon_sid = OTHER_LOGON.to_string();
        let error = authorize(&policy(), &other).expect_err("the peer is refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.contains("logon session"), "{}", error.message);
    }

    #[test]
    fn another_user_is_refused() {
        let mut other = peer();
        other.user_sid = OTHER_USER.to_string();
        let error = authorize(&policy(), &other).expect_err("the peer is refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.contains("user"), "{}", error.message);
    }

    #[test]
    fn another_terminal_session_is_refused() {
        let mut other = peer();
        other.session_id = 4;
        let error = authorize(&policy(), &other).expect_err("the peer is refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.contains("session"), "{}", error.message);
    }

    #[test]
    fn a_policy_without_an_identity_refuses_everybody() {
        let empty = AclPolicy::new("", "", 0);
        let error = authorize(&empty, &peer()).expect_err("the peer is refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
    }

    #[test]
    fn the_process_of_the_peer_does_not_matter() {
        let mut other = peer();
        other.process_id = 9999;
        assert!(authorize(&policy(), &other).is_ok());
    }

    #[test]
    fn the_reason_of_the_refusal_is_a_fixed_string() {
        let mut other = peer();
        other.user_sid = OTHER_USER.to_string();
        let error = authorize(&policy(), &other).expect_err("the peer is refused");
        // the details name the rule, not what the peer said
        assert_eq!(error.details.as_deref(), Some("another user"));
    }
}
