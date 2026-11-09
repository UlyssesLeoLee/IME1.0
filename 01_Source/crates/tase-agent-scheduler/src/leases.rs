//! The leases of the scheduler: the TTL, the reclaim of an expired lease and the fencing token that refuses
//! the late write of a holder that has been replaced (detailed design §17.5 and §17.6).
//!
//! A lease says: "this agent runs this job until this instant". A lease has a TTL, and when the TTL is over
//! the lease may be taken back by another agent.
//!
//! The **fencing token** is what makes the takeover safe. Every lease of the registry carries a token, and
//! the tokens grow strictly: a holder that has been replaced keeps its old token, and every write that comes
//! with a token that is not the current one of the job is refused. That is what keeps a worker that was slow,
//! that was paused, or that came back from the dead from overwriting the work of the worker that replaced it
//! ("旧 Worker lease epoch 比较失败必须终止，不允许过期执行者更新正式 ref", detailed design §17.6).
//!
//! The crate never reads the clock: the caller passes the instant of every operation, so the expiry of a
//! lease and the takeover of a lease are exact in a test.

use crate::Timestamp;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;
use tase_contract::{AgentId, JobId, SessionId, TaseError};

/// The fencing token of a lease.
///
/// The token of a lease is bigger than the token of every lease that came before it, in the whole registry
/// and not only for one job: a token is therefore unique, and the comparison of two tokens always tells
/// which lease is the newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FencingToken(u64);

impl FencingToken {
    /// The first token of a registry.
    pub const FIRST: FencingToken = FencingToken(1);

    /// The value of the token.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FencingToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A lease of a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The job the lease holds.
    pub job_id: JobId,
    /// The agent that holds the lease.
    pub agent_id: AgentId,
    /// The session of the agent, when it has one.
    pub session_id: Option<SessionId>,
    /// The fencing token of the lease.
    pub token: FencingToken,
    /// The instant the lease was issued.
    pub issued_at: Timestamp,
    /// The instant the lease is over: the lease is valid while the clock is before it.
    pub expires_at: Timestamp,
}

impl Lease {
    /// Whether the lease is still valid at `now`.
    pub fn is_valid_at(&self, now: Timestamp) -> bool {
        now < self.expires_at
    }
}

/// The verdict of the fence: may this holder write?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceCheck {
    /// The lease is the current one of the job and it is not over: the write may go on.
    Valid,
    /// The lease is over: the holder must not write.
    Expired,
    /// Another lease has replaced this one: the holder is a zombie and must not write.
    StaleToken,
    /// The job has no lease at all.
    NoLease,
}

impl FenceCheck {
    /// Whether the write may go on.
    pub fn is_valid(self) -> bool {
        matches!(self, FenceCheck::Valid)
    }

    /// The failure of the contract layer for a write that must not go on.
    pub fn to_error(self) -> Option<TaseError> {
        match self {
            FenceCheck::Valid => None,
            FenceCheck::Expired => Some(TaseError::worker_unavailable(
                "the lease of the job is over: the write is refused",
            )),
            FenceCheck::StaleToken => Some(TaseError::permission_denied(
                "the lease of the job was replaced: the late write is refused",
            )),
            FenceCheck::NoLease => Some(TaseError::not_found("the job has no lease any more")),
        }
    }
}

/// The registry of the leases of the Hub.
///
/// The registry holds the current lease of every job and the counter of the fencing tokens. The token of a
/// lease is taken when the lease is issued and it is never given back, so the token of a new lease is always
/// bigger than the token of the lease it replaces.
#[derive(Clone, Debug, Default)]
pub struct LeaseRegistry {
    leases: BTreeMap<JobId, Lease>,
    next_token: u64,
}

impl LeaseRegistry {
    /// A registry without a lease.
    pub fn new() -> Self {
        LeaseRegistry::default()
    }

    /// Takes a lease of a job for an agent, until `now + ttl`.
    ///
    /// A job that is already leased and whose lease is not over cannot be leased again. A job whose lease is
    /// over may be taken: the new lease carries a bigger token, and the holder of the old lease may not write
    /// any more.
    pub fn acquire(
        &mut self,
        job_id: JobId,
        agent_id: AgentId,
        session_id: Option<SessionId>,
        ttl: Duration,
        now: Timestamp,
    ) -> Result<Lease, TaseError> {
        if ttl.is_zero() {
            return Err(TaseError::invalid_argument(
                "the TTL of a lease must not be zero",
            ));
        }
        if let Some(current) = self.leases.get(&job_id)
            && current.is_valid_at(now)
        {
            return Err(TaseError::worker_unavailable(format!(
                "the job {} is already leased until {}",
                job_id, current.expires_at
            )));
        }
        self.next_token = self.next_token.saturating_add(1);
        let lease = Lease {
            job_id: job_id.clone(),
            agent_id,
            session_id,
            token: FencingToken(self.next_token),
            issued_at: now,
            expires_at: now.saturating_add(ttl),
        };
        self.leases.insert(job_id, lease.clone());
        Ok(lease)
    }

    /// The current lease of a job.
    pub fn current(&self, job_id: &JobId) -> Option<&Lease> {
        self.leases.get(job_id)
    }

    /// The number of the leases of the registry.
    pub fn len(&self) -> usize {
        self.leases.len()
    }

    /// Whether the registry holds no lease.
    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    /// The verdict of the fence for a holder that wants to write at `now`.
    ///
    /// The token is compared before the TTL: a holder that was replaced is a zombie even when its own lease
    /// is over, and the reason of the refusal of a zombie is the one that matters.
    pub fn check(&self, lease: &Lease, now: Timestamp) -> FenceCheck {
        match self.leases.get(&lease.job_id) {
            None => FenceCheck::NoLease,
            Some(current) if current.token != lease.token => FenceCheck::StaleToken,
            Some(current) if !current.is_valid_at(now) => FenceCheck::Expired,
            Some(_) => FenceCheck::Valid,
        }
    }

    /// Whether the holder of a lease may write at `now`.
    pub fn may_write(&self, lease: &Lease, now: Timestamp) -> bool {
        self.check(lease, now).is_valid()
    }

    /// Renews a lease: the same token, a new deadline.
    ///
    /// Only the holder of the current lease may renew it, and only while the lease is not over.
    pub fn renew(
        &mut self,
        lease: &Lease,
        ttl: Duration,
        now: Timestamp,
    ) -> Result<Lease, TaseError> {
        if ttl.is_zero() {
            return Err(TaseError::invalid_argument(
                "the TTL of a lease must not be zero",
            ));
        }
        match self.check(lease, now) {
            FenceCheck::Valid => {}
            other => {
                return Err(other
                    .to_error()
                    .unwrap_or_else(|| TaseError::internal("the lease is not valid")));
            }
        }
        let current = self
            .leases
            .get_mut(&lease.job_id)
            .ok_or_else(|| TaseError::not_found("the job has no lease any more"))?;
        current.expires_at = now.saturating_add(ttl);
        Ok(current.clone())
    }

    /// Gives a lease back; only the holder of the current lease may do it.
    pub fn release(&mut self, lease: &Lease, now: Timestamp) -> Result<(), TaseError> {
        match self.check(lease, now) {
            FenceCheck::Valid => {}
            other => {
                return Err(other
                    .to_error()
                    .unwrap_or_else(|| TaseError::internal("the lease is not valid")));
            }
        }
        self.leases.remove(&lease.job_id);
        Ok(())
    }

    /// The leases that are over at `now`.
    pub fn expired(&self, now: Timestamp) -> Vec<Lease> {
        self.leases
            .values()
            .filter(|lease| !lease.is_valid_at(now))
            .cloned()
            .collect()
    }

    /// Takes back the leases that are over at `now` and gives them back to the caller.
    ///
    /// The reclaim does not give a new token: the next lease of the job takes one, and it is bigger than the
    /// token of the lease that was reclaimed.
    pub fn expire(&mut self, now: Timestamp) -> Vec<Lease> {
        let expired = self.expired(now);
        for lease in &expired {
            self.leases.remove(&lease.job_id);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tase_contract::ErrorCode;

    /// An agent of the tests.
    fn agent(name: &str) -> AgentId {
        AgentId::parse(name).expect("a legal agent")
    }

    /// A job of the tests.
    fn job(name: &str) -> JobId {
        JobId::parse(name).expect("a legal job")
    }

    #[test]
    fn a_lease_carries_its_ttl() {
        let mut registry = LeaseRegistry::new();
        let start = Timestamp::from_millis(1_000);
        let lease = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(30),
                start,
            )
            .expect("the lease is issued");

        assert_eq!(lease.issued_at, start);
        assert_eq!(lease.expires_at, Timestamp::from_millis(31_000));
        assert_eq!(lease.agent_id, agent("agent_01"));
        assert!(lease.is_valid_at(start));
        assert!(lease.is_valid_at(Timestamp::from_millis(30_999)));
        assert!(!lease.is_valid_at(lease.expires_at));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_lease_of_zero_ttl_is_refused() {
        let mut registry = LeaseRegistry::new();
        let error = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::ZERO,
                Timestamp::zero(),
            )
            .expect_err("a lease of zero is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_fencing_token_grows_with_every_lease() {
        let mut registry = LeaseRegistry::new();
        let first = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        assert_eq!(first.token, FencingToken::FIRST);

        let second = registry
            .acquire(
                job("job_02"),
                agent("agent_02"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        assert!(
            second.token > first.token,
            "the token of a new lease is always bigger"
        );

        // The first lease is over: the job may be taken again, with a bigger token.
        let later = Timestamp::from_millis(10_000);
        let third = registry
            .acquire(
                job("job_01"),
                agent("agent_03"),
                None,
                Duration::from_secs(10),
                later,
            )
            .expect("the job may be taken again");
        assert!(third.token > second.token);
    }

    #[test]
    fn the_late_write_of_a_replaced_holder_is_refused() {
        let mut registry = LeaseRegistry::new();
        let start = Timestamp::from_millis(0);
        let old = registry
            .acquire(
                job("job_01"),
                agent("agent_old"),
                None,
                Duration::from_secs(10),
                start,
            )
            .expect("the lease is issued");
        assert_eq!(registry.check(&old, start), FenceCheck::Valid);

        // The lease is over and another agent takes the job.
        let later = Timestamp::from_millis(10_000);
        let new = registry
            .acquire(
                job("job_01"),
                agent("agent_new"),
                None,
                Duration::from_secs(10),
                later,
            )
            .expect("the job may be taken again");
        assert!(new.token > old.token);

        // The old holder comes back and writes: the write is refused, it is a zombie.
        assert_eq!(registry.check(&old, later), FenceCheck::StaleToken);
        assert!(!registry.may_write(&old, later));
        let error = registry
            .check(&old, later)
            .to_error()
            .expect("a refused write is a failure");
        assert_eq!(error.code, ErrorCode::PermissionDenied);

        // The new holder may write.
        assert_eq!(registry.check(&new, later), FenceCheck::Valid);
        assert!(registry.may_write(&new, later));
    }

    #[test]
    fn the_write_of_an_expired_holder_is_refused() {
        let mut registry = LeaseRegistry::new();
        let lease = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");

        let later = Timestamp::from_millis(10_000);
        assert_eq!(registry.check(&lease, later), FenceCheck::Expired);
        assert!(!registry.may_write(&lease, later));
        let error = registry
            .check(&lease, later)
            .to_error()
            .expect("a refused write is a failure");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
    }

    #[test]
    fn an_expired_lease_can_be_reclaimed() {
        let mut registry = LeaseRegistry::new();
        let lease = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");

        let later = Timestamp::from_millis(10_000);
        assert_eq!(registry.expired(later).len(), 1);
        let reclaimed = registry.expire(later);
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].token, lease.token);
        assert!(registry.is_empty());
        assert_eq!(registry.current(&job("job_01")), None);
        assert_eq!(registry.check(&lease, later), FenceCheck::NoLease);
    }

    #[test]
    fn a_lease_that_is_not_over_is_not_reclaimed() {
        let mut registry = LeaseRegistry::new();
        registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        let later = Timestamp::from_millis(9_999);
        assert!(registry.expired(later).is_empty());
        assert!(registry.expire(later).is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_job_that_is_already_leased_cannot_be_leased_again() {
        let mut registry = LeaseRegistry::new();
        registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        let error = registry
            .acquire(
                job("job_01"),
                agent("agent_02"),
                None,
                Duration::from_secs(10),
                Timestamp::from_millis(1),
            )
            .expect_err("the job is already leased");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
        assert!(error.message.contains("job_01"));
    }

    #[test]
    fn a_lease_can_be_renewed_by_its_holder() {
        let mut registry = LeaseRegistry::new();
        let start = Timestamp::from_millis(0);
        let lease = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                start,
            )
            .expect("the lease is issued");

        let renewed = registry
            .renew(&lease, Duration::from_secs(10), Timestamp::from_millis(5_000))
            .expect("the lease is renewed");
        assert_eq!(renewed.token, lease.token, "a renewal keeps the token");
        assert_eq!(renewed.expires_at, Timestamp::from_millis(15_000));
        assert!(registry.may_write(&renewed, Timestamp::from_millis(14_999)));
    }

    #[test]
    fn an_expired_lease_cannot_be_renewed() {
        let mut registry = LeaseRegistry::new();
        let lease = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        let error = registry
            .renew(
                &lease,
                Duration::from_secs(10),
                Timestamp::from_millis(10_000),
            )
            .expect_err("the lease is over");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
    }

    #[test]
    fn a_replaced_holder_cannot_renew_or_release_the_lease() {
        let mut registry = LeaseRegistry::new();
        let old = registry
            .acquire(
                job("job_01"),
                agent("agent_old"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        let later = Timestamp::from_millis(10_000);
        let _new = registry
            .acquire(
                job("job_01"),
                agent("agent_new"),
                None,
                Duration::from_secs(10),
                later,
            )
            .expect("the job may be taken again");

        assert_eq!(
            registry.renew(&old, Duration::from_secs(10), later).unwrap_err().code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            registry.release(&old, later).unwrap_err().code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(registry.len(), 1, "the new lease is still there");
    }

    #[test]
    fn a_lease_can_be_given_back() {
        let mut registry = LeaseRegistry::new();
        let lease = registry
            .acquire(
                job("job_01"),
                agent("agent_01"),
                None,
                Duration::from_secs(10),
                Timestamp::zero(),
            )
            .expect("the lease is issued");
        registry
            .release(&lease, Timestamp::from_millis(1))
            .expect("the lease is given back");
        assert!(registry.is_empty());
    }
}
