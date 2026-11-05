//! TASE-LOCAL-IPC: the transport layer of the TASE Local Agent Hub.
//!
//! The Hub is the one process that owns the state of the local agents (DD §17.1). This crate is the layer
//! between the Hub and the agents that run on the same machine: how a frame is written, how two peers agree on a
//! protocol, how a request is paired with its answer, and who may connect at all.
//!
//! | module | question |
//! |---|---|
//! | [`framing`] | what does a frame look like on the wire? |
//! | [`transport`] | how do the bytes travel, and when does the sender give up? |
//! | [`connection`] | how do two peers greet each other, and how is an answer paired with its request? |
//! | [`acl`] | which logon session is this, and who may talk to it? |
//!
//! **The frame format of TASE is the one of this crate.** The repository already has `pime-ipc`, an IPC crate
//! with an authenticated named pipe, a per-user DACL and length-prefixed frames of its own (`PIME-IPC/1`). The
//! two overlap, and the mapping of the design books settles the overlap (DD/BD mapping §7-G4): the pipe, its
//! security and its deadline-bound I/O are **reused** from `pime-ipc`, and the frame format, the protocol
//! negotiation, the session model and the access control of TASE are **written here**, because `pime-ipc` has
//! none of them:
//!
//! * `pime-ipc` speaks `PIME-IPC/1` (an 8-byte header with the magic `"PI"`); the Hub speaks TASE (a 4-byte
//!   little-endian length prefix, as DD §17.3 asks), and a client of one may live on the same machine as a
//!   client of the other;
//! * `pime-ipc` binds its pipe to the *user* SID; the Hub binds its pipe to the *logon* SID, so that two
//!   sessions of the same user never meet (DD §17.2, BD §16.2);
//! * `pime-ipc` has no capability negotiation and no session lease: the Hub grants capabilities and assigns the
//!   session that every frame carries.
//!
//! The crate is pure logic plus one thin adapter to the Windows pipe: no third-party dependency (the strict
//! JSON parser of `pime-core` is the only one), no `unsafe`, and it compiles on every platform - the named pipe
//! is the part that is Windows only.

#![forbid(unsafe_code)]

pub mod acl;
pub mod connection;
pub mod framing;
pub mod transport;

pub use acl::{AclPolicy, PeerIdentity, deny_reason, may_connect, pipe_name};
pub use connection::{
    Connection, Event, Hello, HelloAck, PROTOCOL_MAJOR, PROTOCOL_VERSION, Timeouts, version_is_compatible,
};
pub use framing::{
    FrameError, FrameHeader, FrameKind, FRAME_HEADER_BYTES, MAX_FRAME_BYTES, decode, encode,
};
pub use transport::{MemoryPipe, Transport, WindowsPipe};

/// The version of the crate.
pub const VERSION: &str = "0.1.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crate_offers_the_frame_the_transport_the_connection_and_the_acl() {
        // the frame of TASE is the one of this crate
        let header = FrameHeader {
            kind: FrameKind::Request,
            session_id: 1,
            sequence: 2,
            payload_len: 0,
        };
        let bytes = encode(&header, b"{}").expect("the frame is written");
        let (back, payload) = decode(&bytes).expect("the frame is read");
        assert_eq!(back.kind, FrameKind::Request);
        assert_eq!(back.sequence, 2);
        assert_eq!(payload, b"{}");

        // the transport, the connection and the acl are there
        let (mut a, mut b) = MemoryPipe::pair();
        assert_eq!(a.send(b"x").expect("the bytes are written"), 1);
        let mut buf = [0u8; 4];
        assert_eq!(b.recv(&mut buf).expect("the bytes are read"), 1);
        assert_eq!(VERSION, "0.1.0");
        assert_eq!(PROTOCOL_MAJOR, 1);
        assert!(version_is_compatible(PROTOCOL_VERSION));
    }
}
