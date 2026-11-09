//! The two operations of TASE that the agent API exposes: the preview of a change, and the write of a change into
//! the region the user has selected.
//!
//! # What is written
//!
//! The host of this repository is the input method of the application that has the focus. The unit it may change
//! is the **region the user has selected** (the caret when nothing is selected) - not a file on the disk. So the
//! text of the operation is the text of that region, and the edits are offsets into it, in UTF-16 code units (the
//! unit of every position of the protocol).
//!
//! # The order of one call
//!
//! | step | what happens | what goes over the bridge |
//! |---|---|---|
//! | read | the adapter asks the host for the text of the region | one `ime_read_selection` |
//! | compute | the edits are applied to that text, with the offsets of the core | nothing |
//! | write | the whole new text is written into the region | one `ime_insert_text` with `replace = true` |
//!
//! One call, one write: there is no second write and no half-written region. When the text of the region moved
//! between the read and the write, the operation answers `STALE_BASE` and writes nothing.

use pime_core::agent_proto::{Error, Reply, Request};
use pime_core::json::Json;
use pime_core::sha256::sha256_hex;
use pime_core::textrange::{apply_edits, Edit};
use std::cell::RefCell;
use tase_contract::TaseError;
use tase_executor::apply::{DocumentEditAdapter, HostEditOutcome, HostEditRequest};
use tase_executor::host::{BridgeHost, HostTransport};
use tase_executor::preview::{ChangeKind, FileChange};

use crate::host_transport::BridgeTransport;
use crate::Backend;

/// The name of the region the host may change.
///
/// The path of a document is a label for the unit the host changes, not a file on the disk: this one is the
/// selection at the caret.
pub const REGION: &str = "selection";

/// The preview of a change: the diff of the change, without touching the window that has the focus.
///
/// Nothing is read and nothing is written: the operation works on the text the caller gives.
pub fn preview(params: &Json, _backend: &Backend) -> Json {
    let text = match text_of(params) {
        Ok(text) => text,
        Err(message) => return invalid(&message),
    };
    let edits = match edits_of(params) {
        Ok(edits) => edits,
        Err(message) => return invalid(&message),
    };
    // The edits are applied with the offsets of the core: an offset that is outside the text, or that splits a
    // surrogate pair, is refused here, before anything is written anywhere.
    let new_text = match apply_edits(&text, &edits) {
        Ok(text) => text,
        Err(error) => return invalid(&error.message),
    };
    let change = match FileChange::new(REGION, ChangeKind::Modified, &text, &new_text, &[]) {
        Ok(change) => change,
        Err(error) => return invalid(&error.message),
    };
    Json::obj(vec![
        ("ok", Json::Bool(true)),
        ("diff", Json::str(&change.diff_text)),
        ("added", Json::int(change.added_lines as i64)),
        ("removed", Json::int(change.deleted_lines as i64)),
        ("base_sha256", Json::str(&change.base_sha256)),
        ("new_sha256", Json::str(&change.new_sha256)),
        ("text", Json::str(&new_text)),
    ])
}

/// The write of a change into the region the user has selected.
///
/// The region is read, the edits are applied to the text that was read, and the whole new text is written in one
/// edit. When the host refuses, the answer says what it refused and why.
pub fn apply(params: &Json, backend: &Backend) -> Json {
    let text = match text_of(params) {
        Ok(text) => text,
        Err(message) => return invalid(&message),
    };
    let edits = match edits_of(params) {
        Ok(edits) => edits,
        Err(message) => return invalid(&message),
    };
    // The edits are checked against the text of the caller before the host is asked anything: an illegal edit never
    // reaches the window of the user.
    let new_text = match apply_edits(&text, &edits) {
        Ok(text) => text,
        Err(error) => return invalid(&error.message),
    };
    let new_sha256 = sha256_hex(&new_text);

    let transport = RecordingTransport::new(BridgeTransport::new(
        backend.bridge_handle(),
        backend.source_for(params),
    ));
    let mut host = BridgeHost::new(transport);

    // The read: the text of the region, as the host sees it right now.
    let snapshot = match host.snapshot(REGION) {
        Ok(snapshot) => snapshot,
        Err(error) => return failure(&error, host.transport().last_answer().as_ref()),
    };
    // The base moved: the text the caller holds is not the text of the region any more.
    if snapshot.text != text {
        return Json::obj(vec![
            ("ok", Json::Bool(false)),
            ("error", Json::str("STALE_BASE")),
            (
                "message",
                Json::str(
                    "the text of the region moved since it was read: read it again and apply the edits to it",
                ),
            ),
        ]);
    }
    let request = HostEditRequest {
        path: REGION.to_string(),
        expected_version: snapshot.version,
        expected_sha256: snapshot.sha256.clone(),
        edits,
        new_sha256: new_sha256.clone(),
    };
    // The write: one edit, the whole new text, replacing the region.
    match host.apply_edits(&request) {
        Ok(HostEditOutcome::Accepted) => {
            let inserted = host
                .transport()
                .last_answer()
                .map(|reply| reply.inserted as i64)
                .unwrap_or(0);
            Json::obj(vec![
                ("ok", Json::Bool(true)),
                ("outcome", Json::str("accepted")),
                ("inserted", Json::int(inserted)),
                ("sha256", Json::str(&new_sha256)),
                ("text", Json::str(&new_text)),
            ])
        }
        Ok(HostEditOutcome::Rejected) => {
            failure(&TaseError::policy_rejected("the host refused the write"), host.transport().last_answer().as_ref())
        }
        Ok(HostEditOutcome::Failed) => failure(
            &TaseError::worker_unavailable("the host did not confirm the write"),
            host.transport().last_answer().as_ref(),
        ),
        Err(error) => failure(&error, host.transport().last_answer().as_ref()),
    }
}

/// The text of the region the edits are computed against.
fn text_of(params: &Json) -> Result<String, String> {
    match params.get("text") {
        Some(Json::Str(text)) => Ok(text.clone()),
        Some(_) => Err("the parameter text must be a string".to_string()),
        None => Err("the parameter text is required".to_string()),
    }
}

/// The edits of the parameters: objects with a start, an end and a text, in UTF-16 code units.
fn edits_of(params: &Json) -> Result<Vec<Edit>, String> {
    let Some(items) = params.get("edits").and_then(Json::as_array) else {
        return Err("the parameter edits is required and must be an array of edits".to_string());
    };
    let mut edits = Vec::with_capacity(items.len());
    for item in items {
        let start = item
            .get("start")
            .and_then(Json::as_i64)
            .ok_or("every edit needs a start, in UTF-16 code units")?;
        let end = item
            .get("end")
            .and_then(Json::as_i64)
            .ok_or("every edit needs an end, in UTF-16 code units")?;
        let text = item
            .get("text")
            .and_then(Json::as_str)
            .ok_or("every edit needs the text that replaces the range")?;
        if start < 0 || end < 0 {
            return Err("the offsets of an edit cannot be negative".to_string());
        }
        if start > end {
            return Err(format!("the edit starts at {start} and ends at {end}"));
        }
        edits.push(Edit {
            start: start as usize,
            end: end as usize,
            text: text.to_string(),
        });
    }
    Ok(edits)
}

/// The answer of a request that is not valid.
fn invalid(message: &str) -> Json {
    Json::obj(vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str("INVALID_ARGUMENT")),
        ("message", Json::str(message)),
    ])
}

/// The answer of a failure.
///
/// The code is the code of the host when the host is the one that failed (`NO_IME`, `SECURE_FIELD`, `REFUSED`,
/// `BUSY`, ...) and the code of the contract when the executor is the one that failed (`INVALID_ARGUMENT`,
/// `STALE_BASE`, ...). The message always keeps the words of the host.
fn failure(error: &TaseError, answer: Option<&Reply>) -> Json {
    let (code, message) = match answer {
        Some(reply) if !reply.ok => (
            reply.error.unwrap_or(Error::Refused).as_str().to_string(),
            if reply.message.is_empty() {
                error.message.clone()
            } else {
                reply.message.clone()
            },
        ),
        _ => (error.code.as_str().to_string(), error.message.clone()),
    };
    Json::obj(vec![
        ("ok", Json::Bool(false)),
        ("error", Json::str(&code)),
        ("message", Json::str(&message)),
    ])
}

/// A transport that keeps the answers of the host.
///
/// The outcome of a write says whether the host wrote the text, but not why it did not: the answer of the host is
/// the one that says why, and the tool names its code again.
struct RecordingTransport<T> {
    inner: T,
    answers: RefCell<Vec<Reply>>,
}

impl<T> RecordingTransport<T> {
    fn new(inner: T) -> RecordingTransport<T> {
        RecordingTransport {
            inner,
            answers: RefCell::new(Vec::new()),
        }
    }

    /// The last answer of the host, when there is one.
    fn last_answer(&self) -> Option<Reply> {
        self.answers.borrow().last().cloned()
    }
}

impl<T: HostTransport> HostTransport for RecordingTransport<T> {
    fn call(&self, request: &Request) -> Result<Reply, TaseError> {
        let reply = self.inner.call(request)?;
        self.answers.borrow_mut().push(reply.clone());
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::FakeBackend;
    use pime_core::agent_proto::Op;
    use pime_core::json::{self, Limits};

    fn params(text: &str) -> Json {
        json::parse(text, Limits::DEFAULT).expect("the parameters are JSON")
    }

    #[test]
    fn the_preview_shows_the_diff_and_writes_nothing() {
        let backend = FakeBackend::new();
        let answer = preview(
            &params(r#"{"text":"fn main() {}\n","edits":[{"start":13,"end":13,"text":"    let x = 1;\n"}]}"#),
            backend.backend(),
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(true)));
        let diff = answer.get("diff").and_then(Json::as_str).expect("the diff");
        assert!(diff.contains("+    let x = 1;"), "{diff}");
        assert_eq!(answer.get("added").and_then(Json::as_i64), Some(1));
        assert_eq!(answer.get("removed").and_then(Json::as_i64), Some(0));
        // nothing went over the bridge
        assert!(backend.requests().is_empty(), "the preview touches nothing");
    }

    #[test]
    fn the_preview_refuses_an_edit_that_does_not_fit_the_text() {
        let backend = FakeBackend::new();
        let answer = preview(
            &params(r#"{"text":"short","edits":[{"start":900,"end":901,"text":"x"}]}"#),
            backend.backend(),
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(
            answer.get("error").and_then(Json::as_str),
            Some("INVALID_ARGUMENT")
        );
        assert!(backend.requests().is_empty());
    }

    #[test]
    fn the_apply_writes_once_and_reads_once() {
        let backend = FakeBackend::new();
        let answer = apply(
            &params(r#"{"text":"selected","edits":[{"start":0,"end":8,"text":"selected!"}]}"#),
            backend.backend(),
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(true)));
        assert_eq!(
            answer.get("outcome").and_then(Json::as_str),
            Some("accepted")
        );
        let requests = backend.requests();
        let writes: Vec<_> = requests
            .iter()
            .filter(|request| request.op == Op::Insert)
            .collect();
        assert_eq!(writes.len(), 1, "one call, one write");
        assert!(writes[0].replace, "the write replaces the region");
        assert_eq!(writes[0].text, "selected!");
        let reads: Vec<_> = requests
            .iter()
            .filter(|request| request.op == Op::Read)
            .collect();
        assert_eq!(reads.len(), 1, "one call, one read");
    }

    #[test]
    fn the_apply_refuses_a_text_that_is_not_the_text_of_the_region() {
        let backend = FakeBackend::new();
        let answer = apply(
            &params(r#"{"text":"something else","edits":[{"start":0,"end":0,"text":"x"}]}"#),
            backend.backend(),
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(
            answer.get("error").and_then(Json::as_str),
            Some("STALE_BASE")
        );
        let requests = backend.requests();
        let writes: Vec<_> = requests
            .iter()
            .filter(|request| request.op == Op::Insert)
            .collect();
        assert!(writes.is_empty(), "nothing is written on a stale base");
    }

    #[test]
    fn an_edit_that_is_not_an_edit_is_refused() {
        let backend = FakeBackend::new();
        let answer = apply(
            &params(r#"{"text":"selected","edits":[{"start":"0","end":1,"text":"x"}]}"#),
            backend.backend(),
        );
        assert_eq!(answer.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(
            answer.get("error").and_then(Json::as_str),
            Some("INVALID_ARGUMENT")
        );
        assert!(backend.requests().is_empty());
    }
}
