//! Programming IME core v0.2 (zero third-party dependencies).
//!
//! Pure logic only: no process execution, no file or network access, no `unsafe`.
//! The JavaScript engine in `editor-bridges/vscode/src/core` is the behavioural reference; the golden fixtures in
//! `tests/fixtures` (generated from it) are checked by `tests/parity.rs`.
#![forbid(unsafe_code)]

pub mod analysis;
pub mod candidate;
pub mod error;
pub mod input;
pub mod install_plan;
pub mod json;
pub mod pack;
pub mod predict;
pub mod profiles;
pub mod protocol;
pub mod resolve;
pub mod scope;
pub mod secrets;
pub mod textrange;
pub mod util;
pub mod validators;
pub mod x86;

pub const VERSION: &str = "0.2.0";
pub const PROTOCOL: &str = "0.2";
/// Maximum accepted size of one JSON Lines request.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
