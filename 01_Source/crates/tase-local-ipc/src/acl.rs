//! The namespace of the Hub: one pipe per logon session, and the rules of who may connect.
//!
//! DD §17.2 ("启动与单例模型") fixes the shape of the namespace: "Hub 入口调用 `start_or_connect()`：先以 Windows
//! 登录 SID 作为 namespace 计算本用户管道名和单实例互斥体". BD §16.2 says the same in one line: the pipe of the
//! Hub is `\\.\pipe\ide1-tase-{logon-sid}` and its DACL binds the logon SID of the session.
//!
//! So there is one Hub per **logon session**, not per user: a user that is logged on twice (two terminal
//! sessions, a remote desktop session) gets two Hubs that share nothing, and a process of another session - even
//! of the same user - may not connect to either of them.
//!
//! The rules are pure functions of the identity of the peer, so they can be tested without a pipe:
//!
//! * [`pipe_name`] is the name of the pipe of a logon session;
//! * [`may_connect`] says whether a peer may connect to the Hub of that session.
//!
//! Two levels of the same rule, and both must hold:
//!
//! | level | what it does | who enforces it |
//! |---|---|---|
//! | the DACL of the pipe | only the logon SID may open the pipe | Windows, before a byte is read |
//! | [`may_connect`] | only the same user of the same session may talk to the Hub | the Hub, on the identity of the peer |
//!
//! The DACL is the door and the policy is the room behind it: a peer that got in (because it is of the same user)
//! may still be of another session, and the Hub refuses it.

/// The prefix of the pipe of the Hub.
///
/// The name is the one of the IPC family of the repository (`pime-ipc` names its pipes
/// `\\.\pipe\PIME-IPC-1-{sid}`), because the pipe is opened with `pime-ipc`, whose `PipeName` has no other
/// constructor. The *rule* of DD §17.2 - one pipe per logon session, named after the logon SID - is the one that
/// matters, and it is the one that is implemented; only the literal prefix differs from the schematic
/// `ide1-tase-` of BD §16.2 (the design calls that name schematic: "示意，真实名由安装器生成").
pub const PIPE_PREFIX: &str = r"\\.\pipe\PIME-IPC-1-";

/// Whether a string is a SID that may be put into the name of a pipe.
///
/// A SID is `S-1-<authority>-<sub>...` with digits only: it is formatted into an SDDL string and into a pipe
/// name, so anything else is refused, never escaped.
pub fn is_valid_logon_sid(sid: &str) -> bool {
    pime_ipc::auth::validate_sid(sid).is_ok()
}

/// The name of the pipe of the Hub of a logon session.
///
/// Every logon session gets its own name, so two sessions of the same user never meet on the same pipe, and the
/// name of a session is not something another session can guess by accident: it is the logon SID of the session.
///
/// A string that is not a SID is made safe before it is put into the name (the name goes into a Win32 path):
/// every character that is not a letter, a digit or a dash becomes a dash. A name that is not built from a SID is
/// refused by the pipe itself when it is opened.
pub fn pipe_name(logon_sid: &str) -> String {
    format!("{PIPE_PREFIX}{}", safe_sid(logon_sid))
}

/// The SID of a logon session, made safe for a name.
fn safe_sid(sid: &str) -> String {
    let trimmed = sid.trim();
    if is_valid_logon_sid(trimmed) {
        return trimmed.to_string();
    }
    let mut out = String::new();
    for character in trimmed.chars().take(64) {
        if character.is_ascii_alphanumeric() || character == '-' {
            out.push(character);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        out.push_str("unknown");
    }
    out
}

/// Who is on the other end of a pipe.
///
/// The four fields are the four things the Hub checks: the user, the logon session, the terminal-services session
/// and the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    /// The SID of the user of the peer (`S-1-5-21-...`).
    pub user_sid: String,
    /// The SID of the logon session of the peer (`S-1-5-5-...`).
    pub logon_sid: String,
    /// The terminal-services session of the peer.
    pub session_id: u32,
    /// The process id of the peer.
    pub process_id: u32,
}

impl PeerIdentity {
    /// The identity of a peer of the given user and logon session.
    pub fn new(user_sid: &str, logon_sid: &str, session_id: u32, process_id: u32) -> PeerIdentity {
        PeerIdentity {
            user_sid: user_sid.to_string(),
            logon_sid: logon_sid.to_string(),
            session_id,
            process_id,
        }
    }
}

/// The rules of the Hub of one logon session.
///
/// The Hub builds it once, when it starts, out of its own identity: the logon session it belongs to, the user it
/// runs as and the terminal-services session it runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AclPolicy {
    /// The logon session the Hub belongs to; the peers of another one are refused.
    pub required_logon_sid: String,
    /// The user the Hub runs as; the peers of another user are refused.
    pub required_user_sid: String,
    /// The terminal-services session the Hub runs in; the peers of another one are refused.
    pub required_session_id: u32,
}

impl AclPolicy {
    /// The policy of the Hub of a logon session.
    pub fn new(
        required_logon_sid: &str,
        required_user_sid: &str,
        required_session_id: u32,
    ) -> AclPolicy {
        AclPolicy {
            required_logon_sid: required_logon_sid.to_string(),
            required_user_sid: required_user_sid.to_string(),
            required_session_id,
        }
    }
}

/// Why a peer may not connect to the Hub; `None` when it may.
///
/// The reasons are fixed strings: they go into a log of the Hub, and they never carry what a peer said.
pub fn deny_reason(acl: &AclPolicy, peer: &PeerIdentity) -> Option<&'static str> {
    if acl.required_logon_sid.is_empty() {
        return Some("the policy names no logon session");
    }
    if acl.required_user_sid.is_empty() {
        return Some("the policy names no user");
    }
    if peer.logon_sid != acl.required_logon_sid {
        return Some("another logon session");
    }
    if peer.user_sid != acl.required_user_sid {
        return Some("another user");
    }
    if peer.session_id != acl.required_session_id {
        return Some("another session");
    }
    None
}

/// Whether a peer may connect to the Hub of that session.
///
/// Only a process of the same user, of the same logon session and of the same terminal-services session may
/// connect (DD §17.2, BD §16.2: "不同登录用户之间不共享权限"). Everything else is refused.
pub fn may_connect(acl: &AclPolicy, peer: &PeerIdentity) -> bool {
    deny_reason(acl, peer).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_peer_of_the_same_session_may_connect() {
        assert!(may_connect(&policy(), &peer()));
        assert_eq!(deny_reason(&policy(), &peer()), None);
    }

    /// Another logon session is refused, even for the same user: the sessions share nothing.
    #[test]
    fn another_logon_session_is_refused() {
        let mut other = peer();
        other.logon_sid = OTHER_LOGON.to_string();
        assert!(!may_connect(&policy(), &other));
        assert_eq!(deny_reason(&policy(), &other), Some("another logon session"));
    }

    #[test]
    fn another_user_is_refused() {
        let mut other = peer();
        other.user_sid = OTHER_USER.to_string();
        assert!(!may_connect(&policy(), &other));
        assert_eq!(deny_reason(&policy(), &other), Some("another user"));
    }

    /// Another terminal-services session is refused, even for the same user and the same logon session.
    #[test]
    fn another_session_is_refused() {
        let mut other = peer();
        other.session_id = 4;
        assert!(!may_connect(&policy(), &other));
        assert_eq!(deny_reason(&policy(), &other), Some("another session"));
    }

    /// A policy that names nothing refuses everybody: a Hub that does not know who it is admits nobody.
    #[test]
    fn a_policy_without_an_identity_refuses_everybody() {
        let empty = AclPolicy::new("", "", 0);
        assert!(!may_connect(&empty, &peer()));
        assert_eq!(
            deny_reason(&empty, &peer()),
            Some("the policy names no logon session")
        );
        let no_user = AclPolicy::new(LOGON, "", 3);
        assert!(!may_connect(&no_user, &peer()));
    }

    /// The process id is not a rule: the same session may hold many processes (the IDE, the agent, the CLI).
    #[test]
    fn the_process_does_not_matter() {
        let mut other = peer();
        other.process_id = 9999;
        assert!(may_connect(&policy(), &other));
    }

    /// Every logon session has its own pipe: the name of a session is the logon SID of that session.
    #[test]
    fn every_logon_session_has_its_own_pipe() {
        let first = pipe_name(LOGON);
        let second = pipe_name(OTHER_LOGON);
        assert_ne!(first, second);
        assert_eq!(first, pipe_name(LOGON));
        assert!(first.contains(LOGON));
        assert!(first.starts_with(r"\\.\pipe\"));
    }

    /// The name goes into a Win32 path: a string that is not a SID may not smuggle anything into it.
    #[test]
    fn a_hostile_sid_cannot_smuggle_anything_into_the_name() {
        let name = pipe_name("S-1-5; rm -rf / \\\\evil");
        let suffix = name.strip_prefix(PIPE_PREFIX).expect("the prefix is there");
        assert!(!suffix.contains(';'));
        assert!(!suffix.contains('/'));
        assert!(!suffix.contains('\\'));
        assert!(name.starts_with(PIPE_PREFIX));
        // and an empty one still gives a name
        assert!(!pipe_name("").is_empty());
        assert!(pipe_name("").contains("unknown"));
    }

    /// A logon SID is a SID: the validator of `pime-ipc` is the one that decides.
    #[test]
    fn a_logon_sid_is_a_sid() {
        assert!(is_valid_logon_sid(LOGON));
        assert!(is_valid_logon_sid(USER));
        assert!(is_valid_logon_sid("S-1-5-18"));
        assert!(!is_valid_logon_sid("S-1-5-x"));
        assert!(!is_valid_logon_sid("S-1-"));
        assert!(!is_valid_logon_sid(""));
        assert!(!is_valid_logon_sid("current_user"));
    }

    /// The name of the pipe is the name the pipe is opened with: the two must not drift apart.
    #[cfg(windows)]
    #[test]
    fn the_name_is_the_name_the_pipe_is_opened_with() {
        let name = pipe_name(LOGON);
        let opened = crate::transport::WindowsPipe::pipe_name(LOGON).expect("the name");
        assert_eq!(name, opened);
    }
}
