//! Programming IME core v0.2 with JSON Lines protocol 0.3 (zero third-party dependencies).
//!
//! Pure logic only: no process execution, no file or network access, no `unsafe`. (The one exception is
//! `settings_io`, the atomic write of the settings file: a library facility that no protocol operation can reach.)
//! The JavaScript engine in `editor-bridges/vscode/src/core` is the behavioural reference; the golden fixtures in
//! `tests/fixtures` (generated from it) are checked by `tests/parity.rs`.
#![forbid(unsafe_code)]

pub mod analysis;
pub mod budget;
pub mod candidate;
pub mod clock;
pub mod context;
pub mod edit;
pub mod error;
pub mod grants;
pub mod ids;
pub mod input;
pub mod install_plan;
pub mod json;
pub mod ops;
pub mod pack;
pub mod predict;
pub mod profiles;
pub mod protocol;
pub mod resolve;
pub mod scope;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod sha256;
pub mod textrange;
pub mod util;
pub mod validators;
pub mod x86;

// Steps 1.2 / 1.3: AI request logic, annotations, symbols, realtime scheduling
pub mod ai;
pub mod annotate;
pub mod ops_ai;
mod regex;
pub mod scheduler;
pub mod symbols;

// Step 1.3 (P1B): settings migration, diagnostics, interface texts, UI models
pub mod diag;
pub mod i18n;
mod i18n_table;
pub mod jsval;
pub mod ops_cfg;
pub mod settings_io;
pub mod settings_migrate;
pub mod uimodel;

pub const VERSION: &str = "0.2.0";
/// Protocol 0.3 is a superset of 0.2: every 0.2 operation is unchanged, the session operations are added.
pub const PROTOCOL: &str = "0.3";
/// Maximum accepted size of one JSON Lines request of a stateless 0.2 operation (and of every 0.3 operation that
/// does not carry document text).
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// Maximum accepted size of one request line of an operation that carries document text. A 2 MiB document can
/// grow to about 12 MiB when every character needs a `\u00XX` escape.
pub const MAX_BULK_LINE_BYTES: usize = 16 * 1024 * 1024;
