//! Length-prefixed frames (DD 2.1: "长度前缀帧 + UTF-8 JSON").
//!
//! ```text
//! offset 0  2 bytes  magic "PI"       anything else is garbage (e.g. a JSON Lines client) and closes the connection
//! offset 2  1 byte   class            0 = control (limit 64 KiB), 1 = bulk (limit 2 MiB, snapshot text); others are malformed
//! offset 3  1 byte   reserved         must be 0
//! offset 4  4 bytes  payload length   u32 little endian, 1 ..= the limit of the class; 0 is malformed
//! offset 8  n bytes  payload          UTF-8 JSON text
//! ```
//!
//! The header is validated completely before a single payload byte is buffered, so a peer cannot make the receiver
//! allocate more than the limit of its connection state (4 KiB before authentication, 64 KiB / 2 MiB after).

use crate::error::IpcError;
use std::time::Instant;

pub const HEADER_LEN: usize = 8;
pub const MAGIC: [u8; 2] = *b"PI";
/// DD 2.1 / 15.2: control messages.
pub const CONTROL_FRAME_MAX: usize = 64 * 1024;
/// DD 2.1 / 15.2: messages that carry the authorized full text of a snapshot.
pub const BULK_FRAME_MAX: usize = 2 * 1024 * 1024;
/// Handshake frames are tiny; nothing bigger is read from a peer that has not authenticated yet.
pub const HANDSHAKE_FRAME_MAX: usize = 4 * 1024;
/// No configuration can raise a limit above this (the 16 MiB line of the stdio protocol 0.3, `pime_core::MAX_BULK_LINE_BYTES`).
pub const HARD_MAX_FRAME: usize = pime_core::MAX_BULK_LINE_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameClass {
    Control = 0,
    Bulk = 1,
}

impl FrameClass {
    fn from_byte(b: u8) -> Option<FrameClass> {
        match b {
            0 => Some(FrameClass::Control),
            1 => Some(FrameClass::Bulk),
            _ => None,
        }
    }
}

/// Per-class payload limits of one connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    control: usize,
    bulk: usize,
}

impl Limits {
    /// The limits of an authenticated connection (DD 2.1).
    pub const DEFAULT: Limits = Limits {
        control: CONTROL_FRAME_MAX,
        bulk: BULK_FRAME_MAX,
    };
    /// The limits before authentication: only control frames of at most 4 KiB.
    pub const HANDSHAKE: Limits = Limits {
        control: HANDSHAKE_FRAME_MAX,
        bulk: 0,
    };

    /// Custom limits; both must be at least 1 and at most [`HARD_MAX_FRAME`], and bulk must not be below control.
    pub fn new(control: usize, bulk: usize) -> Result<Limits, IpcError> {
        if control == 0 || bulk < control || bulk > HARD_MAX_FRAME {
            return Err(IpcError::InvalidArgument(
                "frame limits: need 1 <= control <= bulk <= the hard maximum",
            ));
        }
        Ok(Limits { control, bulk })
    }

    pub fn max_for(&self, class: FrameClass) -> usize {
        match class {
            FrameClass::Control => self.control,
            FrameClass::Bulk => self.bulk,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub class: FrameClass,
    pub len: usize,
}

/// A received frame. `text` is valid UTF-8 and a well-formed JSON object (checked by the connection before delivery).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub class: FrameClass,
    pub text: String,
}

/// `now + timeout` without the panic of `Instant + Duration` on overflow: a caller-supplied `Duration::MAX` means "very long", not a crash.
pub fn deadline_after(timeout: std::time::Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + std::time::Duration::from_secs(365 * 24 * 3600))
}

/// A byte stream with deadlines: implemented by the named pipe and by the in-memory pair of the tests.
pub trait Transport {
    /// Reads at least one and at most `buf.len()` bytes. End of stream is `Err(Closed)`, never `Ok(0)`.
    fn read_some(&mut self, buf: &mut [u8], deadline: Instant) -> Result<usize, IpcError>;
    /// Writes all of `data` or fails; a timeout may leave a partial write, so the stream must be dropped afterwards.
    fn write_all(&mut self, data: &[u8], deadline: Instant) -> Result<(), IpcError>;
}

pub fn encode_header(class: FrameClass, len: usize) -> Result<[u8; HEADER_LEN], IpcError> {
    let len32 = u32::try_from(len).map_err(|_| IpcError::PayloadTooLarge {
        len,
        max: HARD_MAX_FRAME,
    })?;
    let mut h = [0u8; HEADER_LEN];
    h[..2].copy_from_slice(&MAGIC);
    h[2] = class as u8;
    h[3] = 0;
    h[4..].copy_from_slice(&len32.to_le_bytes());
    Ok(h)
}

/// Validates a header against the limits of the connection state. Order: magic, class, reserved, zero length, size.
pub fn decode_header(bytes: &[u8; HEADER_LEN], limits: &Limits) -> Result<Header, IpcError> {
    if bytes[..2] != MAGIC {
        return Err(IpcError::Malformed("bad frame magic"));
    }
    let class =
        FrameClass::from_byte(bytes[2]).ok_or(IpcError::Malformed("unknown frame class"))?;
    if bytes[3] != 0 {
        return Err(IpcError::Malformed("reserved frame byte is not zero"));
    }
    let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if len == 0 {
        return Err(IpcError::Malformed("zero-length frame"));
    }
    let max = limits.max_for(class);
    if len > max {
        return Err(IpcError::PayloadTooLarge { len, max });
    }
    Ok(Header { class, len })
}

/// Header + payload as one buffer (one write call: frames of concurrent writers can never interleave).
pub fn encode(class: FrameClass, payload: &[u8], limits: &Limits) -> Result<Vec<u8>, IpcError> {
    let max = limits.max_for(class);
    if payload.is_empty() {
        return Err(IpcError::Malformed("zero-length frame"));
    }
    if payload.len() > max {
        // never truncate silently (DD 15.2): the caller must shrink the request or chunk it
        return Err(IpcError::PayloadTooLarge {
            len: payload.len(),
            max,
        });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&encode_header(class, payload.len())?);
    out.extend_from_slice(payload);
    Ok(out)
}

pub fn write_frame<T: Transport + ?Sized>(
    t: &mut T,
    class: FrameClass,
    payload: &[u8],
    limits: &Limits,
    deadline: Instant,
) -> Result<(), IpcError> {
    let buf = encode(class, payload, limits)?;
    t.write_all(&buf, deadline)
}

/// Reads exactly `buf.len()` bytes. `*consumed` is set as soon as one byte of the frame has arrived, so the caller
/// can tell an idle timeout (nothing read, the stream is still in sync) from a timeout in the middle of a frame.
fn read_exact<T: Transport + ?Sized>(
    t: &mut T,
    buf: &mut [u8],
    deadline: Instant,
    consumed: &mut bool,
    first: bool,
) -> Result<(), IpcError> {
    let mut got = 0;
    while got < buf.len() {
        match t.read_some(&mut buf[got..], deadline) {
            Ok(0) => return Err(IpcError::Malformed("transport returned no data")),
            Ok(n) => {
                got += n;
                *consumed = true;
            }
            // the peer left: between frames that is an orderly end, inside a frame it is a truncation
            Err(IpcError::Closed) if first && !*consumed => return Err(IpcError::Closed),
            Err(IpcError::Closed) => return Err(IpcError::Truncated),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Reads one frame within `deadline`. Returns the raw payload; UTF-8/JSON validation is the connection's job.
/// `consumed` reports whether any byte of this frame was read (see [`read_exact`]), also on error.
pub fn read_frame<T: Transport + ?Sized>(
    t: &mut T,
    limits: &Limits,
    deadline: Instant,
    consumed: &mut bool,
) -> Result<(FrameClass, Vec<u8>), IpcError> {
    *consumed = false;
    let mut hdr = [0u8; HEADER_LEN];
    read_exact(t, &mut hdr, deadline, consumed, true)?;
    let header = decode_header(&hdr, limits)?;
    // `len` is bounded by `limits` here, so this allocation is bounded too
    let mut payload = vec![0u8; header.len];
    read_exact(t, &mut payload, deadline, consumed, false)?;
    Ok((header.class, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Script, far};

    fn frame_bytes(class: FrameClass, payload: &[u8]) -> Vec<u8> {
        encode(class, payload, &Limits::DEFAULT).unwrap()
    }

    fn read(bytes: &[u8], limits: &Limits) -> Result<(FrameClass, Vec<u8>), IpcError> {
        let mut t = Script::new(bytes);
        let mut c = false;
        read_frame(&mut t, limits, far(), &mut c)
    }

    #[test]
    fn ut_ipc_frame_round_trip_control_and_bulk() {
        for class in [FrameClass::Control, FrameClass::Bulk] {
            let bytes = frame_bytes(class, br#"{"a":1}"#);
            assert_eq!(&bytes[..2], b"PI");
            assert_eq!(bytes.len(), HEADER_LEN + 7);
            let (c, p) = read(&bytes, &Limits::DEFAULT).unwrap();
            assert_eq!((c, p.as_slice()), (class, br#"{"a":1}"#.as_slice()));
        }
    }

    #[test]
    fn ut_ipc_frame_header_layout_is_fixed() {
        let h = encode_header(FrameClass::Bulk, 0x0102_0304).unwrap();
        assert_eq!(h, [b'P', b'I', 1, 0, 0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn ut_ipc_frame_zero_length_is_malformed_both_ways() {
        assert_eq!(
            encode(FrameClass::Control, b"", &Limits::DEFAULT),
            Err(IpcError::Malformed("zero-length frame"))
        );
        let hdr = [b'P', b'I', 0, 0, 0, 0, 0, 0];
        assert_eq!(
            read(&hdr, &Limits::DEFAULT),
            Err(IpcError::Malformed("zero-length frame"))
        );
    }

    #[test]
    fn ut_ipc_frame_oversized_is_rejected_from_the_header_alone() {
        // 64 KiB + 1 as a control frame; only the header is supplied: the check must not wait for the payload
        let mut h = encode_header(FrameClass::Control, 1).unwrap();
        h[4..].copy_from_slice(&((CONTROL_FRAME_MAX as u32) + 1).to_le_bytes());
        assert_eq!(
            read(&h, &Limits::DEFAULT),
            Err(IpcError::PayloadTooLarge {
                len: CONTROL_FRAME_MAX + 1,
                max: CONTROL_FRAME_MAX
            })
        );
        // the same length is fine as a bulk frame up to 2 MiB, and not above
        let mut hb = h;
        hb[2] = 1;
        assert!(decode_header(&hb, &Limits::DEFAULT).is_ok());
        hb[4..].copy_from_slice(&((BULK_FRAME_MAX as u32) + 1).to_le_bytes());
        assert!(matches!(
            decode_header(&hb, &Limits::DEFAULT),
            Err(IpcError::PayloadTooLarge {
                max: BULK_FRAME_MAX,
                ..
            })
        ));
        // u32::MAX must not wrap or allocate
        hb[4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decode_header(&hb, &Limits::DEFAULT),
            Err(IpcError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn ut_ipc_frame_sender_never_truncates() {
        let big = vec![b'x'; CONTROL_FRAME_MAX + 1];
        assert_eq!(
            encode(FrameClass::Control, &big, &Limits::DEFAULT),
            Err(IpcError::PayloadTooLarge {
                len: CONTROL_FRAME_MAX + 1,
                max: CONTROL_FRAME_MAX
            })
        );
        assert!(encode(FrameClass::Bulk, &big, &Limits::DEFAULT).is_ok());
    }

    #[test]
    fn ut_ipc_frame_handshake_limits_refuse_bulk_and_big_control() {
        let bulk = frame_bytes(FrameClass::Bulk, b"{}");
        assert!(matches!(
            read(&bulk, &Limits::HANDSHAKE),
            Err(IpcError::PayloadTooLarge { max: 0, .. })
        ));
        let big = frame_bytes(FrameClass::Control, &vec![b' '; HANDSHAKE_FRAME_MAX + 1]);
        assert!(matches!(
            read(&big, &Limits::HANDSHAKE),
            Err(IpcError::PayloadTooLarge {
                max: HANDSHAKE_FRAME_MAX,
                ..
            })
        ));
    }

    #[test]
    fn ut_ipc_frame_bad_magic_class_and_reserved_are_malformed() {
        let ok = frame_bytes(FrameClass::Control, b"{}");
        let mut bad = ok.clone();
        bad[0] = b'{'; // a JSON Lines client talking to the pipe
        assert_eq!(
            read(&bad, &Limits::DEFAULT),
            Err(IpcError::Malformed("bad frame magic"))
        );
        let mut bad = ok.clone();
        bad[2] = 2;
        assert_eq!(
            read(&bad, &Limits::DEFAULT),
            Err(IpcError::Malformed("unknown frame class"))
        );
        let mut bad = ok;
        bad[3] = 1;
        assert_eq!(
            read(&bad, &Limits::DEFAULT),
            Err(IpcError::Malformed("reserved frame byte is not zero"))
        );
    }

    #[test]
    fn ut_ipc_frame_truncated_at_every_cut_point() {
        let bytes = frame_bytes(FrameClass::Control, br#"{"op":"health"}"#);
        // empty stream: orderly close between frames
        assert_eq!(read(&[], &Limits::DEFAULT), Err(IpcError::Closed));
        // every other cut (inside the header or the payload) is a truncation, never a short frame
        for cut in 1..bytes.len() {
            assert_eq!(
                read(&bytes[..cut], &Limits::DEFAULT),
                Err(IpcError::Truncated),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn ut_ipc_frame_reads_across_tiny_chunks_and_back_to_back_frames() {
        let mut stream = frame_bytes(FrameClass::Control, br#"{"n":1}"#);
        stream.extend(frame_bytes(FrameClass::Bulk, br#"{"n":2}"#));
        let mut t = Script::new(&stream).chunked(3);
        let mut c = false;
        let a = read_frame(&mut t, &Limits::DEFAULT, far(), &mut c).unwrap();
        let b = read_frame(&mut t, &Limits::DEFAULT, far(), &mut c).unwrap();
        assert_eq!(a.1, br#"{"n":1}"#);
        assert_eq!(
            (b.0, b.1.as_slice()),
            (FrameClass::Bulk, br#"{"n":2}"#.as_slice())
        );
        assert_eq!(
            read_frame(&mut t, &Limits::DEFAULT, far(), &mut c),
            Err(IpcError::Closed)
        );
    }

    #[test]
    fn ut_ipc_frame_consumed_flag_separates_idle_timeouts_from_partial_ones() {
        let bytes = frame_bytes(FrameClass::Control, b"{}");
        let mut c = true;
        let mut idle = Script::new(&[]).then_timeout();
        assert_eq!(
            read_frame(&mut idle, &Limits::DEFAULT, far(), &mut c),
            Err(IpcError::Timeout)
        );
        assert!(!c, "nothing was read: the stream is still in sync");
        let mut partial = Script::new(&bytes[..3]).then_timeout();
        assert_eq!(
            read_frame(&mut partial, &Limits::DEFAULT, far(), &mut c),
            Err(IpcError::Timeout)
        );
        assert!(c, "a timeout inside a frame desynchronises the stream");
    }

    #[test]
    fn ut_ipc_limits_constructor_enforces_the_hard_maximum() {
        assert!(Limits::new(1, 1).is_ok());
        assert!(Limits::new(0, 10).is_err());
        assert!(Limits::new(100, 10).is_err());
        assert!(Limits::new(100, HARD_MAX_FRAME).is_ok());
        assert!(Limits::new(100, HARD_MAX_FRAME + 1).is_err());
    }

    /// A decoder must survive any byte soup: no panic, and an error or a frame inside the limits.
    #[test]
    fn ut_ipc_frame_decoder_survives_pseudo_random_input() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            // xorshift64*: deterministic, so a failure is reproducible
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for round in 0..3000 {
            let len = (next() % 40) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if round % 2 == 0 && bytes.len() >= HEADER_LEN {
                // make half of the inputs start like a header so the deeper checks run
                bytes[..2].copy_from_slice(b"PI");
                bytes[2] &= 1;
                bytes[3] = 0;
                bytes[6] = 0;
                bytes[7] = 0;
            }
            match read(&bytes, &Limits::DEFAULT) {
                Ok((class, payload)) => {
                    assert!(!payload.is_empty() && payload.len() <= Limits::DEFAULT.max_for(class))
                }
                Err(e) => assert!(
                    matches!(
                        e,
                        IpcError::Closed
                            | IpcError::Truncated
                            | IpcError::Malformed(_)
                            | IpcError::PayloadTooLarge { .. }
                    ),
                    "round {round}: {e:?}"
                ),
            }
        }
    }
}
