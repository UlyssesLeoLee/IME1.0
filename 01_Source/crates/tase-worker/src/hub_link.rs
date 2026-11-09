//! The link of the worker to the Hub: the worker takes a job, it runs it, and it tells the Hub what
//! happened (detailed design §17.2 and §17.5).
//!
//! # What the link is
//!
//! The worker of this crate used to have no door to the Hub: the consumer of [`crate::consumer`]
//! took its jobs out of a queue that lived in the process (or in the store), and the binary ran
//! with a host that refused every job. This module is the door:
//!
//! * [`HubLink`] is the client side of the pipe of the Hub. It speaks the frames of
//!   `tase-local-ipc` and asks for the job that may run now;
//! * [`TakenJob`] is the job the Hub handed out, with the request of the job and the **fencing
//!   token** of the handout;
//! * [`HubJobRunner`] is the part of the worker that runs the job. The runner of the crate
//!   ([`crate::runner::Runner`]) computes the diff of the executor; the worker of this repository
//!   has no host editor, so it never writes into a document of the user: the diff goes back to the
//!   Hub, and the client that asked for the job is the one that applies it.
//!
//! # The shape of one turn
//!
//! ```text
//! worker                                  Hub
//!   hello  {protocol_version, client_id}  --->
//!                                   <---  hello_ack
//!   request {method: "job.take"}         --->
//!                                   <---  response {result: {available, job, fencing_token}}
//!   request {method: "job.complete",     --->
//!            params: {job_id, fencing_token, status, result}}
//!                                   <---  response {result: {accepted}}
//! ```
//!
//! Three rules the module keeps:
//!
//! * **an empty queue is not a failure.** [`HubLink::take`] answers `None` when there is no job,
//!   and the caller decides how long it waits: the library never sleeps.
//! * **a failure is told as it is.** A job that did not run is reported as failed, with the code
//!   of the failure. The link never announces a job that was not done.
//! * **the token goes back.** The report of a worker carries the token of the handout. The Hub
//!   refuses a report whose token is not the current one of the job (the store does not check the
//!   epoch of a job), so a worker that was replaced may not finish the job of another worker.
//!
//! # The time
//!
//! The link never reads the wall clock: the time of the caller is given to it, and the deadlines of
//! the pipe are the ones of `tase-local-ipc`.

use pime_core::json::Json;
use tase_contract::{ErrorCode, TaseError};
use tase_local_ipc::{Connection, FrameError, Hello, Timeouts, Transport};
use tase_executor::preview::Preview;

use crate::runner::{DocumentInput, RunRequest, Runner};

/// The method the Hub answers with the job that may run now.
pub const TAKE_METHOD: &str = "job.take";

/// The method the worker tells the Hub that the job is done.
pub const COMPLETE_METHOD: &str = "job.complete";

/// The capabilities the worker asks the Hub for.
///
/// The worker writes nothing into a document of the user: it asks for the jobs and it reports
/// what it did. `job:write` is the capability of that.
pub const WORKER_CAPABILITIES: [&str; 1] = ["job:write"];

/// A job that the Hub handed to the worker.
#[derive(Clone, Debug, PartialEq)]
pub struct TakenJob {
    /// The id of the job.
    pub job_id: String,
    /// The kind of the job (`apply`, `preview`, `verify`, ...).
    pub job_type: String,
    /// The agent the job belongs to.
    pub agent_id: String,
    /// The session that asked for the job.
    pub session_id: String,
    /// The workspace the job works in.
    pub workspace_id: String,
    /// The priority of the job inside the queue of its agent.
    pub priority: u8,
    /// The time the job was put in the queue.
    pub enqueued_at_ms: u64,
    /// The token of the handout: the worker sends it back with its report.
    ///
    /// The Hub refuses a report whose token is not the current one of the job, so a worker that
    /// was replaced may not finish the job of the worker that replaced it.
    pub fencing_token: u64,
    /// The request of the job, as the client sent it.
    ///
    /// The Hub does not read it: it carries it from the client to the worker. A job that comes
    /// from the store after a restart has no request, and the field is [`Json::Null`].
    pub request: Json,
}

/// What the link did with one job.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkOutcome {
    /// There was no job: the caller decides how long it waits.
    Idle,
    /// The job is done, and the Hub knows it.
    Completed {
        /// The id of the job.
        job_id: String,
    },
    /// The job failed, and the failure was reported to the Hub as it is.
    Failed {
        /// The id of the job.
        job_id: String,
        /// The business code of the failure.
        code: ErrorCode,
        /// The message of the failure.
        message: String,
    },
    /// The Hub refused the report: the worker is old (the token was not the current one).
    Rejected {
        /// The id of the job.
        job_id: String,
        /// Why the Hub refused the report.
        reason: String,
    },
}

impl LinkOutcome {
    /// Whether the link ran a job.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        !matches!(self, LinkOutcome::Idle)
    }

    /// Whether the job is done.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, LinkOutcome::Completed { .. })
    }
}

/// The part of the worker that runs the job the Hub handed out.
///
/// The trait is the seam of the runner: the link takes the job, runs it and reports the result. A
/// runner that fails gives the reason, and the link reports the failure - it never announces a job
/// that was not done.
pub trait HubJobRunner {
    /// Runs the job and gives back the result of it.
    fn run(&mut self, job: &TakenJob, now_ms: u64) -> Result<Json, TaseError>;
}

/// The runner that computes the diff of a job and writes nothing.
///
/// The diff is the diff of the executor ([`crate::runner::Runner`]): the worker writes no second
/// diff. The worker of this repository has no host editor, so the job stops at the preview: the
/// apply to the window of the user is the business of the client that asked for the job.
#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewRunner;

impl HubJobRunner for PreviewRunner {
    fn run(&mut self, job: &TakenJob, _now_ms: u64) -> Result<Json, TaseError> {
        let request = run_request_of(job)?;
        let preview = Runner::preview_of(&request)?;
        Ok(preview_to_json(&preview))
    }
}

/// The link of the worker to the Hub.
pub struct HubLink<T: Transport> {
    connection: Connection<T>,
    client_id: String,
}

impl<T: Transport> HubLink<T> {
    /// A link over a transport.
    ///
    /// The transport is the pipe of the Hub: a named pipe in production
    /// ([`tase_local_ipc::WindowsPipe`]), a memory pipe in the tests. The deadlines are the ones
    /// of the pipe.
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

    /// Greets the Hub and learns what was granted.
    pub fn handshake(&mut self) -> Result<(), TaseError> {
        let hello = Hello::new(&self.client_id, &WORKER_CAPABILITIES);
        self.connection
            .handshake(&hello)
            .map(|_| ())
            .map_err(map_frame_error)
    }

    /// Asks the Hub for the job that may run now.
    ///
    /// `None` means that there is no job: the queue is empty, or the Hub runs as many jobs as it
    /// may. It is not an error: the worker polls, and the caller decides how long it waits.
    pub fn take(&mut self) -> Result<Option<TakenJob>, TaseError> {
        let answer = self
            .connection
            .request(TAKE_METHOD, &Json::Obj(Vec::new()))
            .map_err(map_frame_error)?;
        if answer.get("available").and_then(Json::as_bool) != Some(true) {
            return Ok(None);
        }
        let job = answer
            .get("job")
            .ok_or_else(|| TaseError::internal("the answer of the hub has no job"))?;
        let job_id = job
            .get("job_id")
            .and_then(Json::as_str)
            .ok_or_else(|| TaseError::internal("the job of the answer has no id"))?
            .to_string();
        let token = answer
            .get("fencing_token")
            .and_then(Json::as_i64)
            .and_then(|number| u64::try_from(number).ok())
            .ok_or_else(|| TaseError::internal("the answer of the hub has no fencing token"))?;
        Ok(Some(TakenJob {
            job_id,
            job_type: text_of(job, "job_type"),
            agent_id: text_of(job, "agent_id"),
            session_id: text_of(job, "session_id"),
            workspace_id: text_of(job, "workspace_id"),
            priority: job
                .get("priority")
                .and_then(Json::as_i64)
                .and_then(|number| u8::try_from(number).ok())
                .unwrap_or(0),
            enqueued_at_ms: job
                .get("enqueued_at_ms")
                .and_then(Json::as_i64)
                .and_then(|number| u64::try_from(number).ok())
                .unwrap_or(0),
            fencing_token: token,
            request: answer.get("request").cloned().unwrap_or(Json::Null),
        }))
    }

    /// Tells the Hub what happened to the job.
    ///
    /// The report carries the token of the handout: the Hub refuses a report of a worker that was
    /// replaced, and a job that is refused is not finished.
    pub fn complete(
        &mut self,
        job: &TakenJob,
        status: &str,
        result: Option<Json>,
        error_code: Option<ErrorCode>,
    ) -> Result<(), TaseError> {
        let mut fields = vec![
            ("job_id", Json::str(&job.job_id)),
            ("fencing_token", Json::int(job.fencing_token as i64)),
            ("status", Json::str(status)),
        ];
        if let Some(result) = result {
            fields.push(("result", result));
        }
        if let Some(code) = error_code {
            fields.push(("error_code", Json::str(code.as_str())));
        }
        let answer = self
            .connection
            .request(COMPLETE_METHOD, &Json::obj(fields))
            .map_err(map_frame_error)?;
        if answer.get("accepted").and_then(Json::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(TaseError::internal(
                "the hub did not accept the report of the job",
            ))
        }
    }

    /// Takes one job, runs it and tells the Hub what happened.
    ///
    /// The link never waits: it answers [`LinkOutcome::Idle`] when there is no job, and the caller
    /// decides how long it waits before it asks again.
    pub fn run_once(
        &mut self,
        runner: &mut dyn HubJobRunner,
        now_ms: u64,
    ) -> Result<LinkOutcome, TaseError> {
        let Some(job) = self.take()? else {
            return Ok(LinkOutcome::Idle);
        };
        let job_id = job.job_id.clone();
        match runner.run(&job, now_ms) {
            Ok(result) => match self.complete(&job, "COMMITTED", Some(result), None) {
                Ok(()) => Ok(LinkOutcome::Completed { job_id }),
                Err(error) => Ok(LinkOutcome::Rejected {
                    job_id,
                    reason: error.message,
                }),
            },
            Err(error) => {
                // The job did not run: the failure is reported as it is. The link never announces
                // a job that was not done.
                let code = error.code;
                let message = error.message.clone();
                match self.complete(&job, "FAILED", None, Some(code)) {
                    Ok(()) => Ok(LinkOutcome::Failed {
                        job_id,
                        code,
                        message,
                    }),
                    Err(report) => Ok(LinkOutcome::Rejected {
                        job_id,
                        reason: report.message,
                    }),
                }
            }
        }
    }

    /// Ends the connection: the peer is told why, best effort.
    pub fn close(&mut self, reason: &str) -> Result<(), TaseError> {
        self.connection.close(reason).map_err(map_frame_error)
    }
}

/// The run request of a job, out of the request the client sent.
///
/// The request of the job is free text: the Hub does not read it, it carries it. The worker reads
/// the fields the runner of the crate needs, and a request that has none of them gives an empty
/// job: there is nothing to diff.
pub fn run_request_of(job: &TakenJob) -> Result<RunRequest, TaseError> {
    let request = &job.request;
    Ok(RunRequest {
        job_id: job.job_id.clone(),
        plan_digest: text_of(request, "plan_digest"),
        base_commit: text_of(request, "base_commit"),
        target_ref: text_of(request, "target_ref"),
        documents: documents_of(request)?,
    })
}

/// The documents of a job, out of the request the client sent.
fn documents_of(request: &Json) -> Result<Vec<DocumentInput>, TaseError> {
    let Some(list) = request.get("documents").and_then(Json::as_array) else {
        return Ok(Vec::new());
    };
    let mut documents = Vec::with_capacity(list.len());
    for item in list {
        let path = item
            .get("path")
            .and_then(Json::as_str)
            .ok_or_else(|| TaseError::invalid_argument("a document of the job has no path"))?;
        let symbols: Vec<String> = item
            .get("symbols")
            .and_then(Json::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut document = DocumentInput::new(
            path,
            item.get("base_version")
                .and_then(Json::as_i64)
                .unwrap_or(0),
            item.get("base_text").and_then(Json::as_str).unwrap_or_default(),
            item.get("new_text").and_then(Json::as_str).unwrap_or_default(),
        );
        document.symbols = symbols;
        documents.push(document);
    }
    Ok(documents)
}

/// The preview of the executor as the JSON that the Hub keeps and the client reads.
#[must_use]
pub fn preview_to_json(preview: &Preview) -> Json {
    Json::obj(vec![
        ("base_commit", Json::str(&preview.base_commit)),
        ("plan_digest", Json::str(&preview.plan_digest)),
        ("new_tree_digest", Json::str(&preview.new_tree_digest)),
        ("diff_text", Json::str(&preview.diff_text)),
        ("changed_paths", Json::strs(&preview.changed_paths())),
        ("requires_approval", Json::Bool(preview.requires_approval)),
        ("risk", Json::str(preview.risk.as_str())),
    ])
}

/// A text of an object of the protocol; the empty text when it is not there.
fn text_of(value: &Json, key: &str) -> String {
    value
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The failure of the pipe in the vocabulary of the contract.
///
/// The answer of the Hub that carries an error is read: the business code of the contract is
/// taken out of it, so the caller knows whether the Hub refused the request (and why) or the pipe
/// broke.
fn map_frame_error(error: FrameError) -> TaseError {
    match error {
        FrameError::Remote(text) => remote_error(&text),
        FrameError::Timeout => {
            TaseError::worker_unavailable("the hub did not answer the request in time")
        }
        FrameError::Closed => TaseError::worker_unavailable("the hub closed the pipe"),
        FrameError::ProtocolMismatch => TaseError::invalid_argument(
            "the hub speaks another version of the protocol of the pipe",
        ),
        FrameError::CapabilityDenied => TaseError::permission_denied(
            "the hub did not grant a capability the worker needs",
        ),
        FrameError::TooLarge => TaseError::invalid_argument("the frame is above the limit"),
        FrameError::Malformed | FrameError::Truncated | FrameError::UnknownKind => {
            TaseError::internal("the pipe of the hub is out of sync")
        }
        FrameError::WouldBlock => TaseError::worker_unavailable("the hub does not read the pipe"),
        FrameError::Duplicate => TaseError::internal("the hub answered the same request twice"),
        FrameError::Broken => TaseError::worker_unavailable("the pipe of the hub is broken"),
        FrameError::Io(reason) => TaseError::worker_unavailable(format!(
            "the pipe of the hub failed: {reason}"
        )),
    }
}

/// The error of the Hub, out of the answer of the pipe.
fn remote_error(text: &str) -> TaseError {
    let Ok(value) = pime_core::json::parse(text, pime_core::json::Limits::DEFAULT) else {
        return TaseError::internal("the hub answered with an error that is not JSON");
    };
    let message = value
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or("the hub refused the request")
        .to_string();
    let code = value
        .get("data")
        .and_then(|data| data.get("business_code"))
        .and_then(Json::as_str)
        .and_then(ErrorCode::from_str)
        .unwrap_or(ErrorCode::InternalError);
    TaseError::new(code, message)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tase_contract::SessionId;
    use tase_local_hub::{BoxedTransport, Hub, HubConfig, ManualClock, StaticSessionIdentity};
    use tase_local_ipc::{MemoryPipe, PeerIdentity};

    use super::*;

    const LOGON: &str = "S-1-5-5-1000-1000";
    const USER: &str = "S-1-5-21-1-2-3-1001";

    /// The identity of the Hub of the tests.
    fn identity() -> StaticSessionIdentity {
        StaticSessionIdentity::new(LOGON, USER, 3)
    }

    /// The peer of the worker: the same user of the same logon session.
    fn peer() -> PeerIdentity {
        PeerIdentity::new(USER, LOGON, 3, 4242)
    }

    /// The deadlines of the tests: short, so that a test does not wait for a minute.
    fn fast_timeouts() -> Timeouts {
        Timeouts {
            handshake: Duration::from_secs(2),
            request: Duration::from_secs(5),
            write: Duration::from_secs(2),
            read: Duration::from_secs(2),
        }
    }

    /// The Hub of the tests, with a job in its queue, and the two ends of the pipe.
    ///
    /// The pipe is a memory pipe: no named pipe is opened, and no window of the user is written.
    /// The Hub serves the connection in the thread of the test, and the worker speaks to it from
    /// the thread of the worker.
    fn hub_with_a_job(extra: Vec<(&str, Json)>) -> (Hub, SessionId, String, MemoryPipe, MemoryPipe) {
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
        // the session of the client that asks for the job
        let connection = hub
            .register_connection(&peer(), 1000)
            .expect("the connection is accepted");
        let session = connection.session.session_id.clone();
        hub.handle_message(
            &session,
            "agent.register",
            &Json::obj(vec![
                ("provider", Json::str("claude")),
                ("workspace_id", Json::str("ws_01")),
            ]),
            1000,
        )
        .expect("the agent registers");

        // the job of the client
        let mut fields = vec![
            ("idempotency_key", Json::str("key-1")),
            ("request_digest", Json::str("digest-1")),
            ("job_type", Json::str("preview")),
        ];
        fields.extend(extra);
        let answer = hub
            .handle_message(&session, "job.enqueue", &Json::obj(fields), 1000)
            .expect("the job is queued");
        let job_id = answer
            .get("job_id")
            .and_then(Json::as_str)
            .expect("the job")
            .to_string();

        // the pipe: one end is served by the Hub, the other one is the worker
        let (hub_side, worker_side) = MemoryPipe::pair();
        (hub, session, job_id, hub_side, worker_side)
    }

    /// The link of the worker, with the handshake done.
    fn link(worker_side: MemoryPipe) -> HubLink<MemoryPipe> {
        let mut link = HubLink::new(
            worker_side,
            "worker-1",
            &WORKER_CAPABILITIES,
            fast_timeouts(),
        );
        link.handshake().expect("the handshake works");
        link
    }

    /// The documents of a job of the tests.
    fn documents() -> Json {
        Json::Arr(vec![Json::obj(vec![
            ("path", Json::str("src/main.rs")),
            ("base_version", Json::int(1)),
            ("base_text", Json::str("fn main() {}\n")),
            ("new_text", Json::str("fn main() { run(); }\n")),
        ])])
    }

    /// One whole turn: the client queues a job, the worker takes it, runs it and reports it, and
    /// the client reads the result in the state of the job.
    #[test]
    fn a_job_travels_from_the_client_to_the_worker_and_the_result_comes_back() {
        let (hub, session, job_id, hub_side, worker_side) =
            hub_with_a_job(vec![("documents", documents())]);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut link = link(worker_side);
            let mut runner = PreviewRunner;
            let outcome = link.run_once(&mut runner, 1000);
            let _ = sender.send(outcome);
            let _ = link.close("the test is over");
        });

        // the Hub serves the connection of the worker
        hub.serve(BoxedTransport::new(Box::new(hub_side)), Some(peer()));
        let outcome = receiver
            .recv()
            .expect("the worker answers")
            .expect("the worker runs the job");
        worker.join().expect("the worker ends");
        assert!(outcome.is_completed(), "{outcome:?}");

        // the client polls the state of the job and finds the result
        let status = hub
            .handle_message(
                &session,
                "job.status",
                &Json::obj(vec![("job_id", Json::str(&job_id))]),
                1000,
            )
            .expect("the state of the job");
        assert_eq!(status.get("status").and_then(Json::as_str), Some("COMMITTED"));
        let result = status.get("result").expect("the result of the job");
        assert_eq!(
            result.get("changed_paths").and_then(Json::as_array).map(Vec::len),
            Some(1),
            "the diff of the executor is the result of the job"
        );
        assert!(
            result
                .get("diff_text")
                .and_then(Json::as_str)
                .is_some_and(|text| text.contains("run()")),
            "the diff of the job is in the result"
        );
    }

    /// An empty queue is not a failure: the worker is told that there is no job.
    #[test]
    fn an_empty_queue_gives_no_job_and_no_error() {
        let (hub, session, _job_id, hub_side, worker_side) = hub_with_a_job(vec![]);
        // the job of the client is taken away: the queue is empty
        let taken = hub
            .handle_message(&session, "job.take", &Json::Obj(Vec::new()), 1000)
            .expect("the hub hands the job out");
        assert_eq!(taken.get("available").and_then(Json::as_bool), Some(true));

        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut link = link(worker_side);
            let answer = link.take();
            let _ = sender.send(answer);
            let _ = link.close("the test is over");
        });
        hub.serve(BoxedTransport::new(Box::new(hub_side)), Some(peer()));
        let answer = receiver
            .recv()
            .expect("the worker answers")
            .expect("the worker asks for a job");
        worker.join().expect("the worker ends");
        assert_eq!(answer, None, "there is no job: the worker waits");
    }

    /// The report of a worker that was replaced is refused by the Hub, and the job is not
    /// finished: the store does not check the epoch, so the Hub is the fence.
    #[test]
    fn the_hub_refuses_the_report_of_a_worker_that_was_replaced() {
        let (hub, session, job_id, hub_side, worker_side) =
            hub_with_a_job(vec![("documents", documents())]);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut link = link(worker_side);
            let mut job = link.take().expect("the worker asks for a job").expect("a job");
            // the worker is old: the job was handed out again, and its token is not the current one
            job.fencing_token = job.fencing_token.saturating_add(1);
            let answer = link.complete(&job, "COMMITTED", None, None);
            let _ = sender.send((job.job_id, answer));
            let _ = link.close("the test is over");
        });

        hub.serve(BoxedTransport::new(Box::new(hub_side)), Some(peer()));
        let (reported, answer) = receiver.recv().expect("the worker answers");
        worker.join().expect("the worker ends");
        assert_eq!(reported, job_id);
        let error = answer.expect_err("the hub refuses the report");
        assert_eq!(error.code, ErrorCode::PermissionDenied);

        // the job is not finished
        let status = hub
            .handle_message(
                &session,
                "job.status",
                &Json::obj(vec![("job_id", Json::str(&job_id))]),
                1000,
            )
            .expect("the state of the job");
        assert_ne!(
            status.get("status").and_then(Json::as_str),
            Some("COMMITTED"),
            "the job of the refused report is not finished"
        );
    }

    /// A job that did not run is reported as it is: the link never announces a job that was not
    /// done.
    #[test]
    fn a_job_that_did_not_run_is_reported_as_a_failure() {
        struct Failing;
        impl HubJobRunner for Failing {
            fn run(&mut self, _job: &TakenJob, _now_ms: u64) -> Result<Json, TaseError> {
                Err(TaseError::worker_unavailable("the host refuses the job"))
            }
        }
        let (hub, session, job_id, hub_side, worker_side) =
            hub_with_a_job(vec![("documents", documents())]);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut link = link(worker_side);
            let mut runner = Failing;
            let outcome = link.run_once(&mut runner, 1000);
            let _ = sender.send(outcome);
            let _ = link.close("the test is over");
        });

        hub.serve(BoxedTransport::new(Box::new(hub_side)), Some(peer()));
        let outcome = receiver
            .recv()
            .expect("the worker answers")
            .expect("the worker runs the job");
        worker.join().expect("the worker ends");
        match &outcome {
            LinkOutcome::Failed { code, .. } => {
                assert_eq!(*code, ErrorCode::WorkerUnavailable)
            }
            other => panic!("the job must not be announced as done: {other:?}"),
        }

        // the Hub knows that the job failed
        let status = hub
            .handle_message(
                &session,
                "job.status",
                &Json::obj(vec![("job_id", Json::str(&job_id))]),
                1000,
            )
            .expect("the state of the job");
        assert_eq!(status.get("status").and_then(Json::as_str), Some("FAILED"));
    }

    /// A job of the tests.
    fn job() -> TakenJob {
        TakenJob {
            job_id: "job_000000000001".to_string(),
            job_type: "preview".to_string(),
            agent_id: "agent_01".to_string(),
            session_id: "sess_01".to_string(),
            workspace_id: "ws_01".to_string(),
            priority: 0,
            enqueued_at_ms: 1000,
            fencing_token: 7,
            request: Json::Obj(Vec::new()),
        }
    }

    #[test]
    fn the_run_request_is_built_out_of_the_request_of_the_job() {
        let mut job = job();
        job.request = Json::obj(vec![
            ("plan_digest", Json::str("sha256:plan")),
            ("base_commit", Json::str("commit-1")),
            ("target_ref", Json::str("refs/heads/main")),
            (
                "documents",
                Json::Arr(vec![Json::obj(vec![
                    ("path", Json::str("src/main.rs")),
                    ("base_version", Json::int(3)),
                    ("base_text", Json::str("old\n")),
                    ("new_text", Json::str("new\n")),
                    ("symbols", Json::strs(&["main"])),
                ])]),
            ),
        ]);
        let request = run_request_of(&job).expect("the request is built");
        assert_eq!(request.job_id, "job_000000000001");
        assert_eq!(request.plan_digest, "sha256:plan");
        assert_eq!(request.base_commit, "commit-1");
        assert_eq!(request.target_ref, "refs/heads/main");
        assert_eq!(request.documents.len(), 1);
        assert_eq!(request.documents[0].path, "src/main.rs");
        assert_eq!(request.documents[0].base_version, 3);
        assert_eq!(request.documents[0].symbols, ["main"]);
    }

    /// A job whose request has no document gives an empty job: there is nothing to diff.
    #[test]
    fn a_request_without_documents_gives_an_empty_job() {
        let request = run_request_of(&job()).expect("the request is built");
        assert!(request.documents.is_empty());
    }

    /// A document without a path is a mistake of the client: the job may not run.
    #[test]
    fn a_document_without_a_path_is_refused() {
        let mut job = job();
        job.request = Json::obj(vec![(
            "documents",
            Json::Arr(vec![Json::obj(vec![("new_text", Json::str("x"))])]),
        )]);
        let error = run_request_of(&job).expect_err("the document is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    /// The runner of the link computes the diff of the executor and writes nothing.
    #[test]
    fn the_runner_of_the_link_computes_the_diff() {
        let mut job = job();
        job.request = Json::obj(vec![
            ("plan_digest", Json::str("sha256:plan")),
            ("base_commit", Json::str("commit-1")),
            (
                "documents",
                Json::Arr(vec![Json::obj(vec![
                    ("path", Json::str("src/main.rs")),
                    ("base_text", Json::str("fn main() {}\n")),
                    ("new_text", Json::str("fn main() { run(); }\n")),
                ])]),
            ),
        ]);
        let mut runner = PreviewRunner;
        let result = runner.run(&job, 1000).expect("the preview is built");
        assert_eq!(
            result.get("base_commit").and_then(Json::as_str),
            Some("commit-1")
        );
        assert_eq!(
            result.get("changed_paths").and_then(Json::as_array).map(Vec::len),
            Some(1)
        );
        assert!(
            result
                .get("diff_text")
                .and_then(Json::as_str)
                .is_some_and(|text| text.contains("run()")),
            "the diff of the executor is in the result"
        );
    }

    /// A runner that fails gives the reason: the link reports the failure, it does not announce
    /// a job that was not done.
    #[test]
    fn a_runner_that_fails_gives_the_reason() {
        struct Failing;
        impl HubJobRunner for Failing {
            fn run(&mut self, _job: &TakenJob, _now_ms: u64) -> Result<Json, TaseError> {
                Err(TaseError::worker_unavailable("the host refuses the job"))
            }
        }
        let mut runner = Failing;
        let error = runner.run(&job(), 1000).expect_err("the runner fails");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
    }

    /// The error of the Hub is read: the business code of the contract comes out of it.
    #[test]
    fn the_error_of_the_hub_carries_the_code_of_the_contract() {
        let text = r#"{"code":403,"message":"the token is not the current one","data":{"business_code":"PERMISSION_DENIED","retryable":false}}"#;
        let error = remote_error(text);
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.contains("token"));
    }

    /// A pipe that is closed is a failure of the link, not a panic.
    #[test]
    fn a_closed_pipe_is_a_readable_failure() {
        let (hub_side, worker_side) = MemoryPipe::pair();
        drop(hub_side);
        let mut link = HubLink::new(
            worker_side,
            "worker-1",
            &WORKER_CAPABILITIES,
            Timeouts {
                handshake: Duration::from_millis(200),
                request: Duration::from_millis(200),
                write: Duration::from_millis(200),
                read: Duration::from_millis(200),
            },
        );
        let error = link.handshake().expect_err("the handshake fails");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
    }
}
