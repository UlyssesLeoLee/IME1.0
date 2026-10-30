//! The two ends of `PIME-IPC/1` on a real named pipe: [`Listener`] (server) and [`connect`] (client).
//!
//! Server: the first pipe instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE` and an ACL that names only the
//! current user, and with `PIPE_REJECT_REMOTE_CLIENTS`. A listening instance always exists (the next one is created
//! before a connected one is handed out), so the name is never free for a squatter while the server runs.
//! A connected client is first checked to be in the server's own session, then authenticated by the handshake; only
//! then does the caller see a [`Connection`].

use crate::auth::PipeName;
use crate::connection::{Connection, Timeouts};
use crate::error::IpcError;
use crate::frame::{Limits, deadline_after};
use crate::handshake::{
    ClientCredentials, ServerPolicy, ServerState, client_handshake, server_handshake,
};
use crate::pipe::{Pipe, StopHandle};
use crate::winsec::{
    SecurityDescriptor, SystemEntropy, current_session_id, current_user_sid_string,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type PipeConnection = Connection<Pipe>;

pub struct ServerConfig {
    pub policy: ServerPolicy,
    pub timeouts: Timeouts,
    pub limits: Limits,
    /// Upper bound of simultaneous pipe instances (`nMaxInstances`).
    pub max_instances: u32,
    /// Refuse clients of another terminal-services session (DD 2.1). Default true.
    pub require_same_session: bool,
}

impl ServerConfig {
    pub fn new(policy: ServerPolicy) -> ServerConfig {
        ServerConfig {
            policy,
            timeouts: Timeouts::default(),
            limits: Limits::DEFAULT,
            max_instances: 32,
            require_same_session: true,
        }
    }
}

struct Shared {
    state: ServerState,
    timeouts: Timeouts,
    limits: Limits,
    own_session: Option<u32>,
}

pub struct Listener {
    name: PipeName,
    shared: Arc<Shared>,
    sd: SecurityDescriptor,
    max_instances: u32,
    next: Option<Pipe>,
    stop: StopHandle,
}

impl Listener {
    /// Binds `\\.\pipe\PIME-IPC-1-<current user SID>`.
    pub fn bind_current_user(cfg: ServerConfig) -> Result<Listener, IpcError> {
        let name = PipeName::for_sid(&current_user_sid_string()?)?;
        Listener::bind(name, cfg)
    }

    /// Creates the first instance of `name`. `Err(NameInUse)` if any process already owns the name (squatting).
    pub fn bind(name: PipeName, cfg: ServerConfig) -> Result<Listener, IpcError> {
        let sd = SecurityDescriptor::for_current_user()?;
        let first = Pipe::create_server_instance(&name, true, &sd, cfg.max_instances)?;
        let own_session = if cfg.require_same_session {
            Some(current_session_id()?)
        } else {
            None
        };
        Ok(Listener {
            name,
            shared: Arc::new(Shared {
                state: ServerState::new(cfg.policy),
                timeouts: cfg.timeouts,
                limits: cfg.limits,
                own_session,
            }),
            sd,
            max_instances: cfg.max_instances,
            next: Some(first),
            stop: StopHandle::new()?,
        })
    }

    pub fn name(&self) -> &PipeName {
        &self.name
    }

    /// The keys a client of `role` must present (the installer's job in production; tests use it directly).
    pub fn role_key(&self, role: &str) -> crate::auth::Secret {
        self.shared.state.policy.role_key(role)
    }

    /// A handle that makes `accept`/`serve` return `Err(Stopped)`/`Ok(())` from any thread.
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    /// The DACL of the listening instance as SDDL, for the ACL verification test and diagnostics.
    pub fn dacl_sddl(&self) -> Result<String, IpcError> {
        let pipe = self.next.as_ref().ok_or(IpcError::Broken)?;
        crate::winsec::dacl_sddl_of_handle(pipe.raw())
    }

    fn fresh_instance(&self) -> Option<Pipe> {
        Pipe::create_server_instance(&self.name, false, &self.sd, self.max_instances).ok()
    }

    /// Waits up to `timeout` for a client and returns it *unauthenticated*; call [`PendingConnection::authenticate`]
    /// (possibly on another thread, so a slow client cannot hold up the accept loop).
    pub fn accept_pending(&mut self, timeout: Duration) -> Result<PendingConnection, IpcError> {
        let mut listening = match self.next.take() {
            Some(p) => p,
            // an earlier replacement could not be created (instance limit): try again now
            None => Pipe::create_server_instance(&self.name, false, &self.sd, self.max_instances)?,
        };
        match listening.wait_for_client(deadline_after(timeout), Some(&self.stop)) {
            Ok(()) => {}
            Err(e @ (IpcError::Timeout | IpcError::Stopped)) => {
                // nothing happened: the instance keeps listening
                self.next = Some(listening);
                return Err(e);
            }
            Err(e) => {
                // the client vanished before we could talk to it: replace the instance, never leave the name without one
                self.next = self.fresh_instance();
                return Err(e);
            }
        }
        // the connected instance leaves; its replacement is created first so the listening set is never empty
        self.next = self.fresh_instance();
        let connected = listening.with_stop(self.stop.clone());
        if let Some(own) = self.shared.own_session {
            match connected.client_session_id() {
                Ok(s) if s == own => {}
                // other session, or unknown: fail closed (the dropped handle closes the connection)
                _ => return Err(IpcError::WrongSession),
            }
        }
        Ok(PendingConnection {
            pipe: connected,
            shared: Arc::clone(&self.shared),
        })
    }

    /// `accept_pending` + the handshake on this thread. Fine for a single-client loop; use `serve` or hand the pending
    /// connection to a worker thread when several clients may connect.
    pub fn accept(&mut self, timeout: Duration) -> Result<PipeConnection, IpcError> {
        self.accept_pending(timeout)?.authenticate()
    }

    /// Runs an accept loop until the stop handle is signalled: one thread per authenticated connection, at most
    /// `max_connections` at a time (further clients are dropped at once), a panic in `handler` ends only that
    /// connection. Returns after all connection threads have finished.
    ///
    /// Shutdown does not depend on the handler: signalling the stop handle makes every pending and later read/write of
    /// every connection of this listener fail with `Stopped`, so a handler that returns on errors other than an idle
    /// `Timeout` ends at once. (A handler that ignores errors and loops would keep `serve` waiting; that is a bug in the handler.)
    pub fn serve<H>(&mut self, max_connections: usize, handler: H) -> Result<(), IpcError>
    where
        H: Fn(PipeConnection) + Send + Sync + 'static,
    {
        let handler = Arc::new(handler);
        let active = Arc::new(AtomicUsize::new(0));
        let mut workers: Vec<JoinHandle<()>> = Vec::new();
        let result = loop {
            workers.retain(|w| !w.is_finished());
            if self.stop.is_signalled() {
                break Ok(());
            }
            match self.accept_pending(Duration::from_millis(200)) {
                Ok(pending) => {
                    if active.load(Ordering::SeqCst) >= max_connections {
                        continue; // dropped: the client sees a closed pipe
                    }
                    let slot = ActiveSlot::take(&active);
                    let handler = Arc::clone(&handler);
                    let spawned =
                        thread::Builder::new()
                            .name("pime-ipc-conn".into())
                            .spawn(move || {
                                let _slot = slot; // released on every exit path, including a panic in `authenticate`
                                if let Ok(conn) = pending.authenticate() {
                                    // a panicking handler must not take the process (or the listener) down with it
                                    let _ = catch_unwind(AssertUnwindSafe(|| handler(conn)));
                                }
                            });
                    // if no thread could be created the closure was dropped with the connection and the slot: the client sees a closed pipe
                    if let Ok(worker) = spawned {
                        workers.push(worker);
                    }
                }
                Err(IpcError::Timeout) => {}
                Err(IpcError::Stopped) => break Ok(()),
                // a client of another session, or one that left early: not a reason to stop serving
                Err(IpcError::WrongSession) | Err(IpcError::Closed) => {}
                Err(_) => {
                    // an unexpected OS error (for example the instance limit): back off so a persistent failure
                    // cannot spin, then try again; the stop check at the top of the loop still ends the server
                    thread::sleep(Duration::from_millis(50));
                }
            }
        };
        for w in workers {
            let _ = w.join();
        }
        result
    }
}

/// One of the `max_connections` slots of `serve`: counted while alive, released by `Drop` on every path.
struct ActiveSlot(Arc<AtomicUsize>);

impl ActiveSlot {
    fn take(counter: &Arc<AtomicUsize>) -> ActiveSlot {
        counter.fetch_add(1, Ordering::SeqCst);
        ActiveSlot(Arc::clone(counter))
    }
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A connected, not yet authenticated client.
pub struct PendingConnection {
    pipe: Pipe,
    shared: Arc<Shared>,
}

impl PendingConnection {
    /// Runs the handshake under the handshake deadline. Any failure drops (closes) the connection.
    pub fn authenticate(mut self) -> Result<PipeConnection, IpcError> {
        let shared = Arc::clone(&self.shared);
        let deadline = deadline_after(shared.timeouts.handshake);
        let info = server_handshake(&mut self.pipe, &shared.state, &mut SystemEntropy, deadline)?;
        Ok(Connection::new(
            self.pipe,
            info,
            shared.limits,
            shared.timeouts,
        ))
    }
}

pub struct ClientConfig {
    pub timeouts: Timeouts,
    pub limits: Limits,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            timeouts: Timeouts::default(),
            limits: Limits::DEFAULT,
        }
    }
}

/// Connects to the pipe and runs the client side of the handshake. `Err(Unavailable)` at once if no server exists.
pub fn connect(
    name: &PipeName,
    creds: &ClientCredentials,
    cfg: &ClientConfig,
) -> Result<PipeConnection, IpcError> {
    let mut pipe = Pipe::connect_client(name, deadline_after(cfg.timeouts.connect))?;
    let deadline = deadline_after(cfg.timeouts.handshake);
    let info = client_handshake(&mut pipe, creds, &mut SystemEntropy, deadline)?;
    Ok(Connection::new(pipe, info, cfg.limits, cfg.timeouts))
}

/// A unique pipe name for tests and diagnostics: `PIME-IPC-1-TEST-<random hex>`; never a fixed name.
pub fn unique_test_pipe_name() -> Result<PipeName, IpcError> {
    let suffix = crate::auth::random_hex(&mut SystemEntropy, 8)?;
    PipeName::for_test(&suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_004_connection_slot_is_released_even_when_the_worker_panics() {
        let counter = Arc::new(AtomicUsize::new(0));
        let slot = ActiveSlot::take(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        let worker = thread::spawn(move || {
            let _slot = slot;
            panic!("a worker panics before its handler runs");
        });
        assert!(worker.join().is_err());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "a leaked slot would shrink the server by one connection for good"
        );
    }
}
