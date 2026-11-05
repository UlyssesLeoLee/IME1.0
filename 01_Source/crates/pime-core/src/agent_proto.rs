//! The wire format of the agent bridge: how an AI agent asks the input method to write text into the window that has
//! the focus.
//!
//! The input method is a TSF text service: it lives **inside** the process of the application that has the focus, and
//! only there does it know the document, the caret and the selection. An external agent (an MCP client, a CLI, a
//! script) therefore cannot write into that document by itself - it has to ask the text service of the focused
//! application to do it.
//!
//! The channel is `WM_COPYDATA`: the agent finds the bridge window of the focused process and sends it one message.
//! `WM_COPYDATA` is the message Windows provides for exactly this (a pointer to data that is copied into the receiving
//! process), and it is delivered on the **UI thread of the receiving process** - the thread that owns the text service
//! and the TSF context. The text service answers with the result of the write, and the agent learns whether the text
//! really landed in the document.
//!
//! Two rules the format keeps:
//! * the payload is **JSON** (the same strict parser as everywhere else in the core), so a payload that is not ours is
//!   refused instead of half-understood;
//! * the size is bounded **before** anything is allocated, because the payload comes from another process.
//!
//! The trust model is the one of the clipboard: any program of the user that runs in the same session can ask the
//! input method to write into the focused window. That is no more than what simulated keystrokes (SendInput) can do,
//! and a program that can do that can already type into the window.
//!
//! This module is pure: it describes the format, it does not send anything.

use crate::json::{self, Json, Limits};

/// The version of the bridge format.
pub const VERSION: u32 = 1;

/// The class name of the bridge window (one per process that has the input method active).
pub const BRIDGE_CLASS: &str = "PIME1AgentBridge";

/// The prefix of the title of the bridge window.
pub const BRIDGE_TITLE_PREFIX: &str = "PIME1.";

/// The `dwData` of a request: "PIME" plus the version in the upper half.
pub const COPYDATA_REQUEST: u32 = 0x5049_4D01;

/// The `dwData` of a reply.
pub const COPYDATA_REPLY: u32 = 0x5049_4D02;

/// The largest payload that is accepted. A megabyte of text is far beyond any code block an agent writes at once.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The limits of the JSON parser for a payload.
fn limits() -> Limits {
    Limits {
        max_bytes: MAX_PAYLOAD_BYTES,
        max_depth: 8,
    }
}

/// What the agent asks the input method to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Write text at the caret of the focused document.
    Insert,
    /// Read the text that is selected in the focused document.
    Read,
    /// Switch the language of the input method.
    Language,
    /// Ask what the input method is doing (the language, whether it is active).
    Status,
    /// The words the user typed and that had no candidate.
    Misses,
    /// Write the words an AI proposes into the word bank of the user.
    Curate,
}

impl Op {
    /// The name of the operation on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Insert => "insert",
            Op::Read => "read",
            Op::Language => "language",
            Op::Status => "status",
            Op::Misses => "misses",
            Op::Curate => "curate",
        }
    }

    /// The operation of a name; an unknown name is refused.
    pub fn from_str(name: &str) -> Option<Op> {
        match name {
            "insert" => Some(Op::Insert),
            "read" => Some(Op::Read),
            "language" => Some(Op::Language),
            "status" => Some(Op::Status),
            "misses" => Some(Op::Misses),
            "curate" => Some(Op::Curate),
            _ => None,
        }
    }

    /// Every operation of the bridge.
    pub fn all() -> [Op; 6] {
        [
            Op::Insert,
            Op::Read,
            Op::Language,
            Op::Status,
            Op::Misses,
            Op::Curate,
        ]
    }
}

/// Why a request did not do what it was asked to do.
///
/// The names are the codes the agents see: they are stable, short and safe to put into a message to a user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input method is not running in the focused window (or not at all).
    NoIme,
    /// There is no document to write into (no focused context).
    NoContext,
    /// The host refused the write.
    Refused,
    /// The field is a password field: nothing is ever written into one.
    Secure,
    /// The text service is busy (the answer would come at a bad moment); the caller may try again.
    Busy,
    /// The request was larger than the bridge accepts.
    TooLarge,
    /// The request was not one of ours.
    Malformed,
    /// The operation is not supported by this build.
    Unsupported,
}

impl Error {
    /// The code of the error on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Error::NoIme => "NO_IME",
            Error::NoContext => "NO_CONTEXT",
            Error::Refused => "REFUSED",
            Error::Secure => "SECURE_FIELD",
            Error::Busy => "BUSY",
            Error::TooLarge => "TOO_LARGE",
            Error::Malformed => "MALFORMED",
            Error::Unsupported => "UNSUPPORTED",
        }
    }

    /// The error of a code; an unknown code is refused.
    pub fn from_str(code: &str) -> Option<Error> {
        match code {
            "NO_IME" => Some(Error::NoIme),
            "NO_CONTEXT" => Some(Error::NoContext),
            "REFUSED" => Some(Error::Refused),
            "SECURE_FIELD" => Some(Error::Secure),
            "BUSY" => Some(Error::Busy),
            "TOO_LARGE" => Some(Error::TooLarge),
            "MALFORMED" => Some(Error::Malformed),
            "UNSUPPORTED" => Some(Error::Unsupported),
            _ => None,
        }
    }

    /// Whether trying again can change the answer.
    pub fn is_retryable(self) -> bool {
        matches!(self, Error::Busy)
    }
}

/// A request of the agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// What to do.
    pub op: Op,
    /// The text of the operation (the text to write, the language to switch to, the proposals to store).
    pub text: String,
    /// The language of the operation, when it names one.
    pub language: Option<String>,
    /// Whether the write replaces the selection instead of the caret.
    pub replace: bool,
    /// The window that gets the answer (0 when the caller does not want one).
    pub reply: u64,
}

impl Request {
    /// A request of an operation without anything else.
    pub fn new(op: Op) -> Request {
        Request {
            op,
            text: String::new(),
            language: None,
            replace: false,
            reply: 0,
        }
    }

    /// The request as the JSON that is sent.
    pub fn to_json(&self) -> Json {
        let mut pairs = vec![
            ("v", Json::int(VERSION as i64)),
            ("op", Json::str(self.op.as_str())),
        ];
        if !self.text.is_empty() {
            pairs.push(("text", Json::str(&self.text)));
        }
        if let Some(language) = &self.language {
            pairs.push(("language", Json::str(language)));
        }
        if self.replace {
            pairs.push(("replace", Json::Bool(true)));
        }
        if self.reply != 0 {
            pairs.push(("reply", Json::int(self.reply as i64)));
        }
        Json::obj(pairs)
    }

    /// The request of a JSON payload.
    pub fn from_json(value: &Json) -> Result<Request, Error> {
        let version = value
            .get("v")
            .and_then(Json::as_i64)
            .map(|n| n as u32)
            .unwrap_or(VERSION);
        if version != VERSION {
            return Err(Error::Malformed);
        }
        let op = value
            .get("op")
            .and_then(Json::as_str)
            .and_then(Op::from_str)
            .ok_or(Error::Malformed)?;
        let text = match value.get("text") {
            None | Some(Json::Null) => String::new(),
            Some(Json::Str(s)) => s.clone(),
            Some(_) => return Err(Error::Malformed),
        };
        let language = match value.get("language") {
            None | Some(Json::Null) => None,
            Some(Json::Str(s)) => Some(s.clone()),
            Some(_) => return Err(Error::Malformed),
        };
        let replace = match value.get("replace") {
            None | Some(Json::Null) => false,
            Some(Json::Bool(b)) => *b,
            Some(_) => return Err(Error::Malformed),
        };
        let reply = match value.get("reply") {
            None | Some(Json::Null) => 0,
            Some(Json::Num(n)) if *n >= 0.0 && n.fract() == 0.0 => *n as u64,
            Some(_) => return Err(Error::Malformed),
        };
        Ok(Request {
            op,
            text,
            language,
            replace,
            reply,
        })
    }

    /// The request of a UTF-8 payload; the payload is bounded before it is parsed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Request, Error> {
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Error::Malformed)?;
        let value = json::parse(text, limits()).map_err(|_| Error::Malformed)?;
        Request::from_json(&value)
    }
}

/// The answer of the input method.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// Whether the request did what it was asked to.
    pub ok: bool,
    /// The code of the error, when it did not.
    pub error: Option<Error>,
    /// A line of text for the user, when there is one.
    pub message: String,
    /// How many characters were written.
    pub inserted: usize,
    /// The language of the input method after the request.
    pub language: Option<String>,
    /// The text that was read.
    pub text: String,
    /// The words the user typed and that had no candidate.
    pub words: Vec<String>,
    /// How many words were written into the word bank.
    pub written: usize,
}

impl Reply {
    /// The answer of a request that worked.
    pub fn ok() -> Reply {
        Reply {
            ok: true,
            error: None,
            message: String::new(),
            inserted: 0,
            language: None,
            text: String::new(),
            words: Vec::new(),
            written: 0,
        }
    }

    /// The answer of a request that did not work.
    pub fn failed(error: Error, message: &str) -> Reply {
        Reply {
            ok: false,
            error: Some(error),
            message: message.to_string(),
            ..Reply::ok()
        }
    }

    /// The answer as the JSON that is sent back.
    pub fn to_json(&self) -> Json {
        let mut pairs = vec![
            ("v", Json::int(VERSION as i64)),
            ("ok", Json::Bool(self.ok)),
        ];
        if let Some(error) = self.error {
            pairs.push(("error", Json::str(error.as_str())));
        }
        if !self.message.is_empty() {
            pairs.push(("message", Json::str(&self.message)));
        }
        if self.inserted > 0 {
            pairs.push(("inserted", Json::int(self.inserted as i64)));
        }
        if let Some(language) = &self.language {
            pairs.push(("language", Json::str(language)));
        }
        if !self.text.is_empty() {
            pairs.push(("text", Json::str(&self.text)));
        }
        if !self.words.is_empty() {
            pairs.push((
                "words",
                Json::Arr(self.words.iter().map(|w| Json::str(w)).collect()),
            ));
        }
        if self.written > 0 {
            pairs.push(("written", Json::int(self.written as i64)));
        }
        Json::obj(pairs)
    }

    /// The answer of a JSON payload.
    pub fn from_json(value: &Json) -> Result<Reply, Error> {
        let version = value
            .get("v")
            .and_then(Json::as_i64)
            .map(|n| n as u32)
            .unwrap_or(VERSION);
        if version != VERSION {
            return Err(Error::Malformed);
        }
        let ok = match value.get("ok") {
            Some(Json::Bool(b)) => *b,
            _ => return Err(Error::Malformed),
        };
        let error = match value.get("error") {
            None | Some(Json::Null) => None,
            Some(Json::Str(s)) => Some(Error::from_str(s).ok_or(Error::Malformed)?),
            Some(_) => return Err(Error::Malformed),
        };
        let message = match value.get("message") {
            None | Some(Json::Null) => String::new(),
            Some(Json::Str(s)) => s.clone(),
            Some(_) => return Err(Error::Malformed),
        };
        let inserted = number(value, "inserted")?;
        let language = match value.get("language") {
            None | Some(Json::Null) => None,
            Some(Json::Str(s)) => Some(s.clone()),
            Some(_) => return Err(Error::Malformed),
        };
        let text = match value.get("text") {
            None | Some(Json::Null) => String::new(),
            Some(Json::Str(s)) => s.clone(),
            Some(_) => return Err(Error::Malformed),
        };
        let words = match value.get("words") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Arr(list)) => {
                let mut out = Vec::with_capacity(list.len());
                for item in list {
                    match item {
                        Json::Str(s) => out.push(s.clone()),
                        _ => return Err(Error::Malformed),
                    }
                }
                out
            }
            Some(_) => return Err(Error::Malformed),
        };
        let written = number(value, "written")?;
        Ok(Reply {
            ok,
            error,
            message,
            inserted,
            language,
            text,
            words,
            written,
        })
    }

    /// The answer of a UTF-8 payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Reply, Error> {
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Error::Malformed)?;
        let value = json::parse(text, limits()).map_err(|_| Error::Malformed)?;
        Reply::from_json(&value)
    }
}

fn number(value: &Json, key: &str) -> Result<usize, Error> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(0),
        Some(Json::Num(n)) if *n >= 0.0 && n.fract() == 0.0 => Ok(*n as usize),
        Some(_) => Err(Error::Malformed),
    }
}

/// The proposals of an AI for the word bank of the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The word itself.
    pub word: String,
    /// Its reading (the pinyin or the kana).
    pub reading: String,
    /// Why the AI proposes it.
    pub reason: String,
}

impl Proposal {
    /// The proposal as JSON.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("word", Json::str(&self.word)),
            ("reading", Json::str(&self.reading)),
            ("reason", Json::str(&self.reason)),
        ])
    }

    /// The proposal of a JSON value.
    pub fn from_json(value: &Json) -> Result<Proposal, Error> {
        let word = value
            .get("word")
            .and_then(Json::as_str)
            .ok_or(Error::Malformed)?
            .to_string();
        let reading = match value.get("reading") {
            None | Some(Json::Null) => String::new(),
            Some(Json::Str(s)) => s.clone(),
            Some(_) => return Err(Error::Malformed),
        };
        let reason = match value.get("reason") {
            None | Some(Json::Null) => String::new(),
            Some(Json::Str(s)) => s.clone(),
            Some(_) => return Err(Error::Malformed),
        };
        Ok(Proposal {
            word,
            reading,
            reason,
        })
    }
}

/// The proposals as JSON (the text of a `curate` request carries them).
pub fn proposals_to_json(proposals: &[Proposal]) -> Json {
    Json::Arr(proposals.iter().map(|p| p.to_json()).collect())
}

/// The proposals of a JSON value.
pub fn proposals_from_json(value: &Json) -> Result<Vec<Proposal>, Error> {
    let Json::Arr(list) = value else {
        return Err(Error::Malformed);
    };
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        out.push(Proposal::from_json(item)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_has_a_name_and_comes_back_from_it() {
        for op in Op::all() {
            assert_eq!(Op::from_str(op.as_str()), Some(op), "{op:?}");
        }
        assert_eq!(Op::from_str("nonsense"), None);
        assert_eq!(Op::all().len(), 6);
    }

    #[test]
    fn every_error_has_a_code_and_comes_back_from_it() {
        for error in [
            Error::NoIme,
            Error::NoContext,
            Error::Refused,
            Error::Secure,
            Error::Busy,
            Error::TooLarge,
            Error::Malformed,
            Error::Unsupported,
        ] {
            assert_eq!(Error::from_str(error.as_str()), Some(error), "{error:?}");
        }
        assert_eq!(Error::from_str("NOPE"), None);
        assert!(Error::Busy.is_retryable());
        assert!(!Error::Secure.is_retryable());
    }

    #[test]
    fn a_request_survives_the_round_trip() {
        let request = Request {
            op: Op::Insert,
            text: "你好，世界\n".to_string(),
            language: Some("zh-CN".to_string()),
            replace: true,
            reply: 4242,
        };
        let bytes = request.to_json().to_json_string();
        let back = Request::from_bytes(bytes.as_bytes()).expect("the request is read");
        assert_eq!(back, request);
    }

    #[test]
    fn a_request_without_the_optional_parts_is_read_as_the_defaults() {
        let back = Request::from_bytes(br#"{"v":1,"op":"status"}"#).expect("read");
        assert_eq!(back.op, Op::Status);
        assert!(back.text.is_empty());
        assert!(back.language.is_none());
        assert!(!back.replace);
        assert_eq!(back.reply, 0);
    }

    #[test]
    fn a_reply_survives_the_round_trip() {
        let reply = Reply {
            ok: true,
            error: None,
            message: String::new(),
            inserted: 12,
            language: Some("ja-JP".to_string()),
            text: "選択".to_string(),
            words: vec!["你好".to_string()],
            written: 3,
        };
        let bytes = reply.to_json().to_json_string();
        assert_eq!(Reply::from_bytes(bytes.as_bytes()).expect("read"), reply);
    }

    #[test]
    fn a_failed_reply_carries_its_code() {
        let reply = Reply::failed(Error::Secure, "the field is a password field");
        let bytes = reply.to_json().to_json_string();
        let back = Reply::from_bytes(bytes.as_bytes()).expect("read");
        assert!(!back.ok);
        assert_eq!(back.error, Some(Error::Secure));
        assert_eq!(back.message, "the field is a password field");
    }

    /// A payload that is not ours must be refused, not half-understood: the payload comes from another process.
    #[test]
    fn a_payload_that_is_not_ours_is_refused() {
        for bytes in [
            &b"not json"[..],
            &b"{}"[..],
            &br#"{"v":1}"#[..],
            &br#"{"v":2,"op":"insert"}"#[..],
            &br#"{"v":1,"op":"nonsense"}"#[..],
            &br#"{"v":1,"op":"insert","text":5}"#[..],
            &br#"{"v":1,"op":"insert","replace":"yes"}"#[..],
            &br#"{"v":1,"op":"insert","reply":-1}"#[..],
            &[0xff, 0xfe, 0x00][..],
        ] {
            assert_eq!(
                Request::from_bytes(bytes).err(),
                Some(Error::Malformed),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    /// The size is checked before anything is allocated: the payload comes from another process.
    #[test]
    fn a_payload_that_is_too_big_is_refused_before_it_is_parsed() {
        let big = vec![b'x'; MAX_PAYLOAD_BYTES + 1];
        assert_eq!(Request::from_bytes(&big).err(), Some(Error::TooLarge));
        let big = vec![b'x'; MAX_PAYLOAD_BYTES + 1];
        assert_eq!(Reply::from_bytes(&big).err(), Some(Error::TooLarge));
    }

    #[test]
    fn the_proposals_survive_the_round_trip() {
        let proposals = vec![Proposal {
            word: "你好".to_string(),
            reading: "nihao".to_string(),
            reason: "the user typed it and found nothing".to_string(),
        }];
        let value = proposals_to_json(&proposals);
        assert_eq!(proposals_from_json(&value).expect("read"), proposals);
        assert_eq!(proposals_from_json(&Json::Null).err(), Some(Error::Malformed));
    }

    /// The two sides of the bridge must agree on the numbers: the code of a request and of a reply.
    #[test]
    fn the_codes_of_the_bridge_are_fixed() {
        assert_eq!(COPYDATA_REQUEST, 0x5049_4D01);
        assert_eq!(COPYDATA_REPLY, 0x5049_4D02);
        assert_eq!(BRIDGE_CLASS, "PIME1AgentBridge");
        assert!(BRIDGE_TITLE_PREFIX.starts_with("PIME1"));
    }
}
