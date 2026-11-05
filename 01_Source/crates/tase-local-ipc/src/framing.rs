//! The frame of TASE: a length prefix, a frame kind, the session and the sequence, and the payload.
//!
//! DD §17.3 ("IPC 帧与协议协商") fixes the physical layer: "同机 Named Pipe 承载 JSON-RPC 2.0 消息；物理层采用
//! 4 字节小端 `u32` 的长度前缀，再跟 UTF-8 JSON 字节，默认单帧上限 1 MiB，超过立即拒绝。" This module is that
//! physical layer, plus the few fields a Hub needs to route a frame without parsing its payload: the kind of the
//! frame, the session it belongs to and the sequence that pairs a request with its answer.
//!
//! ```text
//! offset  size  field        value
//! 0       4     length       u32 little endian: the number of bytes that follow this field
//! 4       1     kind         the wire value of the frame kind (1 ..= 8)
//! 5       3     reserved     must be zero
//! 8       8     session_id   u64 little endian: the session the frame belongs to
//! 16      8     sequence     u64 little endian: 0 for the frames that are not paired, else the request number
//! 24      4     payload_len  u32 little endian: the length of the payload
//! 28      n     payload      the bytes of the frame (UTF-8 JSON for the frames of the Hub)
//! ```
//!
//! The length prefix counts the bytes *after* itself, so a reader sizes its buffer with four bytes and then reads
//! exactly that many more. `payload_len` repeats the length of the payload inside the header: the two must agree,
//! and a frame whose two lengths disagree is malformed. The redundancy is deliberate - the outer prefix lets a
//! reader bound its buffer before it knows the frame, the inner field is the length the frame itself carries.
//!
//! Two rules the module keeps:
//! * the size is checked **before** anything is allocated, because the payload comes from another process;
//! * a half frame is reported as [`FrameError::Truncated`], never as a short frame: a reader that lost the tail of
//!   a frame must not take the next bytes for a new frame.
//!
//! The module is pure: it describes the format, it does not send anything.

use std::fmt;

/// The size of the fixed header of a frame, in bytes.
pub const FRAME_HEADER_BYTES: usize = 28;

/// The largest payload a frame may carry: 1 MiB (DD §17.3, "默认单帧上限 1 MiB，超过立即拒绝").
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The largest frame on the wire (header and payload).
pub const MAX_FRAME_TOTAL_BYTES: usize = FRAME_HEADER_BYTES + MAX_FRAME_BYTES;

/// The number of bytes that follow the length prefix: the rest of the header.
const BODY_HEADER_BYTES: usize = FRAME_HEADER_BYTES - 4;

/// The kind of a frame: what the frame is for.
///
/// The wire value is a number, not a name: the header is fixed, and a byte costs nothing. The names are for the
/// logs and for the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameKind {
    /// The client greets the Hub and states its version and its capabilities.
    Hello = 1,
    /// The Hub answers the greeting with the negotiated version, the session and the granted capabilities.
    HelloAck = 2,
    /// A JSON-RPC 2.0 request.
    Request = 3,
    /// The answer of a request; the sequence of the frame is the one of the request.
    Response = 4,
    /// An unsolicited message of the Hub (the event stream of DD §17.3).
    Event = 5,
    /// A liveness probe.
    Ping = 6,
    /// The answer of a probe.
    Pong = 7,
    /// The end of the connection; the payload is the reason, if there is one.
    Close = 8,
}

impl FrameKind {
    /// The name of the kind; the same on the wire in the logs and in the code.
    pub const fn as_str(self) -> &'static str {
        match self {
            FrameKind::Hello => "hello",
            FrameKind::HelloAck => "hello_ack",
            FrameKind::Request => "request",
            FrameKind::Response => "response",
            FrameKind::Event => "event",
            FrameKind::Ping => "ping",
            FrameKind::Pong => "pong",
            FrameKind::Close => "close",
        }
    }

    /// The kind of a wire value; `None` when the byte is not one of ours.
    pub const fn from_u8(value: u8) -> Option<FrameKind> {
        match value {
            1 => Some(FrameKind::Hello),
            2 => Some(FrameKind::HelloAck),
            3 => Some(FrameKind::Request),
            4 => Some(FrameKind::Response),
            5 => Some(FrameKind::Event),
            6 => Some(FrameKind::Ping),
            7 => Some(FrameKind::Pong),
            8 => Some(FrameKind::Close),
            _ => None,
        }
    }

    /// The value of the kind on the wire.
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    /// Every kind of a frame.
    pub const fn all() -> [FrameKind; 8] {
        [
            FrameKind::Hello,
            FrameKind::HelloAck,
            FrameKind::Request,
            FrameKind::Response,
            FrameKind::Event,
            FrameKind::Ping,
            FrameKind::Pong,
            FrameKind::Close,
        ]
    }
}

/// The header of a frame: everything but the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// What the frame is for.
    pub kind: FrameKind,
    /// The session the frame belongs to; 0 before the handshake.
    pub session_id: u64,
    /// The number that pairs a request with its answer; 0 for the frames that are not paired.
    pub sequence: u64,
    /// The length of the payload.
    pub payload_len: u32,
}

/// Why a frame was refused.
///
/// The four first variants are the ones of the frame format itself; the others are the failures of the connection
/// and of the transport. Every variant has a code (`as_str`), so the two ends of a connection agree on the
/// vocabulary, and a `Display` that a human can read in a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The frame is above the limit (DD §17.3: a frame above 1 MiB is refused at once).
    TooLarge,
    /// The bytes are not a frame of ours: a bad magic, a bad reserved byte, two lengths that disagree.
    Malformed,
    /// The frame kind is not one of ours.
    UnknownKind,
    /// The frame is incomplete: the peer stopped in the middle of it.
    Truncated,
    /// The peer is not reading: the buffer is full and the frame is dropped instead of being queued forever.
    WouldBlock,
    /// The deadline passed.
    Timeout,
    /// The peer closed the connection.
    Closed,
    /// The peer speaks another version of the protocol: the exchange stops, nothing is guessed.
    ProtocolMismatch,
    /// The peer did not grant a capability that was asked for.
    CapabilityDenied,
    /// The peer answered a request twice: the second answer is not a new answer.
    Duplicate,
    /// The connection is broken: an earlier failure left the stream out of sync, so it may not be used again.
    Broken,
    /// The transport failed (the message is the reason).
    Io(String),
    /// The peer answered the request with a JSON-RPC error (the message is the error object).
    Remote(String),
}

impl FrameError {
    /// The code of the failure; the same code on both ends of the connection.
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameError::TooLarge => "PAYLOAD_TOO_LARGE",
            FrameError::Malformed => "MALFORMED_FRAME",
            FrameError::UnknownKind => "UNKNOWN_FRAME_KIND",
            FrameError::Truncated => "TRUNCATED_FRAME",
            FrameError::WouldBlock => "WOULD_BLOCK",
            FrameError::Timeout => "IPC_TIMEOUT",
            FrameError::Closed => "IPC_CLOSED",
            FrameError::ProtocolMismatch => "PROTOCOL_MISMATCH",
            FrameError::CapabilityDenied => "CAPABILITY_DENIED",
            FrameError::Duplicate => "DUPLICATE_RESPONSE",
            FrameError::Broken => "IPC_BROKEN",
            FrameError::Io(_) => "IPC_IO_ERROR",
            FrameError::Remote(_) => "REMOTE_ERROR",
        }
    }

    /// Whether the failure is about the peer and not about the machine: the codes of the identity failures are the
    /// same, so a prober cannot tell one from another.
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            FrameError::ProtocolMismatch
                | FrameError::CapabilityDenied
                | FrameError::Duplicate
                | FrameError::Remote(_)
        )
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooLarge => write!(
                f,
                "PAYLOAD_TOO_LARGE: the payload is above the limit of {MAX_FRAME_BYTES} bytes"
            ),
            FrameError::Malformed => write!(f, "MALFORMED_FRAME: the bytes are not a TASE frame"),
            FrameError::UnknownKind => write!(f, "UNKNOWN_FRAME_KIND: the frame kind is not one of ours"),
            FrameError::Truncated => write!(f, "TRUNCATED_FRAME: the frame stops in the middle"),
            FrameError::WouldBlock => write!(
                f,
                "WOULD_BLOCK: the peer is not reading, the frame is dropped instead of queued"
            ),
            FrameError::Timeout => write!(f, "IPC_TIMEOUT: the deadline passed"),
            FrameError::Closed => write!(f, "IPC_CLOSED: the peer closed the connection"),
            FrameError::ProtocolMismatch => {
                write!(f, "PROTOCOL_MISMATCH: the peer speaks another version")
            }
            FrameError::CapabilityDenied => {
                write!(f, "CAPABILITY_DENIED: the peer did not grant a capability")
            }
            FrameError::Duplicate => {
                write!(f, "DUPLICATE_RESPONSE: the peer answered the same request twice")
            }
            FrameError::Broken => write!(
                f,
                "IPC_BROKEN: the connection is out of sync and may not be used again"
            ),
            FrameError::Io(why) => write!(f, "IPC_IO_ERROR: {why}"),
            FrameError::Remote(why) => write!(f, "REMOTE_ERROR: {why}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// The size of the frame of a header: the header and the payload.
pub fn frame_len(header: &FrameHeader) -> usize {
    FRAME_HEADER_BYTES + header.payload_len as usize
}

/// The number of bytes that follow the length prefix, read from the prefix alone.
///
/// The reader of a stream calls this with the four bytes it has just read: the answer tells it how much more it
/// has to read. A prefix that announces more than the limit is refused here, before a buffer is reserved.
pub fn body_len(prefix: &[u8]) -> Result<usize, FrameError> {
    if prefix.len() < 4 {
        return Err(FrameError::Truncated);
    }
    let body = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    if body < BODY_HEADER_BYTES {
        // a frame that is shorter than its own header
        return Err(FrameError::Malformed);
    }
    if body > BODY_HEADER_BYTES + MAX_FRAME_BYTES {
        // the limit is checked here: the peer may not make the receiver reserve more than the limit
        return Err(FrameError::TooLarge);
    }
    Ok(body)
}

/// The header of the frame at the beginning of `bytes`, and the length of the whole frame.
///
/// `bytes` may hold more than one frame: the answer says where the next one starts.
pub fn decode_header(bytes: &[u8]) -> Result<(FrameHeader, usize), FrameError> {
    let body = body_len(bytes)?;
    let total = 4 + body;
    if bytes.len() < total {
        return Err(FrameError::Truncated);
    }
    let kind = FrameKind::from_u8(bytes[4]).ok_or(FrameError::UnknownKind)?;
    if bytes[5] != 0 || bytes[6] != 0 || bytes[7] != 0 {
        return Err(FrameError::Malformed);
    }
    let session_id = u64::from_le_bytes(bytes[8..16].try_into().expect("8 bytes"));
    let sequence = u64::from_le_bytes(bytes[16..24].try_into().expect("8 bytes"));
    let payload_len = u32::from_le_bytes(bytes[24..28].try_into().expect("4 bytes"));
    if payload_len as usize > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    if BODY_HEADER_BYTES + payload_len as usize != body {
        // the length prefix and the header disagree: the peer is broken
        return Err(FrameError::Malformed);
    }
    Ok((
        FrameHeader {
            kind,
            session_id,
            sequence,
            payload_len,
        },
        total,
    ))
}

/// The frame at the beginning of `bytes`: its header and its payload.
///
/// `bytes` may hold more than one frame; the frame that is read is the first one.
pub fn decode(bytes: &[u8]) -> Result<(FrameHeader, Vec<u8>), FrameError> {
    let (header, total) = decode_header(bytes)?;
    let payload = bytes[FRAME_HEADER_BYTES..total].to_vec();
    Ok((header, payload))
}

/// The frame of a header and a payload, as the bytes that go on the wire.
///
/// The length of the payload in `header` is recomputed from `payload`: a frame never lies about its length. A
/// payload above the limit is refused, never truncated (DD §17.3: "超过立即拒绝").
pub fn encode(header: &FrameHeader, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    let payload_len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge)?;
    let body = BODY_HEADER_BYTES + payload.len();
    let body32 = u32::try_from(body).map_err(|_| FrameError::TooLarge)?;
    let mut out = Vec::with_capacity(FRAME_HEADER_BYTES + payload.len());
    out.extend_from_slice(&body32.to_le_bytes());
    out.push(header.kind.wire_value());
    out.extend_from_slice(&[0u8; 3]);
    out.extend_from_slice(&header.session_id.to_le_bytes());
    out.extend_from_slice(&header.sequence.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: FrameKind, sequence: u64) -> FrameHeader {
        FrameHeader {
            kind,
            session_id: 0x0102_0304_0506_0708,
            sequence,
            payload_len: 0,
        }
    }

    #[test]
    fn a_frame_survives_the_round_trip() {
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"tase.local.status"}"#;
        let bytes = encode(&header(FrameKind::Request, 7), payload).expect("the frame is written");
        assert_eq!(bytes.len(), FRAME_HEADER_BYTES + payload.len());
        let (head, back) = decode(&bytes).expect("the frame is read");
        assert_eq!(head.kind, FrameKind::Request);
        assert_eq!(head.session_id, 0x0102_0304_0506_0708);
        assert_eq!(head.sequence, 7);
        assert_eq!(head.payload_len as usize, payload.len());
        assert_eq!(back.as_slice(), payload);
        assert_eq!(frame_len(&head), bytes.len());
    }

    /// A frame without a payload is a frame: the ping, the pong and the close carry nothing.
    #[test]
    fn a_frame_without_a_payload_is_a_frame() {
        let bytes = encode(&header(FrameKind::Ping, 0), b"").expect("the frame is written");
        assert_eq!(bytes.len(), FRAME_HEADER_BYTES);
        let (head, back) = decode(&bytes).expect("the frame is read");
        assert_eq!(head.kind, FrameKind::Ping);
        assert_eq!(head.payload_len, 0);
        assert!(back.is_empty());
    }

    /// The payload of the limit is accepted; one byte more is refused at once, and never truncated.
    #[test]
    fn the_limit_is_the_limit() {
        let big = vec![b'x'; MAX_FRAME_BYTES];
        let bytes = encode(&header(FrameKind::Event, 1), &big).expect("the frame at the limit");
        assert_eq!(bytes.len(), MAX_FRAME_TOTAL_BYTES);
        let (head, back) = decode(&bytes).expect("the frame at the limit is read");
        assert_eq!(head.payload_len as usize, MAX_FRAME_BYTES);
        assert_eq!(back.len(), MAX_FRAME_BYTES);

        let bigger = vec![b'x'; MAX_FRAME_BYTES + 1];
        assert_eq!(
            encode(&header(FrameKind::Event, 1), &bigger),
            Err(FrameError::TooLarge)
        );
    }

    /// The limit is checked from the length prefix alone: a peer may not make the receiver reserve a gigabyte.
    #[test]
    fn a_huge_length_prefix_is_refused_before_anything_is_reserved() {
        let mut bytes = encode(&header(FrameKind::Request, 1), b"{}").expect("a frame");
        bytes[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&bytes), Err(FrameError::TooLarge));
        assert_eq!(body_len(&u32::MAX.to_le_bytes()), Err(FrameError::TooLarge));
        // a length that announces a frame shorter than its own header is broken, not short
        let mut short = bytes.clone();
        short[..4].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(body_len(&short[..4]), Err(FrameError::Malformed));
    }

    /// A frame that stops in the middle is a truncation, at every cut point: a reader must not take the rest of the
    /// stream for a new frame.
    #[test]
    fn a_half_frame_is_a_truncation() {
        let bytes = encode(&header(FrameKind::Request, 3), b"{\"a\":1}").expect("a frame");
        for cut in 0..bytes.len() {
            assert_eq!(
                decode(&bytes[..cut]),
                Err(FrameError::Truncated),
                "cut at {cut}"
            );
        }
        // the whole frame is not a truncation
        assert!(decode(&bytes).is_ok());
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let mut bytes = encode(&header(FrameKind::Request, 1), b"{}").expect("a frame");
        bytes[4] = 0;
        assert_eq!(decode(&bytes), Err(FrameError::UnknownKind));
        bytes[4] = 9;
        assert_eq!(decode(&bytes), Err(FrameError::UnknownKind));
        bytes[4] = 255;
        assert_eq!(decode(&bytes), Err(FrameError::UnknownKind));
    }

    #[test]
    fn a_reserved_byte_that_is_not_zero_is_malformed() {
        let bytes = encode(&header(FrameKind::Request, 1), b"{}").expect("a frame");
        for offset in [5usize, 6, 7] {
            let mut bad = bytes.clone();
            bad[offset] = 1;
            assert_eq!(decode(&bad), Err(FrameError::Malformed), "offset {offset}");
        }
    }

    /// The length prefix and the header must agree; a frame that disagrees with itself is broken.
    #[test]
    fn two_lengths_that_disagree_are_malformed() {
        let mut bytes = encode(&header(FrameKind::Request, 1), b"{}").expect("a frame");
        // the payload says three bytes, the prefix says two
        bytes[24..28].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(decode(&bytes), Err(FrameError::Malformed));
        // the payload says more than the limit
        bytes[24..28].copy_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_le_bytes());
        bytes[..4].copy_from_slice(&((BODY_HEADER_BYTES + MAX_FRAME_BYTES + 1) as u32).to_le_bytes());
        assert_eq!(decode(&bytes), Err(FrameError::TooLarge));
    }

    /// The layout of the header is fixed: the Hub, the agents and the tools that look at a trace must agree on it.
    #[test]
    fn the_layout_of_the_header_is_fixed() {
        let bytes = encode(
            &FrameHeader {
                kind: FrameKind::Request,
                session_id: 0x0102_0304_0506_0708,
                sequence: 0x1112_1314_1516_1718,
                payload_len: 2,
            },
            b"{}",
        )
        .expect("a frame");
        // the prefix counts the bytes that follow it: the rest of the header and the payload
        assert_eq!(&bytes[..4], &26u32.to_le_bytes());
        assert_eq!(bytes[4], 3); // the wire value of Request
        assert_eq!(&bytes[5..8], &[0, 0, 0]);
        assert_eq!(&bytes[8..16], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(&bytes[16..24], &0x1112_1314_1516_1718u64.to_le_bytes());
        assert_eq!(&bytes[24..28], &2u32.to_le_bytes());
        assert_eq!(&bytes[28..], b"{}");
    }

    /// Every kind has a name and comes back from it; the wire values are the ones of the format.
    #[test]
    fn every_kind_has_a_name_and_a_wire_value() {
        for kind in FrameKind::all() {
            assert_eq!(FrameKind::from_u8(kind.wire_value()), Some(kind), "{kind:?}");
            assert!(!kind.as_str().is_empty());
        }
        assert_eq!(FrameKind::Hello.wire_value(), 1);
        assert_eq!(FrameKind::Close.wire_value(), 8);
        assert_eq!(FrameKind::from_u8(0), None);
        assert_eq!(FrameKind::from_u8(9), None);
        assert_eq!(FrameKind::all().len(), 8);
    }

    /// Two frames may follow each other: the reader uses the length of the first to find the second.
    #[test]
    fn two_frames_in_one_buffer_are_two_frames() {
        let mut stream = encode(&header(FrameKind::Request, 1), b"{}").expect("a frame");
        let second = encode(&header(FrameKind::Response, 1), b"{\"ok\":true}").expect("a frame");
        stream.extend_from_slice(&second);
        let (head, total) = decode_header(&stream).expect("the first frame");
        assert_eq!(head.kind, FrameKind::Request);
        assert_eq!(total, FRAME_HEADER_BYTES + 2);
        let (head, _) = decode_header(&stream[total..]).expect("the second frame");
        assert_eq!(head.kind, FrameKind::Response);
    }

    /// A decoder must survive any byte soup: no panic, and an error or a frame inside the limits.
    #[test]
    fn the_decoder_survives_pseudo_random_input() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for round in 0..2000 {
            let len = (next() % 48) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if round % 2 == 0 && bytes.len() >= 4 {
                // half of the inputs start like a frame, so the deeper checks run
                let body = (next() % 64) as u32;
                bytes[..4].copy_from_slice(&body.to_le_bytes());
            }
            match decode(&bytes) {
                Ok((head, payload)) => {
                    assert!(payload.len() <= MAX_FRAME_BYTES);
                    assert_eq!(head.payload_len as usize, payload.len());
                }
                Err(e) => assert!(
                    matches!(
                        e,
                        FrameError::Truncated
                            | FrameError::Malformed
                            | FrameError::UnknownKind
                            | FrameError::TooLarge
                    ),
                    "round {round}: {e:?}"
                ),
            }
        }
    }

    /// The codes of the errors are the codes of the design, and the display of an error never repeats the data of a
    /// peer in a fixed string.
    #[test]
    fn the_codes_of_the_errors_are_stable() {
        assert_eq!(FrameError::TooLarge.as_str(), "PAYLOAD_TOO_LARGE");
        assert_eq!(FrameError::Malformed.as_str(), "MALFORMED_FRAME");
        assert_eq!(FrameError::UnknownKind.as_str(), "UNKNOWN_FRAME_KIND");
        assert_eq!(FrameError::Truncated.as_str(), "TRUNCATED_FRAME");
        assert_eq!(FrameError::Timeout.as_str(), "IPC_TIMEOUT");
        assert_eq!(FrameError::ProtocolMismatch.as_str(), "PROTOCOL_MISMATCH");
        assert!(FrameError::ProtocolMismatch.is_remote());
        assert!(!FrameError::Timeout.is_remote());
        assert_eq!(
            FrameError::TooLarge.to_string(),
            "PAYLOAD_TOO_LARGE: the payload is above the limit of 1048576 bytes"
        );
        assert!(FrameError::Io("the pipe went away".into())
            .to_string()
            .contains("the pipe went away"));
    }
}
