//! The connection of the Hub: the handshake, the protocol negotiation and the request/answer.
//!
//! DD §17.3 ("IPC 帧与协议协商") describes the exchange:
//!
//! ```text
//! client                                            Hub
//!   hello   {protocol_version, client_id, capabilities}  --->
//!                                                 <---  hello_ack {protocol_version, session_id, capabilities}
//!   request {jsonrpc, id, method, params}              --->
//!                                                 <---  response {jsonrpc, id, result | error}
//!   event   {..}                                     <---
//! ```
//!
//! Three rules the module keeps:
//!
//! * **the version is negotiated, never guessed.** Two peers whose major versions differ do not talk: the
//!   exchange stops with [`FrameError::ProtocolMismatch`] instead of going on with a protocol that both sides
//!   only half know.
//! * **a capability is granted, not assumed.** The Hub grants the intersection of what the client asked for and
//!   what it offers; the client refuses an answer that leaves one of its capabilities out.
//! * **an answer belongs to one request.** The sequence of the frame pairs a request with its answer, and a
//!   second answer to the same request is [`FrameError::Duplicate`], not a new answer. A request that nobody
//!   answers is given up at its deadline: the Hub does not wait for ever, and the connection is broken, because
//!   a late answer would be taken for the answer of the next request.
//!
//! The connection is generic over the transport, so it is tested with the memory pipe of the crate.

use crate::framing::{self, FrameError, FrameHeader, FrameKind, MAX_FRAME_BYTES};
use crate::transport::Transport;
use pime_core::json::{self, Json};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The version of the protocol this build speaks: the major version in front, the minor version behind.
///
/// A peer is compatible when the major versions are the same: a client of `1.0` and a Hub of `1.1` talk, a
/// client of `2.0` and a Hub of `1.1` do not.
pub const PROTOCOL_VERSION: u32 = 1001;

/// The major version of [`PROTOCOL_VERSION`].
pub const PROTOCOL_MAJOR: u32 = PROTOCOL_VERSION / 1000;

/// The major part of a version.
pub const fn protocol_major(version: u32) -> u32 {
    version / 1000
}

/// The minor part of a version.
pub const fn protocol_minor(version: u32) -> u32 {
    version % 1000
}

/// Whether a peer that speaks `version` may talk to this build.
pub const fn version_is_compatible(version: u32) -> bool {
    protocol_major(version) == PROTOCOL_MAJOR
}

/// The deadlines of one connection.
///
/// The design fixes no number (DD §17.5 fixes the budgets of the Hub, not the deadlines of a socket), so these
/// are the defaults of a local pipe: a handshake that takes longer than five seconds is not a handshake, and a
/// request that nobody answers after half a minute is abandoned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// The whole handshake (hello to hello_ack).
    pub handshake: Duration,
    /// One request and its answer.
    pub request: Duration,
    /// One write.
    pub write: Duration,
    /// One read that is not part of a request.
    pub read: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            handshake: Duration::from_secs(5),
            request: Duration::from_secs(30),
            write: Duration::from_secs(10),
            read: Duration::from_secs(10),
        }
    }
}

/// What the client says when it greets the Hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The version the client speaks.
    pub protocol_version: u32,
    /// The name of the client (`codex-cli`, `ide`, ...). A name is not an identity (DD §17.3).
    pub client_id: String,
    /// The capabilities the client asks for.
    pub capabilities: Vec<String>,
}

impl Hello {
    /// The greeting of a client that asks for the given capabilities.
    pub fn new(client_id: &str, capabilities: &[&str]) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            client_id: client_id.to_string(),
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        }
    }

    /// The greeting as the JSON of the frame.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("protocol_version", Json::int(i64::from(self.protocol_version))),
            ("client_id", Json::str(&self.client_id)),
            ("capabilities", Json::strs(&self.capabilities)),
        ])
    }

    /// The greeting out of the JSON of a frame.
    pub fn from_json(value: &Json) -> Result<Hello, FrameError> {
        Ok(Hello {
            protocol_version: u32_field(value, "protocol_version")?,
            client_id: string_field(value, "client_id")?,
            capabilities: string_list_field(value, "capabilities")?,
        })
    }
}

/// What the Hub answers when it is greeted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloAck {
    /// The version that was agreed on.
    pub protocol_version: u32,
    /// The session of this connection; every frame of the connection carries it.
    pub session_id: u64,
    /// The capabilities that were granted.
    pub capabilities: Vec<String>,
}

impl HelloAck {
    /// The answer as the JSON of the frame.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("protocol_version", Json::int(i64::from(self.protocol_version))),
            ("session_id", Json::int(self.session_id as i64)),
            ("capabilities", Json::strs(&self.capabilities)),
        ])
    }

    /// The answer out of the JSON of a frame.
    pub fn from_json(value: &Json) -> Result<HelloAck, FrameError> {
        let session_id = value
            .get("session_id")
            .and_then(Json::as_i64)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(FrameError::Malformed)?;
        Ok(HelloAck {
            protocol_version: u32_field(value, "protocol_version")?,
            session_id,
            capabilities: string_list_field(value, "capabilities")?,
        })
    }
}

/// A frame that arrived and was not the answer of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// The kind of the frame.
    pub kind: FrameKind,
    /// The sequence of the frame.
    pub sequence: u64,
    /// The payload of the frame.
    pub payload: Vec<u8>,
}

impl Event {
    /// The payload of the event as JSON.
    pub fn json(&self) -> Result<Json, FrameError> {
        parse_json(&self.payload)
    }
}

/// What a frame that arrived while the caller waited turned out to be.
enum Arrived {
    /// The answer of the request the caller was waiting for.
    Answer(Json),
    /// An event of the Hub.
    Event(Event),
    /// Nothing to do; the caller waits for more.
    Nothing,
}

/// A connection to the Hub (or from it) over a transport.
///
/// The connection is one side of a pipe: the client side speaks [`Connection::handshake`], the Hub side speaks
/// [`Connection::accept_handshake`].
pub struct Connection<T: Transport> {
    transport: T,
    /// The capabilities of this side: the ones it grants (the Hub) or the ones it asks for (a client that states
    /// them in its greeting).
    capabilities: Vec<String>,
    timeouts: Timeouts,
    session_id: u64,
    next_sequence: u64,
    /// The sequence of the request that is in flight.
    pending: Option<u64>,
    /// The sequences that were answered already, so that a second answer is recognised.
    answered: VecDeque<u64>,
    /// The events that arrived while the caller waited for something else.
    events: VecDeque<Event>,
    /// Whether an earlier failure left the stream out of sync.
    broken: bool,
}

/// How many answered sequences are remembered; enough for a client that pipelines, small enough to be free.
const ANSWERED_MEMORY: usize = 256;

impl<T: Transport> Connection<T> {
    /// A connection over a transport.
    ///
    /// `capabilities` are the capabilities of this side: the ones the Hub grants, or the ones the client asks
    /// for. `timeouts` are the deadlines of this connection.
    pub fn new(transport: T, capabilities: Vec<String>, timeouts: Timeouts) -> Connection<T> {
        Connection {
            transport,
            capabilities,
            timeouts,
            session_id: 0,
            next_sequence: 1,
            pending: None,
            answered: VecDeque::new(),
            events: VecDeque::new(),
            broken: false,
        }
    }

    /// The transport of the connection.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport of the connection, once the connection is done with it.
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// The session of the connection; 0 before the handshake.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// The capabilities of this side.
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// The sequence of the request that is in flight, when there is one.
    ///
    /// The Hub logs it: a request that is in flight is a request the Hub owes an answer to.
    pub fn pending_request(&self) -> Option<u64> {
        self.pending
    }

    /// The deadlines of the connection.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// Whether the connection may still be used.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// The name of the peer, when the transport knows it.
    pub fn peer(&self) -> Option<String> {
        self.transport.peer()
    }

    /// The client side of the handshake: greets the Hub and learns what was granted.
    ///
    /// The version is checked before anything is sent: a client that speaks another major version does not
    /// bother the Hub with a greeting it cannot answer.
    pub fn handshake(&mut self, hello: &Hello) -> Result<HelloAck, FrameError> {
        if self.broken {
            return Err(FrameError::Broken);
        }
        if !version_is_compatible(hello.protocol_version) {
            return Err(FrameError::ProtocolMismatch);
        }
        let payload = hello.to_json().to_json_string();
        self.write_frame(FrameKind::Hello, 0, payload.as_bytes())?;

        let deadline = Instant::now() + self.timeouts.handshake;
        let (header, payload) = self.read_frame(deadline)?;
        if header.kind != FrameKind::HelloAck {
            return Err(self.fail(FrameError::Malformed));
        }
        let ack = HelloAck::from_json(&parse_json(&payload)?)?;
        if !version_is_compatible(ack.protocol_version) {
            return Err(self.fail(FrameError::ProtocolMismatch));
        }
        // every capability that was asked for must be granted: a trimmed answer is not an answer
        for capability in &hello.capabilities {
            if !ack.capabilities.iter().any(|granted| granted == capability) {
                return Err(self.fail(FrameError::CapabilityDenied));
            }
        }
        self.session_id = ack.session_id;
        Ok(ack)
    }

    /// The Hub side of the handshake: reads the greeting and answers it.
    ///
    /// The granted capabilities are the intersection of what the client asked for and what this Hub offers: a
    /// capability that was not asked for is not granted, and one that was asked for but is not offered is simply
    /// not in the answer (the client refuses the answer then).
    pub fn accept_handshake(&mut self, session_id: u64) -> Result<HelloAck, FrameError> {
        if self.broken {
            return Err(FrameError::Broken);
        }
        let deadline = Instant::now() + self.timeouts.handshake;
        let (header, payload) = self.read_frame(deadline)?;
        if header.kind != FrameKind::Hello {
            return Err(self.fail(FrameError::Malformed));
        }
        let hello = Hello::from_json(&parse_json(&payload)?)?;
        if !version_is_compatible(hello.protocol_version) {
            // the client is told why, best effort: the exchange ends either way
            self.send_close("PROTOCOL_MISMATCH");
            return Err(self.fail(FrameError::ProtocolMismatch));
        }
        let granted: Vec<String> = if hello.capabilities.is_empty() {
            self.capabilities.clone()
        } else {
            self.capabilities
                .iter()
                .filter(|capability| hello.capabilities.contains(capability))
                .cloned()
                .collect()
        };
        let ack = HelloAck {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            capabilities: granted,
        };
        let payload = ack.to_json().to_json_string();
        self.write_frame(FrameKind::HelloAck, 0, payload.as_bytes())?;
        self.session_id = session_id;
        Ok(ack)
    }

    /// Asks the Hub to do something and waits for the answer.
    ///
    /// The request is a JSON-RPC 2.0 message whose `id` is the sequence of the frame: the answer is the frame
    /// with the same sequence, whatever order the answers arrive in (DD §17.3: "并行响应按 `id` 关联而非依赖到达
    /// 顺序"). A peer that does not answer is given up at the deadline; the connection is broken then, because a
    /// late answer would be taken for the answer of the next request.
    pub fn request(&mut self, method: &str, params: &Json) -> Result<Json, FrameError> {
        if self.broken {
            return Err(FrameError::Broken);
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let body = Json::obj(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::int(sequence as i64)),
            ("method", Json::str(method)),
            ("params", params.clone()),
        ]);
        let payload = body.to_json_string();
        self.write_frame(FrameKind::Request, sequence, payload.as_bytes())?;
        self.pending = Some(sequence);

        let deadline = Instant::now() + self.timeouts.request;
        loop {
            let (header, payload) = match self.read_frame(deadline) {
                Ok(frame) => frame,
                Err(error) => {
                    self.pending = None;
                    return Err(self.fail(error));
                }
            };
            match self.dispatch(header, payload, Some(sequence))? {
                Arrived::Answer(answer) => {
                    // the request is answered: nothing is in flight any more
                    self.pending = None;
                    return Ok(answer);
                }
                Arrived::Event(event) => {
                    self.events.push_back(event);
                }
                Arrived::Nothing => {}
            }
        }
    }

    /// Waits for an event of the Hub.
    ///
    /// The events that arrived while a request was waiting are answered first: they are not lost.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Event, FrameError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        if self.broken {
            return Err(FrameError::Broken);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let (header, payload) = match self.read_frame(deadline) {
                Ok(frame) => frame,
                Err(error) => return Err(self.fail(error)),
            };
            match self.dispatch(header, payload, None)? {
                Arrived::Event(event) => return Ok(event),
                Arrived::Answer(_) => {
                    // an answer without a request: the peer and this side disagree about what was asked
                    return Err(self.fail(FrameError::Malformed));
                }
                Arrived::Nothing => {}
            }
        }
    }

    /// The events that arrived while the caller was waiting for something else.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    /// Asks whether the peer is still there.
    pub fn ping(&mut self) -> Result<(), FrameError> {
        if self.broken {
            return Err(FrameError::Broken);
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.write_frame(FrameKind::Ping, sequence, &[])?;
        let deadline = Instant::now() + self.timeouts.request;
        loop {
            let (header, payload) = match self.read_frame(deadline) {
                Ok(frame) => frame,
                Err(error) => return Err(self.fail(error)),
            };
            if header.kind == FrameKind::Pong && header.sequence == sequence {
                return Ok(());
            }
            match self.dispatch(header, payload, Some(sequence))? {
                Arrived::Nothing => {}
                Arrived::Event(event) => self.events.push_back(event),
                Arrived::Answer(_) => {}
            }
        }
    }

    /// Ends the connection: the peer is told why, best effort.
    pub fn close(&mut self, reason: &str) -> Result<(), FrameError> {
        if !self.broken {
            self.send_close(reason);
        }
        self.transport.close();
        self.broken = true;
        Ok(())
    }

    /// Sends a close frame, best effort: the connection is ending anyway.
    fn send_close(&mut self, reason: &str) {
        let _ = self.write_frame(FrameKind::Close, 0, reason.as_bytes());
    }

    /// Handles a frame that arrived while the caller waited for something.
    fn dispatch(
        &mut self,
        header: FrameHeader,
        payload: Vec<u8>,
        expected: Option<u64>,
    ) -> Result<Arrived, FrameError> {
        match header.kind {
            FrameKind::Response => {
                if let Some(wanted) = expected
                    && header.sequence == wanted
                {
                    self.remember(header.sequence);
                    return Ok(Arrived::Answer(parse_response(&payload, wanted)?));
                }
                if self.answered.contains(&header.sequence) {
                    // the peer answered the same request twice: the second answer is not a new answer
                    return Err(self.fail(FrameError::Duplicate));
                }
                // an answer to a request that was never made: the two sides disagree about the stream
                Err(self.fail(FrameError::Malformed))
            }
            FrameKind::Event => Ok(Arrived::Event(Event {
                kind: header.kind,
                sequence: header.sequence,
                payload,
            })),
            FrameKind::Ping => {
                self.write_frame(FrameKind::Pong, header.sequence, &[])?;
                Ok(Arrived::Nothing)
            }
            FrameKind::Pong => Ok(Arrived::Nothing),
            FrameKind::Close => Err(self.fail(FrameError::Closed)),
            _ => Err(self.fail(FrameError::Malformed)),
        }
    }

    /// Remembers that a sequence was answered, so that a second answer is recognised.
    fn remember(&mut self, sequence: u64) {
        if self.answered.len() >= ANSWERED_MEMORY {
            self.answered.pop_front();
        }
        self.answered.push_back(sequence);
    }

    /// Writes a frame of this connection.
    fn write_frame(
        &mut self,
        kind: FrameKind,
        sequence: u64,
        payload: &[u8],
    ) -> Result<(), FrameError> {
        let header = FrameHeader {
            kind,
            session_id: self.session_id,
            sequence,
            payload_len: payload.len() as u32,
        };
        let bytes = framing::encode(&header, payload)?;
        let deadline = Instant::now() + self.timeouts.write;
        match self.transport.send_until(&bytes, deadline) {
            Ok(written) if written == bytes.len() => Ok(()),
            Ok(_) => Err(self.fail(FrameError::Io(
                "the transport wrote only a part of the frame".into(),
            ))),
            // nothing was written: the stream is still in sync, so the connection may be used again
            Err(FrameError::WouldBlock) => Err(FrameError::WouldBlock),
            Err(error) => Err(self.fail(error)),
        }
    }

    /// Reads one frame of this connection.
    fn read_frame(&mut self, deadline: Instant) -> Result<(FrameHeader, Vec<u8>), FrameError> {
        let mut prefix = [0u8; 4];
        self.read_exact(&mut prefix, deadline)?;
        let body = framing::body_len(&prefix)?;
        let mut buffer = vec![0u8; 4 + body];
        buffer[..4].copy_from_slice(&prefix);
        self.read_exact(&mut buffer[4..], deadline)?;
        framing::decode(&buffer)
    }

    /// Reads exactly `buf.len()` bytes.
    fn read_exact(&mut self, buf: &mut [u8], deadline: Instant) -> Result<(), FrameError> {
        let mut got = 0;
        while got < buf.len() {
            let read = self.transport.recv_until(&mut buf[got..], deadline)?;
            if read == 0 {
                return Err(FrameError::Closed);
            }
            got += read;
        }
        Ok(())
    }

    /// Marks the connection as broken: an earlier failure left the stream out of sync.
    fn fail(&mut self, error: FrameError) -> FrameError {
        self.broken = true;
        self.pending = None;
        error
    }
}

/// The JSON of a payload; the payload of a frame is UTF-8 JSON.
fn parse_json(payload: &[u8]) -> Result<Json, FrameError> {
    let text = std::str::from_utf8(payload).map_err(|_| FrameError::Malformed)?;
    json::parse(
        text,
        json::Limits {
            max_bytes: MAX_FRAME_BYTES,
            max_depth: 32,
        },
    )
    .map_err(|_| FrameError::Malformed)
}

/// The answer of a request: the result, or the error of the peer.
fn parse_response(payload: &[u8], sequence: u64) -> Result<Json, FrameError> {
    let value = parse_json(payload)?;
    if value.as_object().is_none() {
        return Err(FrameError::Malformed);
    }
    match value.get("id") {
        Some(Json::Num(number)) if *number == sequence as f64 => {}
        // a client may use a name of its own; the sequence of the frame pairs the answer anyway
        Some(Json::Str(_)) => {}
        _ => return Err(FrameError::Malformed),
    }
    if let Some(result) = value.get("result") {
        return Ok(result.clone());
    }
    if let Some(error) = value.get("error") {
        return Err(FrameError::Remote(error.to_json_string()));
    }
    Err(FrameError::Malformed)
}

fn u32_field(value: &Json, key: &str) -> Result<u32, FrameError> {
    value
        .get(key)
        .and_then(Json::as_i64)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or(FrameError::Malformed)
}

fn string_field(value: &Json, key: &str) -> Result<String, FrameError> {
    value
        .get(key)
        .and_then(Json::as_str)
        .map(|text| text.to_string())
        .ok_or(FrameError::Malformed)
}

fn string_list_field(value: &Json, key: &str) -> Result<Vec<String>, FrameError> {
    let list = value
        .get(key)
        .and_then(Json::as_array)
        .ok_or(FrameError::Malformed)?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        out.push(item.as_str().ok_or(FrameError::Malformed)?.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MemoryPipe;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    /// The deadlines of the tests: short, so that a test does not wait for a minute.
    fn fast_timeouts() -> Timeouts {
        Timeouts {
            handshake: Duration::from_millis(500),
            request: Duration::from_millis(300),
            write: Duration::from_millis(500),
            read: Duration::from_millis(500),
        }
    }

    /// Reads exactly `buf.len()` bytes of a pipe.
    fn read_exact(pipe: &mut MemoryPipe, buf: &mut [u8], deadline: Instant) {
        let mut got = 0;
        while got < buf.len() {
            let read = pipe
                .recv_until(&mut buf[got..], deadline)
                .expect("the bytes arrive");
            assert!(read > 0);
            got += read;
        }
    }

    /// Reads one frame of a pipe.
    fn read_frame(pipe: &mut MemoryPipe, deadline: Instant) -> (FrameHeader, Vec<u8>) {
        let mut prefix = [0u8; 4];
        read_exact(pipe, &mut prefix, deadline);
        let body = framing::body_len(&prefix).expect("the length of the frame");
        let mut buffer = vec![0u8; 4 + body];
        buffer[..4].copy_from_slice(&prefix);
        read_exact(pipe, &mut buffer[4..], deadline);
        framing::decode(&buffer).expect("the frame is read")
    }

    /// Writes one frame into a pipe.
    fn write_frame(pipe: &mut MemoryPipe, kind: FrameKind, sequence: u64, payload: &[u8]) {
        let header = FrameHeader {
            kind,
            session_id: 0,
            sequence,
            payload_len: payload.len() as u32,
        };
        let bytes = framing::encode(&header, payload).expect("the frame is written");
        pipe.send(&bytes).expect("the frame is sent");
    }

    /// The deadline of the tests.
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn the_handshake_agrees_on_the_version_and_the_capabilities() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut connection = Connection::new(
                hub_pipe,
                vec!["catalog:read".to_string(), "plan:write".to_string()],
                fast_timeouts(),
            );
            connection.accept_handshake(7)
        });
        let mut client = Connection::new(
            client_pipe,
            vec!["catalog:read".to_string()],
            fast_timeouts(),
        );
        let ack = client
            .handshake(&Hello::new("codex-cli", &["catalog:read"]))
            .expect("the handshake succeeds");
        assert_eq!(ack.protocol_version, PROTOCOL_VERSION);
        assert_eq!(ack.session_id, 7);
        assert_eq!(ack.capabilities, vec!["catalog:read".to_string()]);
        assert_eq!(client.session_id(), 7);
        let granted = hub.join().expect("the hub ends").expect("the hub agrees");
        assert_eq!(granted.capabilities, vec!["catalog:read".to_string()]);
    }

    /// The versions are packed: the major version decides whether two peers may talk.
    #[test]
    fn the_versions_are_packed_and_the_major_one_decides() {
        assert_eq!(protocol_major(PROTOCOL_VERSION), 1);
        assert_eq!(protocol_minor(PROTOCOL_VERSION), 1);
        assert!(version_is_compatible(1000));
        assert!(version_is_compatible(1001));
        assert!(version_is_compatible(1099));
        assert!(!version_is_compatible(2001));
        assert!(!version_is_compatible(0));
    }

    /// A client that speaks another major version does not even greet the Hub.
    #[test]
    fn a_version_that_does_not_match_is_refused() {
        let (mut hub_pipe, client_pipe) = MemoryPipe::pair();
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        let mut hello = Hello::new("old-client", &[]);
        hello.protocol_version = 2001;
        assert_eq!(
            client.handshake(&hello),
            Err(FrameError::ProtocolMismatch)
        );
        // nothing was sent: the Hub waits for a greeting that never comes
        let mut buf = [0u8; 1];
        assert_eq!(
            hub_pipe.recv_until(&mut buf, Instant::now() + Duration::from_millis(50)),
            Err(FrameError::Timeout)
        );
    }

    /// The Hub refuses a greeting of another major version too, and says why.
    #[test]
    fn the_hub_refuses_a_greeting_of_another_version() {
        let (hub_pipe, mut client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut connection = Connection::new(hub_pipe, Vec::new(), fast_timeouts());
            connection.accept_handshake(1)
        });
        let mut hello = Hello::new("old-client", &[]);
        hello.protocol_version = 2001;
        write_frame(
            &mut client_pipe,
            FrameKind::Hello,
            0,
            hello.to_json().to_json_string().as_bytes(),
        );
        assert_eq!(hub.join().expect("the hub ends"), Err(FrameError::ProtocolMismatch));
    }

    /// A capability that is not granted is a refused handshake, not a quiet trim.
    #[test]
    fn a_capability_that_is_not_granted_is_refused() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut connection =
                Connection::new(hub_pipe, vec!["catalog:read".to_string()], fast_timeouts());
            connection.accept_handshake(3)
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        let answer = client.handshake(&Hello::new("greedy", &["catalog:read", "code:write"]));
        assert_eq!(answer, Err(FrameError::CapabilityDenied));
        hub.join().expect("the hub ends").expect("the hub agrees");
    }

    /// A request and its answer: the sequence pairs them.
    #[test]
    fn a_request_and_its_answer() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut hub_pipe = hub_pipe;
            let (header, payload) = read_frame(&mut hub_pipe, deadline());
            assert_eq!(header.kind, FrameKind::Request);
            let request = parse_json(&payload).expect("the request is JSON");
            assert_eq!(request.get("method").and_then(Json::as_str), Some("tase.local.status"));
            assert_eq!(request.get("jsonrpc").and_then(Json::as_str), Some("2.0"));
            let answer = Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("id", Json::int(1)),
                ("result", Json::obj(vec![("queue", Json::int(3))])),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::Response,
                header.sequence,
                answer.to_json_string().as_bytes(),
            );
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        let answer = client
            .request("tase.local.status", &Json::obj(vec![("session", Json::str("s-1"))]))
            .expect("the answer arrives");
        assert_eq!(answer.get("queue").and_then(Json::as_i64), Some(3));
        assert_eq!(client.pending_request(), None, "nothing is in flight any more");
        hub.join().expect("the hub ends");
    }

    /// A second answer to the same request is not a new answer.
    #[test]
    fn a_second_answer_to_the_same_request_is_refused() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut hub_pipe = hub_pipe;
            let (header, _) = read_frame(&mut hub_pipe, deadline());
            let answer = Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("id", Json::int(header.sequence as i64)),
                ("result", Json::str("first")),
            ]);
            // the same answer twice
            for _ in 0..2 {
                write_frame(
                    &mut hub_pipe,
                    FrameKind::Response,
                    header.sequence,
                    answer.to_json_string().as_bytes(),
                );
            }
            // and the pipe is read once more so that the client may write its second request
            let _ = read_frame(&mut hub_pipe, deadline());
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        let first = client.request("tase.local.status", &Json::Null);
        assert_eq!(
            first.as_ref().ok().and_then(|v| v.as_str()),
            Some("first"),
            "{first:?}"
        );
        let second = client.request("tase.local.status", &Json::Null);
        assert_eq!(second, Err(FrameError::Duplicate));
        hub.join().expect("the hub ends");
    }

    /// A peer that does not answer: the request is given up at its deadline, and the connection is broken.
    #[test]
    fn a_peer_that_does_not_answer_makes_the_request_give_up() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let hub = thread::spawn(move || {
            let mut hub_pipe = hub_pipe;
            // the greeting is answered, the request is read and never answered
            let (header, _) = read_frame(&mut hub_pipe, deadline());
            assert_eq!(header.kind, FrameKind::Hello);
            let answer = Json::obj(vec![
                ("protocol_version", Json::int(i64::from(PROTOCOL_VERSION))),
                ("session_id", Json::int(1)),
                ("capabilities", Json::Arr(Vec::new())),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::HelloAck,
                0,
                answer.to_json_string().as_bytes(),
            );
            let _ = read_frame(&mut hub_pipe, deadline());
            while !flag.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        client
            .handshake(&Hello::new("quiet-client", &[]))
            .expect("the handshake succeeds");
        let start = Instant::now();
        let answer = client.request("tase.local.status", &Json::Null);
        assert_eq!(answer, Err(FrameError::Timeout));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "the request waits for its deadline and no longer"
        );
        // the connection is broken: a late answer would be taken for the answer of the next request
        assert!(client.is_broken());
        assert_eq!(
            client.request("tase.local.status", &Json::Null),
            Err(FrameError::Broken)
        );
        stop.store(true, Ordering::SeqCst);
        hub.join().expect("the hub ends");
    }

    /// An event that arrives while the answer is awaited is not lost.
    #[test]
    fn an_event_that_arrives_while_the_answer_is_awaited_is_kept() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut hub_pipe = hub_pipe;
            let (header, _) = read_frame(&mut hub_pipe, deadline());
            let event = Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("method", Json::str("tase.local.event")),
                ("params", Json::obj(vec![("seq", Json::int(9))])),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::Event,
                header.sequence,
                event.to_json_string().as_bytes(),
            );
            let answer = Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("id", Json::int(header.sequence as i64)),
                ("result", Json::str("done")),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::Response,
                header.sequence,
                answer.to_json_string().as_bytes(),
            );
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        let answer = client.request("tase.local.status", &Json::Null);
        assert_eq!(answer.ok().and_then(|v| v.as_str().map(|s| s.to_string())), Some("done".to_string()));
        let events = client.take_events();
        assert_eq!(events.len(), 1);
        let params = events[0].json().expect("the event is JSON");
        assert_eq!(
            params.get("params").and_then(|p| p.get("seq")).and_then(Json::as_i64),
            Some(9)
        );
        hub.join().expect("the hub ends");
    }

    /// The error of the peer is the error of the request.
    #[test]
    fn the_error_of_the_peer_is_reported() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut hub_pipe = hub_pipe;
            let (header, _) = read_frame(&mut hub_pipe, deadline());
            let answer = Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("id", Json::int(header.sequence as i64)),
                (
                    "error",
                    Json::obj(vec![
                        ("code", Json::int(-32601)),
                        ("message", Json::str("method not found")),
                    ]),
                ),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::Response,
                header.sequence,
                answer.to_json_string().as_bytes(),
            );
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        let answer = client.request("tase.unknown", &Json::Null);
        match answer {
            Err(FrameError::Remote(text)) => assert!(text.contains("method not found"), "{text}"),
            other => panic!("the error of the peer is expected, got {other:?}"),
        }
        hub.join().expect("the hub ends");
    }

    /// The handshake and the request travel in one session: the frames carry the session id.
    #[test]
    fn the_frames_carry_the_session_of_the_connection() {
        let (hub_pipe, client_pipe) = MemoryPipe::pair();
        let hub = thread::spawn(move || {
            let mut hub_pipe = hub_pipe;
            let (_header, _) = read_frame(&mut hub_pipe, deadline());
            let answer = Json::obj(vec![
                ("protocol_version", Json::int(i64::from(PROTOCOL_VERSION))),
                ("session_id", Json::int(4242)),
                ("capabilities", Json::Arr(Vec::new())),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::HelloAck,
                0,
                answer.to_json_string().as_bytes(),
            );
            let (header, _) = read_frame(&mut hub_pipe, deadline());
            assert_eq!(header.session_id, 4242, "the session of the connection");            let answer = Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("id", Json::int(header.sequence as i64)),
                ("result", Json::str("ok")),
            ]);
            write_frame(
                &mut hub_pipe,
                FrameKind::Response,
                header.sequence,
                answer.to_json_string().as_bytes(),
            );
        });
        let mut client = Connection::new(client_pipe, Vec::new(), fast_timeouts());
        client
            .handshake(&Hello::new("session-client", &[]))
            .expect("the handshake succeeds");
        let _ = client.request("tase.local.status", &Json::Null);
        hub.join().expect("the hub ends");
    }
}
