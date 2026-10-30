//! An authenticated connection: framed, size-limited, deadline-bound JSON messages over any [`Transport`].
//!
//! Failure policy (fail closed): a malformed or oversized frame answers with one coarse error frame and closes the
//! connection; a timeout in the middle of a frame, a failed write and every earlier error leave the connection
//! *broken* and every later call returns `Err(Broken)`. The only recoverable error is an idle `Timeout` on `recv`
//! (nothing of a frame had arrived), which lets a server loop wake up to check its stop flag.

use crate::error::IpcError;
use crate::frame::{self, Frame, FrameClass, Limits, Transport};
use crate::handshake::{SessionInfo, send_error};
use pime_core::json;
use std::time::{Duration, Instant};

/// The deadlines of one endpoint.
///
/// DD 2.1 requires explicit deadlines ("Bridge 请求与 Apply 有显式截止时间") but fixes no numbers. These are derived from
/// what the design does state: the 500 ms `ttl_ms` of the `PredictLocal` example (DD 2.2) bounds a request, and
/// NFR-102/UT-067/IT-004 require Direct input within 2 s of a Core failure, so connect + handshake (the slowest path
/// before Direct) must stay under that: 500 ms + 1000 ms = 1.5 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Opening the pipe, including waiting for a busy instance.
    pub connect: Duration,
    /// The whole handshake (hello to ready), one shared deadline.
    pub handshake: Duration,
    /// One `recv` (and the reply wait of `request`).
    pub read: Duration,
    /// One `send`.
    pub write: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            connect: Duration::from_millis(500),
            handshake: Duration::from_millis(1000),
            read: Duration::from_millis(500),
            write: Duration::from_millis(500),
        }
    }
}

pub struct Connection<T: Transport> {
    transport: T,
    info: SessionInfo,
    limits: Limits,
    timeouts: Timeouts,
    broken: bool,
}

impl<T: Transport> Connection<T> {
    pub fn new(
        transport: T,
        info: SessionInfo,
        limits: Limits,
        timeouts: Timeouts,
    ) -> Connection<T> {
        Connection {
            transport,
            info,
            limits,
            timeouts,
            broken: false,
        }
    }

    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }

    fn poison<R>(&mut self, e: IpcError) -> Result<R, IpcError> {
        self.broken = true;
        Err(e)
    }

    /// Sends one JSON object. `Err(PayloadTooLarge)` / `Err(Malformed)` is returned without touching the stream, so
    /// the connection stays usable; a failed write poisons it (part of the frame may have been written).
    pub fn send(&mut self, class: FrameClass, text: &str) -> Result<(), IpcError> {
        self.send_within(class, text, self.timeouts.write)
    }

    pub fn send_within(
        &mut self,
        class: FrameClass,
        text: &str,
        timeout: Duration,
    ) -> Result<(), IpcError> {
        if self.broken {
            return Err(IpcError::Broken);
        }
        let buf = frame::encode(class, text.as_bytes(), &self.limits)?;
        match self.transport.write_all(&buf, Instant::now() + timeout) {
            Ok(()) => Ok(()),
            Err(e) => self.poison(e),
        }
    }

    pub fn recv(&mut self) -> Result<Frame, IpcError> {
        self.recv_within(self.timeouts.read)
    }

    /// Receives one frame within `timeout`. The payload is checked to be UTF-8 and a JSON object (depth <= 16, the
    /// limit of protocol 0.3) before it is delivered.
    pub fn recv_within(&mut self, timeout: Duration) -> Result<Frame, IpcError> {
        if self.broken {
            return Err(IpcError::Broken);
        }
        let mut consumed = false;
        let read = frame::read_frame(
            &mut self.transport,
            &self.limits,
            Instant::now() + timeout,
            &mut consumed,
        );
        let (class, payload) = match read {
            Ok(ok) => ok,
            // idle: the stream is still in sync, the caller may simply try again
            Err(IpcError::Timeout) if !consumed => return Err(IpcError::Timeout),
            Err(e) => {
                if matches!(e, IpcError::PayloadTooLarge { .. } | IpcError::Malformed(_)) {
                    send_error(&mut self.transport, &e, None);
                }
                return self.poison(e);
            }
        };
        match validate_payload(payload) {
            Ok(text) => Ok(Frame { class, text }),
            Err(e) => {
                send_error(&mut self.transport, &e, None);
                self.poison(e)
            }
        }
    }

    /// Sends a control request and waits for the reply under one deadline. After *any* failure the connection is
    /// broken: a late reply would otherwise be taken for the answer to the next request, and DD 2.1 forbids a blind
    /// repeat of a write ("禁止重复写入").
    pub fn request(&mut self, text: &str) -> Result<Frame, IpcError> {
        self.request_within(text, self.timeouts.read)
    }

    pub fn request_within(&mut self, text: &str, timeout: Duration) -> Result<Frame, IpcError> {
        let deadline = Instant::now() + timeout;
        self.send_within(FrameClass::Control, text, timeout)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.recv_within(remaining) {
            Ok(f) => Ok(f),
            Err(e) => {
                self.broken = true;
                Err(e)
            }
        }
    }
}

fn validate_payload(payload: Vec<u8>) -> Result<String, IpcError> {
    let text =
        String::from_utf8(payload).map_err(|_| IpcError::Malformed("payload is not UTF-8"))?;
    let limits = json::Limits {
        max_bytes: text.len(),
        max_depth: 16,
    };
    match json::parse(&text, limits) {
        Ok(v) if v.as_object().is_some() => Ok(text),
        Ok(_) => Err(IpcError::Malformed("payload is not a JSON object")),
        Err(_) => Err(IpcError::Malformed("payload is not valid JSON")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Script, mem_pair};
    use std::time::Duration;

    fn info() -> SessionInfo {
        // the framing layer does not depend on how the session was established
        crate::handshake::tests_support::session()
    }

    fn conn<T: Transport>(t: T) -> Connection<T> {
        Connection::new(t, info(), Limits::DEFAULT, Timeouts::default())
    }

    fn framed(class: FrameClass, payload: &[u8]) -> Vec<u8> {
        frame::encode(class, payload, &Limits::DEFAULT).unwrap()
    }

    #[test]
    fn ut_ipc_connection_round_trip() {
        let (a, b) = mem_pair();
        let (mut a, mut b) = (conn(a), conn(b));
        a.send(FrameClass::Control, r#"{"op":"health","id":1}"#)
            .unwrap();
        let f = b.recv().unwrap();
        assert_eq!(f.text, r#"{"op":"health","id":1}"#);
        assert_eq!(f.class, FrameClass::Control);
        b.send(FrameClass::Bulk, r#"{"ok":true}"#).unwrap();
        assert_eq!(a.recv().unwrap().class, FrameClass::Bulk);
    }

    #[test]
    fn ut_ipc_connection_rejects_non_utf8_and_non_json_and_closes() {
        for (name, payload) in [
            ("utf8", vec![0xc3u8, 0x28]),
            ("not json", b"{not json}".to_vec()),
            ("array", b"[1,2]".to_vec()),
            ("string", b"\"x\"".to_vec()),
            (
                "too deep",
                format!("{}{}", r#"{"a":"#.repeat(40), "1}".repeat(40)).into_bytes(),
            ),
        ] {
            let mut c = conn(Script::new(&framed(FrameClass::Control, &payload)));
            assert!(matches!(c.recv(), Err(IpcError::Malformed(_))), "{name}");
            assert!(c.is_broken(), "{name}");
            assert_eq!(c.recv(), Err(IpcError::Broken), "{name}");
            assert_eq!(
                c.send(FrameClass::Control, "{}"),
                Err(IpcError::Broken),
                "{name}"
            );
        }
    }

    #[test]
    fn ut_ipc_connection_oversized_frame_gets_one_error_frame_then_is_closed() {
        let mut h = frame::encode_header(FrameClass::Control, 1).unwrap();
        h[4..].copy_from_slice(&(70_000u32).to_le_bytes());
        let mut c = conn(Script::new(&h));
        assert!(matches!(
            c.recv(),
            Err(IpcError::PayloadTooLarge {
                len: 70_000,
                max: 65_536
            })
        ));
        assert!(c.is_broken());
        let sent = String::from_utf8_lossy(&c.transport.written[frame::HEADER_LEN..]).into_owned();
        assert_eq!(sent, r#"{"ok":false,"error":"PAYLOAD_TOO_LARGE"}"#);
    }

    #[test]
    fn ut_ipc_connection_send_refuses_oversized_without_breaking() {
        let mut c = conn(Script::new(&[]));
        let big = format!(r#"{{"a":"{}"}}"#, "x".repeat(70_000));
        assert!(matches!(
            c.send(FrameClass::Control, &big),
            Err(IpcError::PayloadTooLarge { .. })
        ));
        assert!(!c.is_broken());
        assert!(
            c.transport.written.is_empty(),
            "nothing may reach the stream"
        );
        assert!(c.send(FrameClass::Bulk, &big).is_ok());
        assert_eq!(
            c.send(FrameClass::Control, ""),
            Err(IpcError::Malformed("zero-length frame"))
        );
    }

    #[test]
    fn ut_ipc_connection_idle_timeout_is_recoverable_but_a_partial_frame_is_not() {
        let bytes = framed(FrameClass::Control, br#"{"a":1}"#);
        let mut idle = conn(Script::new(&[]).then_timeout());
        assert_eq!(
            idle.recv_within(Duration::from_millis(10)),
            Err(IpcError::Timeout)
        );
        assert!(!idle.is_broken());
        let mut partial = conn(Script::new(&bytes[..5]).then_timeout());
        assert_eq!(
            partial.recv_within(Duration::from_millis(10)),
            Err(IpcError::Timeout)
        );
        assert!(
            partial.is_broken(),
            "half a frame was consumed: the stream is out of sync"
        );
    }

    #[test]
    fn ut_ipc_request_times_out_within_the_limit_and_breaks_the_connection() {
        let (a, _silent_peer) = mem_pair();
        let mut c = conn(a);
        let started = Instant::now();
        let r = c.request_within(r#"{"op":"health"}"#, Duration::from_millis(150));
        assert_eq!(r, Err(IpcError::Timeout));
        assert!(
            started.elapsed() < Duration::from_millis(1200),
            "{:?}",
            started.elapsed()
        );
        assert!(
            c.is_broken(),
            "a late reply must never be read as the next answer"
        );
        assert_eq!(c.request(r#"{"op":"health"}"#), Err(IpcError::Broken));
    }

    #[test]
    fn ut_ipc_connection_peer_close_between_frames_is_closed_inside_a_frame_is_truncated() {
        let bytes = framed(FrameClass::Control, br#"{"a":1}"#);
        let mut c = conn(Script::new(&bytes));
        assert!(c.recv().is_ok());
        assert_eq!(c.recv(), Err(IpcError::Closed));
        let mut t = conn(Script::new(&bytes[..bytes.len() - 2]));
        assert_eq!(t.recv(), Err(IpcError::Truncated));
        assert!(t.is_broken());
    }
}
