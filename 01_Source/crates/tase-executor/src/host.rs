//! The door to the host: the adapter that carries the edits of a plan into the editor.
//!
//! The executor never writes a file and never keeps a buffer (detailed design §8.1): it asks the host for the
//! state of the document and hands the edits to the host, which writes them in **one** editor edit.
//!
//! # Which host is in this repository
//!
//! The design book assumes the host of an IDE (`ide-kernel::Document`: a file with a version and a digest). This
//! repository is an input method: the only process that can reach the window the user is typing in is the one that
//! speaks the bridge of the text service (the `WM_COPYDATA` bridge of `pime-agent`). Therefore the host of this
//! adapter is **the text the user has in the focused window** (the selection), and not a file on the disk. The
//! path of a snapshot is the caller's name for that text; the adapter hands it back unchanged.
//!
//! # The seam
//!
//! [`HostTransport`] is the whole contract between this crate and the host: one request goes out, one answer comes
//! back. The transport that really talks to the text service lives in the product (`pime-agent`, the crate that
//! owns the bridge client); this crate only knows the seam, and its tests use a transport that answers from a
//! script.
//!
//! # The two operations of the host
//!
//! | step | operation | what it does |
//! |---|---|---|
//! | read | `Op::Read` | the text of the focused window |
//! | write | `Op::Insert` with `replace = true` | the text is written in **one** edit, replacing what was there |
//!
//! The offsets of the edits are **UTF-16** units, the unit of every position of the protocol; the core knows how to
//! apply them ([`pime_core::textrange`]), so this crate writes no second implementation of them.

use std::cell::{Cell, RefCell};

use pime_core::agent_proto::{Error as ProtoError, Op, Reply, Request};
use pime_core::sha256::sha256_hex;
use tase_contract::TaseError;

use crate::Result;

use crate::apply::{DocumentEditAdapter, HostEditOutcome, HostEditRequest};
use crate::workspace::DocumentSnapshot;

/// The seam of the host: one request goes out, one answer comes back.
///
/// The method takes `&self` because the state of a snapshot may be read while the adapter is borrowed; a transport
/// that needs state of its own keeps it inside itself.
pub trait HostTransport {
    /// Carries a request to the host and waits for its answer.
    ///
    /// `Err` means the transport itself failed and there is no answer of the host at all; an answer that says "no"
    /// (the input method that is not running, the password field, the host that refuses) is an `Ok` whose `Reply`
    /// carries the error, so that the code of the bridge travels unchanged.
    fn call(&self, request: &Request) -> Result<Reply>;
}

/// The host of the input method: the text the user has in the focused window.
pub struct BridgeHost<T> {
    transport: T,
    /// The text of the last snapshot: the edits of a plan are offsets into it.
    last: RefCell<Option<Snapshot>>,
    /// The version of the document. The host of this repository does not number its versions, so the adapter does
    /// it: the number grows with every snapshot, and the executor compares it with the one of the plan.
    version: Cell<i64>,
}

/// What the host had at the moment of a snapshot.
#[derive(Clone, Debug)]
struct Snapshot {
    path: String,
    text: String,
    sha256: String,
}

impl<T> BridgeHost<T> {
    /// A host that talks through the transport.
    pub fn new(transport: T) -> BridgeHost<T> {
        BridgeHost {
            transport,
            last: RefCell::new(None),
            version: Cell::new(0),
        }
    }

    /// The transport, so that a caller may reach the host it wraps.
    pub fn transport(&self) -> &T {
        &self.transport
    }
}

/// The request that reads the text of the focused window.
fn read_request() -> Request {
    Request::new(Op::Read)
}

/// The request that writes the text into the focused window, replacing what was there.
fn insert_request(text: &str) -> Request {
    let mut request = Request::new(Op::Insert);
    request.text = text.to_string();
    request.replace = true;
    request
}

/// Whether the answer of the host says that the write was refused (and not that it failed).
///
/// A password field, a host without a focused document and an input method that is not running are refusals: the
/// write was not made and it will not be made. Everything else (a busy text service, a malformed answer, a request
/// that was too large) leaves the question open, so it is a failure and not a refusal.
fn is_refusal(reply: &Reply) -> bool {
    matches!(
        reply.error,
        Some(
            ProtoError::Secure
                | ProtoError::Refused
                | ProtoError::NoContext
                | ProtoError::NoIme
        )
    )
}

impl<T: HostTransport> DocumentEditAdapter for BridgeHost<T> {
    fn snapshot(&self, path: &str) -> Result<DocumentSnapshot> {
        let reply = self.transport.call(&read_request())?;
        if !reply.ok {
            return Err(TaseError::new(
                tase_contract::ErrorCode::NotFound,
                format!(
                    "the host did not give the text of the focused window: {}",
                    reply
                        .error
                        .map(|error| error.as_str().to_string())
                        .unwrap_or_else(|| "no reason".to_string())
                ),
            ));
        }
        let text = reply.text;
        let sha256 = sha256_hex(&text);
        self.version.set(self.version.get() + 1);
        *self.last.borrow_mut() = Some(Snapshot {
            path: path.to_string(),
            text: text.clone(),
            sha256: sha256.clone(),
        });
        Ok(DocumentSnapshot {
            path: path.to_string(),
            version: self.version.get(),
            text,
            sha256,
            // The text of the focused window may not be saved: the adapter does not know, so it says that it may be.
            dirty: true,
        })
    }

    fn apply_edits(&mut self, request: &HostEditRequest) -> Result<HostEditOutcome> {
        // The offsets of the edits are offsets into the text of the last snapshot. The executor has just compared
        // the digest of that snapshot with the base of the plan, so this is the text the plan was made against.
        let snapshot = self.last.borrow().clone();
        let Some(snapshot) = snapshot else {
            // Nobody looked before writing: the caller is out of order, and nothing is written.
            return Ok(HostEditOutcome::Failed);
        };
        if snapshot.path != request.path {
            return Ok(HostEditOutcome::Failed);
        }
        if snapshot.sha256 != request.expected_sha256 {
            // The document changed between the preview and the apply: the offsets would land in the wrong place.
            return Ok(HostEditOutcome::Rejected);
        }
        // The edits are UTF-16 offsets: the core applies them, this crate writes no second implementation.
        let text = match pime_core::textrange::apply_edits(&snapshot.text, &request.edits) {
            Ok(text) => text,
            Err(_) => {
                // The offsets do not fit the text: the host cannot take this edit.
                return Ok(HostEditOutcome::Rejected);
            }
        };
        // ONE write, the whole text, replacing what was in the window.
        let reply = self.transport.call(&insert_request(&text))?;
        if reply.ok {
            return Ok(HostEditOutcome::Accepted);
        }
        if is_refusal(&reply) {
            return Ok(HostEditOutcome::Rejected);
        }
        // The answer does not say what happened: the result is unknown, and the executor reports it as such.
        Ok(HostEditOutcome::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::textrange::Edit;
    use std::cell::RefCell;

    /// A transport that answers from a script: the requests it received are kept, and the answer is the one the
    /// script says.
    struct ScriptedTransport {
        requests: RefCell<Vec<Request>>,
        answers: RefCell<Vec<Result<Reply>>>,
    }

    impl ScriptedTransport {
        fn new(answers: Vec<Result<Reply>>) -> ScriptedTransport {
            ScriptedTransport {
                requests: RefCell::new(Vec::new()),
                answers: RefCell::new(answers),
            }
        }

        fn requests(&self) -> Vec<Request> {
            self.requests.borrow().clone()
        }
    }

    impl HostTransport for ScriptedTransport {
        fn call(&self, request: &Request) -> Result<Reply> {
            self.requests.borrow_mut().push(request.clone());
            let mut answers = self.answers.borrow_mut();
            if answers.is_empty() {
                return Err(TaseError::not_found("the script has no answer left"));
            }
            answers.remove(0)
        }
    }

    /// The answer of a host that gives a text.
    fn answer_with(text: &str) -> Result<Reply> {
        let mut reply = Reply::ok();
        reply.text = text.to_string();
        Ok(reply)
    }

    /// The answer of a host that refuses.
    fn answer_refused(error: ProtoError) -> Result<Reply> {
        let mut reply = Reply::ok();
        reply.ok = false;
        reply.error = Some(error);
        Ok(reply)
    }

    /// The answer of a host that fails in a way that is not a refusal.
    fn answer_broken() -> Result<Reply> {
        let mut reply = Reply::ok();
        reply.ok = false;
        reply.error = Some(ProtoError::Busy);
        Ok(reply)
    }

    #[test]
    fn a_snapshot_is_the_text_of_the_focused_window() {
        let transport = ScriptedTransport::new(vec![
            answer_with("fn main() {}"),
            answer_with("fn main() {}"),
        ]);
        let host = BridgeHost::new(transport);
        let snapshot = host.snapshot("src/main.rs").expect("the host answers");
        assert_eq!(snapshot.text, "fn main() {}");
        assert_eq!(snapshot.path, "src/main.rs");
        assert_eq!(snapshot.sha256, sha256_hex("fn main() {}"));
        assert!(snapshot.dirty, "the text of the window may not be saved");
        // the version grows with every snapshot
        let second = host.snapshot("src/main.rs").expect("the host answers");
        assert!(second.version > snapshot.version);
        // the host was asked twice, with the read operation
        let requests = host.transport().requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].op, Op::Read);
        assert_eq!(requests[1].op, Op::Read);
    }

    #[test]
    fn the_edits_land_on_the_text_of_the_snapshot() {
        let transport = ScriptedTransport::new(vec![answer_with("fn main() {}"), Ok(Reply::ok())]);
        let mut host = BridgeHost::new(transport);
        let snapshot = host.snapshot("src/main.rs").expect("the host answers");
        let edits = vec![Edit {
            start: 3,
            end: 7,
            text: "run".to_string(),
        }];
        let outcome = host
            .apply_edits(&HostEditRequest {
                path: "src/main.rs".to_string(),
                expected_version: snapshot.version,
                expected_sha256: snapshot.sha256.clone(),
                edits,
                new_sha256: sha256_hex("fn run() {}"),
            })
            .expect("the host answers");
        assert_eq!(outcome, HostEditOutcome::Accepted);
        // ONE write, the whole text, replacing what was there
        let requests = host.transport().requests();
        assert_eq!(requests.len(), 2, "one read and one write");
        let write = &requests[1];
        assert_eq!(write.op, Op::Insert);
        assert!(write.replace, "the write replaces the selection");
        assert_eq!(write.text, "fn run() {}", "the whole text is written in one edit");
    }

    #[test]
    fn the_offsets_are_utf16_units_and_not_bytes() {
        // "é" is two bytes and one UTF-16 unit; the offset of the "x" is 1, not 2.
        let transport = ScriptedTransport::new(vec![answer_with("éx"), Ok(Reply::ok())]);
        let mut host = BridgeHost::new(transport);
        let snapshot = host.snapshot("a.txt").expect("the host answers");
        let outcome = host
            .apply_edits(&HostEditRequest {
                path: "a.txt".to_string(),
                expected_version: snapshot.version,
                expected_sha256: snapshot.sha256.clone(),
                edits: vec![Edit {
                    start: 1,
                    end: 2,
                    text: "y".to_string(),
                }],
                new_sha256: sha256_hex("éy"),
            })
            .expect("the host answers");
        assert_eq!(outcome, HostEditOutcome::Accepted);
        let requests = host.transport().requests();
        assert_eq!(requests[1].text, "éy");
    }

    #[test]
    fn a_document_that_changed_is_not_written() {
        let transport = ScriptedTransport::new(vec![answer_with("fn main() {}")]);
        let mut host = BridgeHost::new(transport);
        let snapshot = host.snapshot("src/main.rs").expect("the host answers");
        // the plan was made against another text
        let outcome = host
            .apply_edits(&HostEditRequest {
                path: "src/main.rs".to_string(),
                expected_version: snapshot.version,
                expected_sha256: sha256_hex("something else"),
                edits: vec![Edit {
                    start: 0,
                    end: 0,
                    text: "x".to_string(),
                }],
                new_sha256: sha256_hex("x"),
            })
            .expect("the host answers");
        assert_eq!(outcome, HostEditOutcome::Rejected);
        // nothing was written
        assert_eq!(host.transport().requests().len(), 1, "only the read went out");
    }

    #[test]
    fn an_edit_that_does_not_fit_the_text_is_refused() {
        let transport = ScriptedTransport::new(vec![answer_with("short")]);
        let mut host = BridgeHost::new(transport);
        let snapshot = host.snapshot("a.txt").expect("the host answers");
        let outcome = host
            .apply_edits(&HostEditRequest {
                path: "a.txt".to_string(),
                expected_version: snapshot.version,
                expected_sha256: snapshot.sha256.clone(),
                edits: vec![Edit {
                    start: 900,
                    end: 901,
                    text: "x".to_string(),
                }],
                new_sha256: sha256_hex("x"),
            })
            .expect("the host answers");
        assert_eq!(outcome, HostEditOutcome::Rejected);
        assert_eq!(host.transport().requests().len(), 1, "nothing was written");
    }

    #[test]
    fn a_write_without_a_snapshot_is_not_made() {
        let transport = ScriptedTransport::new(vec![Ok(Reply::ok())]);
        let mut host = BridgeHost::new(transport);
        let outcome = host
            .apply_edits(&HostEditRequest {
                path: "a.txt".to_string(),
                expected_version: 1,
                expected_sha256: sha256_hex("a"),
                edits: Vec::new(),
                new_sha256: sha256_hex("a"),
            })
            .expect("the host answers");
        assert_eq!(outcome, HostEditOutcome::Failed);
        assert!(host.transport().requests().is_empty(), "nothing went out");
    }

    #[test]
    fn a_refusal_is_a_refusal_and_not_a_failure() {
        for error in [
            ProtoError::Secure,
            ProtoError::Refused,
            ProtoError::NoContext,
            ProtoError::NoIme,
        ] {
            let transport = ScriptedTransport::new(vec![answer_with("a"), answer_refused(error)]);
            let mut host = BridgeHost::new(transport);
            let snapshot = host.snapshot("a.txt").expect("the host answers");
            let outcome = host
                .apply_edits(&HostEditRequest {
                    path: "a.txt".to_string(),
                    expected_version: snapshot.version,
                    expected_sha256: snapshot.sha256.clone(),
                    edits: vec![Edit {
                        start: 0,
                        end: 1,
                        text: "b".to_string(),
                    }],
                    new_sha256: sha256_hex("b"),
                })
                .expect("the host answers");
            assert_eq!(outcome, HostEditOutcome::Rejected, "{error:?}");
        }
    }

    #[test]
    fn an_answer_that_says_nothing_leaves_the_result_open() {
        let transport = ScriptedTransport::new(vec![answer_with("a"), answer_broken()]);
        let mut host = BridgeHost::new(transport);
        let snapshot = host.snapshot("a.txt").expect("the host answers");
        let outcome = host
            .apply_edits(&HostEditRequest {
                path: "a.txt".to_string(),
                expected_version: snapshot.version,
                expected_sha256: snapshot.sha256.clone(),
                edits: vec![Edit {
                    start: 0,
                    end: 1,
                    text: "b".to_string(),
                }],
                new_sha256: sha256_hex("b"),
            })
            .expect("the host answers");
        assert_eq!(outcome, HostEditOutcome::Failed);
    }

    #[test]
    fn a_host_that_cannot_be_reached_is_an_error() {
        let transport = ScriptedTransport::new(Vec::new());
        let host = BridgeHost::new(transport);
        let error = host.snapshot("a.txt").expect_err("the host is not there");
        assert_eq!(error.code, tase_contract::ErrorCode::NotFound);
    }
}
