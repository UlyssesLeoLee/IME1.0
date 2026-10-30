//! Programming IME AI worker (plan phase P2): the only process that sees an API key.
//!
//! The worker speaks JSON Lines over stdio (framing and conventions of `pime-core`'s protocol 0.3), sends HTTPS requests
//! through WinHTTP (SChannel, the system trust store, the system proxy) and keeps API keys in Windows Credential Manager.
//! A key is stored once (`cred.set`) and from then on only ever injected into an outgoing request header by this process:
//! there is deliberately no operation that returns it. See `docs/AI_WORKER.md` for the protocol and the security
//! properties, and which of them are verified.
//!
//! Layout: everything except `winhttp` and `wincred` is safe, pure Rust and is unit-tested with a fake transport and an
//! in-memory vault; `secret` contains the volatile wipe (one `unsafe` block per primitive) and the two FFI modules
//! contain all the remaining `unsafe`, each block with its safety argument.
#![deny(unsafe_code)]

pub mod config;
pub mod engine;
pub mod errmap;
pub mod error;
pub mod framing;
pub mod policy;
pub mod redact;
pub mod secret;
pub mod transport;
pub mod vault;
pub mod wincred;
pub mod winhttp;
