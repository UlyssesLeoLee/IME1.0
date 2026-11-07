//! TASE-LOCAL-HUB: the resident Local Agent Hub of TASE.
//!
//! The Hub is the one process of a logon session that owns the state of the local agents (detailed
//! design §17.1). It is the piece that the design books describe in §17 of the detailed design and in
//! §16 of the basic design: one process per login session, a named pipe as its namespace, a rule that
//! only the processes of that very session may talk to it, a session per agent, an arbitration of the
//! writes into the workspaces, and a fair queue for the jobs.
//!
//! | module | question |
//! |---|---|
//! | [`bootstrap`] | how does the process start, and how is it alone in its namespace? |
//! | [`auth`] | who may talk to the Hub? |
//! | [`session`] | what is a session of an agent? |
//! | [`workspace`] | who may write into which file? |
//! | [`server`] | what does the Hub do with the frames it receives? |
//! | [`telemetry`] | what did the Hub see? |
//!
//! The crate is built on the crates that are already there and are not rewritten here:
//!
//! * `tase-local-ipc` owns the frame format, the protocol negotiation, the connection and the access
//!   control of the pipe. The Hub speaks it; it does not write a second transport;
//! * `tase-local-store` owns the persistence (the sessions, the jobs, the leases, the idempotency
//!   records and the generation of the files);
//! * `tase-contract` owns the identifiers and the twelve business error codes;
//! * `pime-core` owns the strict JSON parser and the SHA-256.
//!
//! Three rules the crate keeps:
//!
//! * **the Hub never reads the clock.** Every timestamp comes from the caller (`now_ms`) or from a clock
//!   that the caller injected ([`Clock`]), so a test is deterministic and a recovery can be replayed;
//! * **the Hub writes no second truth.** The state of the sessions, of the jobs and of the generations
//!   lives in `tase-local-store`; the Hub keeps the in-memory view of it and asks the store when the two
//!   may differ;
//! * **the Hub writes no secret into its events.** The telemetry is redacted ([`telemetry::redact`]).
//!
//! The Windows part (the token of the process, the named pipe) is behind `#[cfg(windows)]`; the crate
//! compiles on every platform, and the parts that are Windows only say so when they are called.

#![cfg_attr(not(windows), forbid(unsafe_code))]

pub mod auth;
pub mod bootstrap;
pub mod server;
pub mod session;
pub mod telemetry;
pub mod workspace;

pub use auth::authorize;
pub use bootstrap::{
    BoxedTransport, HubConfig, HubHandle, HubRole, PipeFactory, PipeListener, PlatformPipeFactory,
    PlatformSessionIdentity, SessionIdentity, StaticSessionIdentity, platform_factory,
    platform_identity, start_or_connect, start_or_connect_with,
};
pub use server::{Hub, HubState, JobRequest, JobScheduler, RecoveryReport, RoundRobinScheduler};
pub use session::{AgentSession, SessionEntry, SessionRegistry, SessionState};
pub use telemetry::{HubEvent, HubEventKind, Telemetry};
pub use workspace::{Clock, FileLock, ManualClock, SystemClock, WorkspaceRegistry};

/// The version of the crate.
pub const VERSION: &str = "0.1.0";

/// The capabilities the Hub offers to the agents (detailed design §17.3).
///
/// A capability is granted, never assumed: the Hub grants the intersection of what the agent asks for
/// and of this list.
pub const HUB_CAPABILITIES: [&str; 8] = [
    "catalog:read",
    "plan:write",
    "preview:read",
    "code:write",
    "workspace:read",
    "workspace:write",
    "job:write",
    "events:read",
];

/// The intervals the Hub uses when nobody says otherwise (detailed design §17.3).
///
/// The design fixes the numbers of the budgets of §17.5 (64 connections, 16 preparations, 2 compiles,
/// 1 commit window) and not the deadlines of a session; these are the defaults of a local pipe.
pub mod defaults {
    /// The most connections at the same time (detailed design §17.5).
    pub const MAX_CONNECTIONS: u32 = 64;
    /// The most jobs that are not finished at the same time (detailed design §17.5).
    pub const MAX_CONCURRENT_JOBS: u32 = 16;
    /// How long a session lease lasts when the agent does not say otherwise.
    pub const SESSION_LEASE_MS: u64 = 30_000;
    /// How long a write lock lasts when the agent does not say otherwise.
    pub const FILE_LEASE_MS: u64 = 60_000;
    /// How often the Hub expects a heartbeat.
    pub const HEARTBEAT_INTERVAL_MS: u64 = 5_000;
}
