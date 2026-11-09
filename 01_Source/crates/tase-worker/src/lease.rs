//! The lease of a worker: the deadline, the fencing token and the guard that refuses the write of an old holder
//! (detailed design §6.2 and §5.5).
//!
//! # The rule of the lease
//!
//! Detailed design §6.2 says it in one sentence: the lease of a job has a deadline, the epoch of the lease grows with
//! every claim, and a worker whose lease ran out may not move the job any more. This module is the small, in-process
//! form of that rule:
//!
//! * a lease holds a resource for a while (`ttl_ms`);
//! * every lease of a resource carries a **fencing token** that is greater than the token of every lease that came
//!   before it;
//! * a write is only accepted when it carries the token of the **current** lease of the resource **and** when that
//!   lease is still alive;
//! * a lease that ran out may be taken back, and the worker that comes back late is told that it is old.
//!
//! The last point is the one that keeps two workers from writing at the same time (the split brain of detailed design
//! §5.5): the old holder does not know that it is old, but every write of the resource goes through the table, and the
//! table refuses it.
//!
//! # The time
//!
//! Every method of this module takes the time of the moment as an argument, and the clock of the worker
//! ([`Clock`]) is the only thing that knows what time it is. A test moves a [`LogicalClock`] by hand and never waits;
//! the [`SystemClock`] exists for the binary and is the only place of the crate that reads the wall clock.

use std::fmt;

use tase_contract::{ErrorCode, TaseError};

/// The source of the time of the worker.
///
/// Every deadline of the worker - the TTL of a lease, the window of a backoff, the moment a lease runs out - comes from
/// this trait and never from the wall clock: a test moves the clock by hand and the worker behaves the same on every
/// machine.
pub trait Clock {
    /// The time of the worker, in milliseconds since the epoch of the Unix time.
    fn now_ms(&self) -> u64;
}

/// A clock that is moved by hand.
///
/// The clock of the tests and of the replays: it starts at a moment and only moves when somebody moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogicalClock {
    now_ms: u64,
}

impl LogicalClock {
    /// A clock at a moment.
    pub fn new(start_ms: u64) -> LogicalClock {
        LogicalClock { now_ms: start_ms }
    }

    /// The time of the clock.
    pub fn now(&self) -> u64 {
        self.now_ms
    }

    /// Moves the clock forward and gives the new time back.
    pub fn advance(&mut self, delta_ms: u64) -> u64 {
        self.now_ms = self.now_ms.saturating_add(delta_ms);
        self.now_ms
    }

    /// Moves the clock to a moment.
    pub fn set(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }
}

impl Clock for LogicalClock {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
}

/// The clock of the machine.
///
/// This is the only place of the crate that reads the wall clock, and only the binary of the worker uses it: the core
/// of the worker always takes the time as an argument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(elapsed) => u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            Err(_) => 0,
        }
    }
}

/// The state of a lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// The holder has the lease.
    Held,
    /// The holder gave the lease back.
    Released,
    /// The lease ran out of time.
    Expired,
}

impl LeaseState {
    /// The name of the state.
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseState::Held => "HELD",
            LeaseState::Released => "RELEASED",
            LeaseState::Expired => "EXPIRED",
        }
    }
}

/// A lease of a resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The resource that is leased.
    pub resource: String,
    /// The holder of the lease.
    pub holder: String,
    /// The fencing token of the lease.
    ///
    /// The token is greater than the token of every lease of the same resource that came before it. A write that
    /// carries an older token is the write of a worker that came back too late.
    pub fencing_token: u64,
    /// The moment the lease was taken.
    pub acquired_at_ms: u64,
    /// The moment the lease runs out.
    pub expires_at_ms: u64,
    /// The state of the lease.
    pub state: LeaseState,
}

impl Lease {
    /// Whether the lease is still the one of its holder at the given moment.
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        self.state == LeaseState::Held && self.expires_at_ms > now_ms
    }

    /// Whether the lease ran out of time at the given moment.
    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        self.state == LeaseState::Held && self.expires_at_ms <= now_ms
    }
}

/// Why a lease operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseError {
    /// The resource is held by somebody else.
    Held {
        /// The resource that is leased.
        resource: String,
        /// The holder of the lease.
        holder: String,
        /// The moment the lease runs out.
        expires_at_ms: u64,
    },
    /// The lease ran out of time.
    Expired {
        /// The resource that was leased.
        resource: String,
        /// The moment the lease ran out.
        expires_at_ms: u64,
    },
    /// The token is not the token of the current lease: the writer is old.
    ///
    /// This is the split brain of detailed design §5.5: the worker that comes back late is told that it is old, and
    /// its write is refused.
    StaleToken {
        /// The resource that was written.
        resource: String,
        /// The token the writer carried.
        presented: u64,
        /// The token of the current lease.
        current: u64,
    },
    /// The caller is not the holder of the lease.
    NotHolder {
        /// The resource that was leased.
        resource: String,
        /// The caller that asked.
        holder: String,
    },
    /// There is no lease of the resource.
    Unknown {
        /// The resource that was asked for.
        resource: String,
    },
}

impl LeaseError {
    /// The business code of the refusal.
    pub fn code(&self) -> ErrorCode {
        match self {
            LeaseError::Held { .. } => ErrorCode::RateLimited,
            LeaseError::Expired { .. } => ErrorCode::WorkerUnavailable,
            LeaseError::StaleToken { .. } | LeaseError::NotHolder { .. } => {
                ErrorCode::PermissionDenied
            }
            LeaseError::Unknown { .. } => ErrorCode::NotFound,
        }
    }

    /// The refusal as the error of the contract layer.
    pub fn to_error(&self) -> TaseError {
        TaseError::new(self.code(), self.to_string())
    }
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseError::Held {
                resource,
                holder,
                expires_at_ms,
            } => write!(
                f,
                "the resource {resource} is leased by {holder} until {expires_at_ms}"
            ),
            LeaseError::Expired {
                resource,
                expires_at_ms,
            } => write!(f, "the lease of {resource} ran out at {expires_at_ms}"),
            LeaseError::StaleToken {
                resource,
                presented,
                current,
            } => write!(
                f,
                "the token {presented} of {resource} is old; the current token is {current}"
            ),
            LeaseError::NotHolder { resource, holder } => {
                write!(f, "the lease of {resource} is not held by {holder}")
            }
            LeaseError::Unknown { resource } => write!(f, "there is no lease of {resource}"),
        }
    }
}

impl std::error::Error for LeaseError {}

/// The leases of one worker process.
///
/// The table is the in-process form of the table `leases` of the store of the local hub: it holds the leases of the
/// resources the worker works on, it hands them out one at a time and it refuses the write of a holder that is old.
#[derive(Clone, Debug, Default)]
pub struct LeaseTable {
    leases: Vec<Lease>,
    next_token: u64,
}

impl LeaseTable {
    /// A table without a lease.
    pub fn new() -> LeaseTable {
        LeaseTable {
            leases: Vec::new(),
            next_token: 0,
        }
    }

    /// Takes a lease of a resource.
    ///
    /// A resource that is held by a live lease is refused; a resource whose lease ran out is taken back, and the new
    /// lease carries a token that is greater than the token of the lease it replaces.
    pub fn acquire(
        &mut self,
        resource: &str,
        holder: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<Lease, LeaseError> {
        if let Some(current) = self.lease_of(resource)
            && current.is_valid_at(now_ms)
        {
            return Err(LeaseError::Held {
                resource: resource.to_string(),
                holder: current.holder.clone(),
                expires_at_ms: current.expires_at_ms,
            });
        }

        // The token of the new lease is greater than the token of every lease of the resource that came before it, so
        // that the holder that comes back late is told that it is old (detailed design §6.2).
        let previous = self
            .lease_of(resource)
            .map_or(0, |lease| lease.fencing_token);
        let fencing_token = self.next_token.max(previous).saturating_add(1);
        self.next_token = fencing_token;

        let lease = Lease {
            resource: resource.to_string(),
            holder: holder.to_string(),
            fencing_token,
            acquired_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            state: LeaseState::Held,
        };
        self.leases.retain(|held| held.resource != resource);
        self.leases.push(lease.clone());
        Ok(lease)
    }

    /// Renews the lease of a resource.
    ///
    /// A lease that ran out of time is not renewed: the holder has to take a new one, and the new one carries a
    /// greater token.
    pub fn renew(
        &mut self,
        resource: &str,
        holder: &str,
        fencing_token: u64,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<Lease, LeaseError> {
        let lease = self
            .lease_mut(resource)
            .ok_or_else(|| LeaseError::Unknown {
                resource: resource.to_string(),
            })?;
        if lease.fencing_token != fencing_token {
            return Err(LeaseError::StaleToken {
                resource: resource.to_string(),
                presented: fencing_token,
                current: lease.fencing_token,
            });
        }
        if lease.holder != holder {
            return Err(LeaseError::NotHolder {
                resource: resource.to_string(),
                holder: holder.to_string(),
            });
        }
        if !lease.is_valid_at(now_ms) {
            return Err(LeaseError::Expired {
                resource: resource.to_string(),
                expires_at_ms: lease.expires_at_ms,
            });
        }
        lease.expires_at_ms = now_ms.saturating_add(ttl_ms);
        Ok(lease.clone())
    }

    /// Gives the lease of a resource back.
    ///
    /// A lease that is not held any more is accepted again and keeps its state: the call changes nothing.
    pub fn release(
        &mut self,
        resource: &str,
        holder: &str,
        fencing_token: u64,
    ) -> Result<Lease, LeaseError> {
        let lease = self
            .lease_mut(resource)
            .ok_or_else(|| LeaseError::Unknown {
                resource: resource.to_string(),
            })?;
        if lease.fencing_token != fencing_token {
            return Err(LeaseError::StaleToken {
                resource: resource.to_string(),
                presented: fencing_token,
                current: lease.fencing_token,
            });
        }
        if lease.holder != holder {
            return Err(LeaseError::NotHolder {
                resource: resource.to_string(),
                holder: holder.to_string(),
            });
        }
        if lease.state == LeaseState::Held {
            lease.state = LeaseState::Released;
        }
        Ok(lease.clone())
    }

    /// Takes the leases that ran out of time back, and gives them back.
    pub fn reclaim_expired(&mut self, now_ms: u64) -> Vec<Lease> {
        let mut taken = Vec::new();
        for lease in &mut self.leases {
            if lease.is_expired_at(now_ms) {
                lease.state = LeaseState::Expired;
                taken.push(lease.clone());
            }
        }
        taken
    }

    /// The lease of a resource.
    pub fn lease_of(&self, resource: &str) -> Option<&Lease> {
        self.leases.iter().find(|lease| lease.resource == resource)
    }

    /// Every lease of the table.
    pub fn leases(&self) -> &[Lease] {
        &self.leases
    }

    /// Checks that a holder may still write.
    ///
    /// The write is accepted only when the token is the token of the current lease of the resource and when that lease
    /// is still alive. A worker whose lease ran out is refused here, and so is a worker that comes back after another
    /// worker took the lease back.
    pub fn check(&self, resource: &str, fencing_token: u64, now_ms: u64) -> Result<(), LeaseError> {
        let lease = self.lease_of(resource).ok_or_else(|| LeaseError::Unknown {
            resource: resource.to_string(),
        })?;
        if lease.fencing_token != fencing_token {
            return Err(LeaseError::StaleToken {
                resource: resource.to_string(),
                presented: fencing_token,
                current: lease.fencing_token,
            });
        }
        if !lease.is_valid_at(now_ms) {
            return Err(LeaseError::Expired {
                resource: resource.to_string(),
                expires_at_ms: lease.expires_at_ms,
            });
        }
        Ok(())
    }

    /// The lease of a resource, to change it.
    fn lease_mut(&mut self, resource: &str) -> Option<&mut Lease> {
        self.leases
            .iter_mut()
            .find(|lease| lease.resource == resource)
    }
}

/// The check that tells a writer whether it still owns the resource.
pub trait FenceGuard {
    /// Checks that the caller may still write.
    fn check(&self, now_ms: u64) -> Result<(), LeaseError>;
}

/// The guard of a holder: the table, the resource and the token of the holder.
#[derive(Clone, Debug)]
pub struct LeaseGuard<'a> {
    table: &'a LeaseTable,
    resource: String,
    fencing_token: u64,
}

impl<'a> LeaseGuard<'a> {
    /// The guard of a holder of a lease.
    pub fn new(table: &'a LeaseTable, resource: &str, fencing_token: u64) -> LeaseGuard<'a> {
        LeaseGuard {
            table,
            resource: resource.to_string(),
            fencing_token,
        }
    }

    /// The token of the guard.
    pub fn fencing_token(&self) -> u64 {
        self.fencing_token
    }
}

impl FenceGuard for LeaseGuard<'_> {
    fn check(&self, now_ms: u64) -> Result<(), LeaseError> {
        self.table.check(&self.resource, self.fencing_token, now_ms)
    }
}

/// The guard of a worker that has no lease to defend.
///
/// It is the guard of the parts of the worker that write nothing that another worker may write at the same time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unguarded;

impl FenceGuard for Unguarded {
    fn check(&self, _now_ms: u64) -> Result<(), LeaseError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resource the tests of the module lease.
    const RESOURCE: &str = "job_000000000001";

    fn table() -> LeaseTable {
        LeaseTable::new()
    }

    #[test]
    fn the_first_lease_of_a_resource_carries_the_first_token() {
        let mut table = table();
        let lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        assert_eq!(lease.fencing_token, 1);
        assert_eq!(lease.holder, "worker-1");
        assert_eq!(lease.expires_at_ms, 31_000);
        assert_eq!(lease.state, LeaseState::Held);
    }

    #[test]
    fn the_token_grows_with_every_lease_of_the_resource() {
        let mut table = table();
        let first = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        table
            .release(RESOURCE, "worker-1", first.fencing_token)
            .expect("the lease is given back");
        let second = table
            .acquire(RESOURCE, "worker-2", 30_000, 2_000)
            .expect("the lease is taken again");
        assert!(second.fencing_token > first.fencing_token);
        let third = table
            .acquire(RESOURCE, "worker-3", 30_000, 3_000)
            .expect_err("the lease is held");
        assert!(matches!(third, LeaseError::Held { .. }));
    }

    #[test]
    fn the_write_of_an_old_holder_is_refused() {
        let mut table = table();
        let first = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        table
            .release(RESOURCE, "worker-1", first.fencing_token)
            .expect("the lease is given back");
        let second = table
            .acquire(RESOURCE, "worker-2", 30_000, 2_000)
            .expect("the lease is taken again");

        // the old holder comes back late and writes with the token it had
        let refused = table
            .check(RESOURCE, first.fencing_token, 2_500)
            .expect_err("the write of the old holder is refused");
        assert!(matches!(
            refused,
            LeaseError::StaleToken { presented, current, .. } if presented == first.fencing_token && current == second.fencing_token
        ));
        // the new holder may write
        assert!(table.check(RESOURCE, second.fencing_token, 2_500).is_ok());
    }

    #[test]
    fn the_guard_of_an_old_holder_is_refused() {
        let mut table = table();
        let first = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let token = first.fencing_token;

        {
            let guard = LeaseGuard::new(&table, RESOURCE, token);
            assert!(guard.check(1_500).is_ok());
            assert_eq!(guard.fencing_token(), token);
        }

        table
            .release(RESOURCE, "worker-1", token)
            .expect("the lease is given back");
        let _second = table
            .acquire(RESOURCE, "worker-2", 30_000, 2_000)
            .expect("the lease is taken again");

        // the guard of the old holder is refused: the token it carries is not the token of the lease any more
        let guard = LeaseGuard::new(&table, RESOURCE, token);
        assert!(matches!(
            guard.check(2_500),
            Err(LeaseError::StaleToken { .. })
        ));
    }

    #[test]
    fn a_lease_that_ran_out_of_time_is_taken_back() {
        let mut table = table();
        let first = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        assert!(table.check(RESOURCE, first.fencing_token, 30_999).is_ok());

        let taken = table.reclaim_expired(31_000);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].state, LeaseState::Expired);
        assert_eq!(taken[0].fencing_token, first.fencing_token);

        // the resource may be leased again, with a greater token
        let second = table
            .acquire(RESOURCE, "worker-2", 30_000, 31_000)
            .expect("the lease is taken again");
        assert!(second.fencing_token > first.fencing_token);
    }

    #[test]
    fn the_write_of_a_holder_whose_lease_ran_out_is_refused() {
        let mut table = table();
        let lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let refused = table
            .check(RESOURCE, lease.fencing_token, 31_000)
            .expect_err("the write of a holder whose lease ran out is refused");
        assert!(matches!(refused, LeaseError::Expired { .. }));
        assert_eq!(
            refused.code(),
            ErrorCode::WorkerUnavailable,
            "an expired lease is an infrastructure failure"
        );
    }

    #[test]
    fn a_lease_that_is_held_is_not_taken_twice() {
        let mut table = table();
        let _lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let refused = table
            .acquire(RESOURCE, "worker-2", 30_000, 2_000)
            .expect_err("the lease is held");
        assert!(matches!(refused, LeaseError::Held { .. }));
        assert_eq!(refused.code(), ErrorCode::RateLimited);
    }

    #[test]
    fn a_lease_is_renewed_by_its_holder() {
        let mut table = table();
        let lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let renewed = table
            .renew(RESOURCE, "worker-1", lease.fencing_token, 60_000, 10_000)
            .expect("the lease is renewed");
        assert_eq!(renewed.expires_at_ms, 70_000);
        assert_eq!(renewed.fencing_token, lease.fencing_token);
    }

    #[test]
    fn a_lease_that_ran_out_is_not_renewed() {
        let mut table = table();
        let lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let refused = table.renew(RESOURCE, "worker-1", lease.fencing_token, 60_000, 31_000);
        assert!(matches!(refused, Err(LeaseError::Expired { .. })));
    }

    #[test]
    fn a_lease_is_not_renewed_by_somebody_else() {
        let mut table = table();
        let lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let refused = table.renew(RESOURCE, "worker-2", lease.fencing_token, 60_000, 2_000);
        assert!(matches!(refused, Err(LeaseError::NotHolder { .. })));
    }

    #[test]
    fn a_lease_that_is_not_held_any_more_may_be_given_back_again() {
        let mut table = table();
        let lease = table
            .acquire(RESOURCE, "worker-1", 30_000, 1_000)
            .expect("the lease is taken");
        let first = table
            .release(RESOURCE, "worker-1", lease.fencing_token)
            .expect("the lease is given back");
        let second = table
            .release(RESOURCE, "worker-1", lease.fencing_token)
            .expect("the lease is given back again");
        assert_eq!(first.state, LeaseState::Released);
        assert_eq!(second.state, LeaseState::Released);
    }

    #[test]
    fn a_resource_without_a_lease_is_unknown() {
        let table = table();
        let refused = table
            .check("job_000000000009", 1, 1_000)
            .expect_err("there is no lease of the resource");
        assert!(matches!(refused, LeaseError::Unknown { .. }));
        assert_eq!(refused.code(), ErrorCode::NotFound);
    }

    #[test]
    fn the_clock_of_the_worker_is_moved_by_hand() {
        let mut clock = LogicalClock::new(1_000);
        assert_eq!(clock.now_ms(), 1_000);
        assert_eq!(clock.advance(500), 1_500);
        assert_eq!(clock.now(), 1_500);
        clock.set(42);
        assert_eq!(clock.now_ms(), 42);
    }

    #[test]
    fn the_clock_of_the_machine_is_a_clock_of_the_worker() {
        let clock = SystemClock;
        assert!(clock.now_ms() > 0);
    }
}
