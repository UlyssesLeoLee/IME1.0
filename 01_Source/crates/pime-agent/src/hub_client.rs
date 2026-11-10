//! The link of an AI client to the Hub of TASE: the client hands a job to the Hub, the Hub queues it,
//! a worker computes it, and the client comes back for the answer.
//!
//! # The three steps of a job
//!
//! | step | what happens | what goes over the pipe |
//! |---|---|---|
//! | submit | the client hands the job to the Hub; the Hub puts it in its queue | one `job.enqueue` |
//! | compute | a worker takes the job, computes the **diff** (it does not write the window of the user) | `job.take` and `job.complete`, between the worker and the Hub |
//! | collect | the client asks how the job went and reads the result | one `job.status` |
//!
//! # Where the name of the pipe comes from
//!
//! The Hub of a logon session listens on the pipe of that session (`\\.\pipe\PIME-IPC-1-<logon sid>`).
//! **A client cannot read its own logon SID**: the transport of TASE reads the token of the user, not the
//! logon SID of the session (defect D-48). The SID is therefore a configuration, and this module takes it from
//! one of three places, in this order:
//!
//! * the argument of the caller (`logon_sid`),
//! * the name of the pipe itself (the SID is read out of it),
//! * the variable `TASE_LOGON_SID`.
//!
//! When none of them is there, the link says so and does **not** guess a name.

use pime_core::json::Json;
use tase_contract::TaseError;
use tase_local_ipc::{Connection, FrameError, Hello, Timeouts, Transport};

/// The capabilities this client asks for.
///
/// They have to be a part of the list of the Hub (`tase_local_hub::HUB_CAPABILITIES`): the Hub grants the
/// intersection of what the client asks for and of what it has, and a capability that is not granted ends the
/// handshake.
pub const CLIENT_CAPABILITIES: [&str; 2] = ["job:write", "events:read"];

/// The name of the variable that holds the logon SID of the session of the Hub.
pub const LOGON_SID_VARIABLE: &str = "TASE_LOGON_SID";

/// The prefix of the name of the pipe of the Hub.
pub const PIPE_PREFIX: &str = "\\\\.\\pipe\\PIME-IPC-1-";

/// A job the Hub put in its queue.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRef {
    /// The id of the job.
    pub job_id: String,
    /// The state the job is in (`QUEUED` when it was just accepted).
    pub status: String,
    /// Whether the job was already there: the Hub answers `true` when the idempotency key is known.
    pub deduplicated: bool,
}

/// The state of a job, and its result when it is done.
#[derive(Clone, Debug, PartialEq)]
pub struct JobState {
    /// The state the job is in (`QUEUED`, `RUNNING`, `COMMITTED`, `FAILED`, ...).
    pub status: String,
    /// The result of the job: the diff of the change, the files it touches and the digest of the tree.
    pub result: Option<Json>,
}

/// The link of a client to the Hub.
pub struct HubLink<T: Transport> {
    connection: Connection<T>,
    client_id: String,
}

impl<T: Transport> HubLink<T> {
    /// A link over a transport.
    ///
    /// The transport is the pipe of the Hub: a named pipe in production
    /// ([`tase_local_ipc::WindowsPipe`]), a memory pipe in the tests. The deadlines are the ones of the pipe.
    pub fn new(
        transport: T,
        client_id: &str,
        capabilities: &[&str],
        timeouts: Timeouts,
    ) -> HubLink<T> {
        HubLink {
            connection: Connection::new(
                transport,
                capabilities.iter().map(|name| name.to_string()).collect(),
                timeouts,
            ),
            client_id: client_id.to_string(),
        }
    }

    /// The name of the client of this link.
    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The session of the connection; 0 before the handshake.
    #[must_use]
    pub fn session_id(&self) -> u64 {
        self.connection.session_id()
    }

    /// Greets the Hub and learns what it granted.
    pub fn handshake(&mut self) -> Result<(), TaseError> {
        let hello = Hello::new(&self.client_id, &CLIENT_CAPABILITIES);
        self.connection
            .handshake(&hello)
            .map(|_| ())
            .map_err(map_frame_error)
    }

    /// Registers this client with the Hub: who it is and in which workspace it works.
    ///
    /// The Hub needs it before a job may be queued: the workspace of the job is the workspace of the session.
    pub fn register(&mut self, provider: &str, workspace_id: &str) -> Result<(), TaseError> {
        let params = Json::obj(vec![
            ("provider", Json::str(provider)),
            ("workspace_id", Json::str(workspace_id)),
            (
                "requested_scopes",
                Json::strs(&CLIENT_CAPABILITIES.iter().map(|name| name.to_string()).collect::<Vec<_>>()),
            ),
        ]);
        self.connection
            .request("agent.register", &params)
            .map(|_| ())
            .map_err(map_frame_error)
    }

    /// Hands a job to the Hub.
    ///
    /// `params` is the request of the job (what has to be computed). The Hub does not read it: it queues it and
    /// hands it to a worker as it is.
    pub fn submit(&mut self, params: &Json) -> Result<JobRef, TaseError> {
        let answer = self
            .connection
            .request("job.enqueue", params)
            .map_err(map_frame_error)?;
        let job_id = answer
            .get("job_id")
            .and_then(Json::as_str)
            .ok_or_else(|| {
                TaseError::internal("the hub did not name the job it queued".to_string())
            })?
            .to_string();
        Ok(JobRef {
            job_id,
            status: answer
                .get("status")
                .and_then(Json::as_str)
                .unwrap_or("QUEUED")
                .to_string(),
            deduplicated: answer.get("deduplicated").and_then(Json::as_bool) == Some(true),
        })
    }

    /// Asks the Hub how the job went.
    ///
    /// The result is there when the job is done; while it is not, the state says so and the result is `None`.
    pub fn state(&mut self, job_id: &str) -> Result<JobState, TaseError> {
        let mut params = Json::Obj(Vec::new());
        if let Json::Obj(pairs) = &mut params {
            pairs.push(("job_id".to_string(), Json::str(job_id)));
        }
        let answer = self
            .connection
            .request("job.status", &params)
            .map_err(map_frame_error)?;
        Ok(JobState {
            status: answer
                .get("status")
                .and_then(Json::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            result: answer.get("result").cloned().filter(|value| !value.is_null()),
        })
    }

    /// Closes the pipe.
    pub fn close(&mut self, reason: &str) -> Result<(), TaseError> {
        self.connection.close(reason).map_err(map_frame_error)
    }
}

/// The logon SID of the session of the Hub, out of the configuration.
///
/// The order is the argument, then the name of the pipe, then the variable. When none of them is there, the
/// reason says what to do.
pub fn logon_sid_of(
    argument: Option<&str>,
    pipe_name: Option<&str>,
    variable: Option<&str>,
) -> Result<String, String> {
    if let Some(sid) = argument.map(str::trim).filter(|sid| !sid.is_empty()) {
        return Ok(sid.to_string());
    }
    if let Some(name) = pipe_name {
        return logon_sid_of_pipe(name);
    }
    if let Some(sid) = variable.map(str::trim).filter(|sid| !sid.is_empty()) {
        return Ok(sid.to_string());
    }
    Err(format!(
        "the client needs the logon SID of the session of the Hub: give the logon SID, give the name of the \
         pipe, or set the {LOGON_SID_VARIABLE} variable"
    ))
}

/// The logon SID out of the name of the pipe of the Hub.
pub fn logon_sid_of_pipe(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    let Some(sid) = trimmed.strip_prefix(PIPE_PREFIX) else {
        return Err(format!(
            "the pipe of the hub is named {PIPE_PREFIX}<logon sid>: {trimmed} is not such a name"
        ));
    };
    let sid = sid.trim();
    if sid.is_empty() {
        return Err("the name of the pipe holds no logon SID".to_string());
    }
    Ok(sid.to_string())
}

/// The frame error of the pipe as the error of the contract.
fn map_frame_error(error: FrameError) -> TaseError {
    match error {
        FrameError::Remote(text) => remote_error(&text),
        FrameError::Timeout => {
            TaseError::worker_unavailable("the hub did not answer the request in time")
        }
        FrameError::Closed => TaseError::worker_unavailable("the hub closed the pipe"),
        FrameError::ProtocolMismatch => {
            TaseError::invalid_argument("the hub speaks another version of the protocol of the pipe")
        }
        FrameError::CapabilityDenied => {
            TaseError::permission_denied("the hub did not grant a capability the client needs")
        }
        FrameError::TooLarge => TaseError::invalid_argument("the frame is above the limit"),
        FrameError::Malformed | FrameError::Truncated | FrameError::UnknownKind => {
            TaseError::internal("the pipe of the hub is out of sync")
        }
        FrameError::WouldBlock => TaseError::worker_unavailable("the hub does not read the pipe"),
        FrameError::Duplicate => TaseError::internal("the hub answered the same request twice"),
        FrameError::Broken => TaseError::worker_unavailable("the pipe of the hub is broken"),
        FrameError::Io(reason) => {
            TaseError::worker_unavailable(format!("the pipe of the hub failed: {reason}"))
        }
    }
}

/// The error the Hub answered, read back as the error of the contract.
///
/// The Hub answers with the JSON-RPC error of the contract; the code travels unchanged.
fn remote_error(text: &str) -> TaseError {
    let Ok(value) = pime_core::json::parse(text, pime_core::json::Limits::DEFAULT) else {
        return TaseError::internal(format!("the hub answered something that is not JSON: {text}"));
    };
    let code = value
        .get("code")
        .and_then(Json::as_str)
        .unwrap_or("INTERNAL");
    let message = value
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or("the hub refused the request");
    TaseError::new(tase_contract::ErrorCode::from_str(code).unwrap_or(tase_contract::ErrorCode::InternalError), message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tase_local_hub::{Hub, HubConfig, ManualClock, StaticSessionIdentity};
    use tase_local_ipc::{MemoryPipe, PeerIdentity};

    /// The deadlines of the tests: short, so that a test does not wait for a minute.
    fn fast_timeouts() -> Timeouts {
        Timeouts {
            handshake: Duration::from_secs(2),
            request: Duration::from_secs(5),
            write: Duration::from_secs(2),
            read: Duration::from_secs(2),
        }
    }

    /// The logon SID and the user SID of the tests.
    const LOGON: &str = "S-1-5-5-1000-1000";
    const USER: &str = "S-1-5-18";

    /// The identity of the Hub of the tests.
    fn identity() -> StaticSessionIdentity {
        StaticSessionIdentity::new(LOGON, USER, 3)
    }

    /// The peer of the client: the same user of the same logon session.
    fn peer() -> PeerIdentity {
        PeerIdentity::new(USER, LOGON, 3, 4242)
    }

    #[test]
    fn the_logon_sid_comes_from_the_argument_the_pipe_or_the_variable() {
        assert_eq!(
            logon_sid_of(Some(" S-1-5-5-1000-1000 "), None, None).expect("the argument"),
            "S-1-5-5-1000-1000"
        );
        assert_eq!(
            logon_sid_of(None, Some("\\\\.\\pipe\\PIME-IPC-1-S-1-5-5-1000-1000"), None)
                .expect("the name of the pipe"),
            "S-1-5-5-1000-1000"
        );
        assert_eq!(
            logon_sid_of(None, None, Some("S-1-5-5-1000-1000")).expect("the variable"),
            "S-1-5-5-1000-1000"
        );
        // none of them: the reason says what to do, and nothing is guessed
        let error = logon_sid_of(None, None, None).expect_err("there is no SID");
        assert!(error.contains("logon SID"), "{error}");
        assert!(error.contains(LOGON_SID_VARIABLE), "{error}");
    }

    #[test]
    fn a_name_that_is_not_the_pipe_of_the_hub_is_refused() {
        let error = logon_sid_of_pipe("not a pipe").expect_err("that is not a pipe name");
        assert!(error.contains("PIME-IPC-1"), "{error}");
        let error = logon_sid_of_pipe("\\\\.\\pipe\\PIME-IPC-1-").expect_err("no SID in it");
        assert!(error.contains("no logon SID"), "{error}");
    }

    #[test]
    fn the_frame_error_becomes_the_error_of_the_contract() {
        let error = map_frame_error(FrameError::Closed);
        assert_eq!(error.code, tase_contract::ErrorCode::WorkerUnavailable);
        let error = map_frame_error(FrameError::CapabilityDenied);
        assert_eq!(error.code, tase_contract::ErrorCode::PermissionDenied);
        let error = map_frame_error(FrameError::Io("the pipe is gone".to_string()));
        assert_eq!(error.code, tase_contract::ErrorCode::WorkerUnavailable);
        assert!(error.message.contains("the pipe is gone"), "{}", error.message);
    }

    #[test]
    fn the_answer_of_the_hub_is_read_back() {
        // the JSON-RPC answer of the Hub
        let answer = r#"{"code":"POLICY_REJECTED","message":"the queue is full"}"#;
        let error = remote_error(answer);
        assert_eq!(error.code, tase_contract::ErrorCode::PolicyRejected);
        assert_eq!(error.message, "the queue is full");
        // something that is not JSON at all
        let error = remote_error("not json");
        assert_eq!(error.code, tase_contract::ErrorCode::InternalError);
    }

    /// One whole turn: the client hands a job to the Hub over the pipe and reads its state back.
    ///
    /// The pipe is a memory pipe: no named pipe is opened, and no window of the user is written.
    #[test]
    fn a_client_hands_a_job_to_the_hub_and_reads_its_state() {
        let (hub_side, client_side) = MemoryPipe::pair();
        let hub = Hub::new(
            HubConfig {
                max_connections: 4,
                max_concurrent_jobs: 1,
                ..HubConfig::default()
            },
            &identity(),
        )
        .expect("the hub is built")
        .with_clock(Arc::new(ManualClock::new(1000)));
        let hub_thread = std::thread::spawn(move || {
            hub.serve(
                tase_local_hub::BoxedTransport::new(Box::new(hub_side)),
                Some(peer()),
            );
        });

        let mut link = HubLink::new(
            client_side,
            "claude-code",
            &CLIENT_CAPABILITIES,
            fast_timeouts(),
        );
        link.handshake().expect("the handshake works");
        link.register("claude", "ws_01").expect("the client registers");
        let job = link
            .submit(&Json::obj(vec![
                ("idempotency_key", Json::str("job-1")),
                ("request_digest", Json::str("digest-1")),
                ("job_type", Json::str("preview")),
                ("plan_digest", Json::str("plan-1")),
                ("base_commit", Json::str("HEAD")),
                ("documents", Json::Arr(Vec::new())),
            ]))
            .expect("the hub queues the job");
        assert!(!job.job_id.is_empty(), "the hub named the job");
        assert_eq!(job.status, "QUEUED");

        // the job is in the queue: the state says so, and there is no result yet
        let state = link.state(&job.job_id).expect("the state of the job");
        assert_eq!(state.status, "QUEUED");
        assert!(state.result.is_none(), "the job did not run yet");

        let _ = link.close("the test is over");
        let _ = hub_thread.join();
    }

    #[test]
    fn a_hub_that_is_not_there_is_a_readable_failure() {
        let (hub_side, client_side) = MemoryPipe::pair();
        drop(hub_side);
        let mut link = HubLink::new(
            client_side,
            "claude-code",
            &CLIENT_CAPABILITIES,
            fast_timeouts(),
        );
        let error = link.handshake().expect_err("there is no hub");
        assert_eq!(error.code, tase_contract::ErrorCode::WorkerUnavailable);
    }
}
