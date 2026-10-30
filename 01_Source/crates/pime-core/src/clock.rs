//! Injectable wall clock (milliseconds since the Unix epoch, the same unit as JavaScript `Date.now()`).
//!
//! Every time-dependent rule (plan and token lifetimes, grant expiry, the apply deadline) takes the current time
//! as an argument; the protocol layer reads it from a [`Clock`], so tests drive time explicitly and never sleep.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send {
    fn now_ms(&self) -> u64;
}

/// The real clock. Grant expiry times cross the engine boundary (they are persisted by the JS bridge and shown to
/// the user), so this must be wall-clock epoch time, not a monotonic counter.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A clock that only moves when told to. Clones share the same time, so a test can keep one handle and give
/// another to the core.
#[derive(Clone, Debug, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    pub fn new(start_ms: u64) -> Self {
        ManualClock(Arc::new(AtomicU64::new(start_ms)))
    }

    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_epoch_milliseconds() {
        let now = SystemClock.now_ms();
        // after 2024-01-01 and before the year 2100 (catches seconds-vs-milliseconds mistakes)
        assert!(now > 1_704_067_200_000 && now < 4_102_444_800_000);
    }

    #[test]
    fn manual_clock_clones_share_time() {
        let a = ManualClock::new(1000);
        let b = a.clone();
        a.advance(500);
        assert_eq!(b.now_ms(), 1500);
        b.set(7);
        assert_eq!(a.now_ms(), 7);
    }
}
