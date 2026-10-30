//! `PIME-IPC/1`: the authenticated named-pipe transport between the TSF text service, Core, the AI Worker, the UI and
//! the editor bridges (RUST_MIGRATION_PLAN P3, DD 2.1). See `docs/IPC.md` for the format, the threat model and what
//! is and is not verified.
//!
//! Layers, from the bottom:
//! * [`frame`]: 8-byte header + UTF-8 JSON payload, hard size limits checked before any allocation (pure).
//! * [`auth`]: keys, HMAC transcripts, constant-time comparison, nonce replay window, pipe name and SDDL (pure).
//! * [`handshake`]: version negotiation, mutual challenge-response, capability token (generic over a transport).
//! * [`connection`]: framed, deadline-bound messages with fail-closed error handling (generic over a transport).
//! * `winsec`, `pipe`, `endpoint` (Windows only): the process token SID, the ACL, overlapped pipe I/O, the
//!   [`endpoint::Listener`] and [`endpoint::connect`].
//!
//! `unsafe` is denied crate-wide and allowed in exactly two modules, `winsec` and `pipe`, so the compiler enforces
//! that the Win32 FFI boundary stays there. `pime-core` stays free of `unsafe` and of Windows dependencies; this
//! crate is outside the main workspace.
#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

pub mod auth;
pub mod connection;
pub mod error;
pub mod frame;
pub mod handshake;

#[cfg(windows)]
pub mod endpoint;
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod pipe;
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod winsec;

#[cfg(test)]
pub(crate) mod testutil;

pub use auth::{PipeName, Secret};
pub use connection::{Connection, Timeouts};
pub use error::IpcError;
pub use frame::{Frame, FrameClass, Limits};
pub use handshake::{ClientCredentials, RolePolicy, ServerPolicy, ServerState, SessionInfo};
