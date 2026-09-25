use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::time::Instant;

use super::{
    breaker::{Breaker, Signal},
    budget::RetryBudget,
    config::GovernorPolicy,
    config::GroupConfig,
    jitter::Random,
    pool::PermitPool,
    rate::TokenBucket,
};

/// Cumulative per-group event counts for the Limits page and logs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub requests: u64,
    pub retries: u64,
    pub successes: u64,
    pub transient_failures: u64,
    pub timeouts: u64,
    pub backend_unavailable: u64,
    pub admission_refused: u64,
    /// `try_acquire` found no free permit.
    pub shed_busy: u64,
    /// A queued wait ran out.
    pub shed_timeout: u64,
    /// Turned away by an open breaker or an in-flight probe.
    pub shed_unavailable: u64,
    /// The rate ceiling needed a longer wait than the caller allowed.
    pub shed_rate: u64,
    pub retries_denied: u64,
}

pub(super) struct Group {
    pub(super) name: String,
    pub(super) backend: String,
    pub(super) aliases: Vec<String>,
    pub(super) random: Arc<dyn Random>,
    state: Mutex<GroupState>,
}

pub(super) struct GroupState {
    pub(super) pool: PermitPool,
    pub(super) rate: TokenBucket,
    pub(super) budget: RetryBudget,
    pub(super) breaker: Breaker,
    pub(super) counters: Counters,
}

impl Group {
    pub(super) fn new(
        config: &GroupConfig,
        policy: &GovernorPolicy,
        random: Arc<dyn Random>,
        now: Instant,
    ) -> Self {
        Self {
            name: config.name.clone(),
            backend: config.backend.clone(),
            aliases: config.aliases.clone(),
            random,
            state: Mutex::new(GroupState {
                pool: PermitPool::new(config.permits),
                rate: TokenBucket::new(config.effective_burst(), config.requests_per_min, now),
                budget: RetryBudget::new(
                    policy.retry_window,
                    policy.retry_permille,
                    policy.retry_floor,
                ),
                breaker: Breaker::new(policy, now),
                counters: Counters::default(),
            }),
        }
    }

    /// Never held across an await.
    pub(super) fn lock(&self) -> MutexGuard<'_, GroupState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl GroupState {
    /// Re-evaluates the breaker and hands out whatever permits it now allows.
    pub(super) fn dispatch(&mut self, now: Instant) {
        self.breaker.refresh(now);
        let limit = self.breaker.capacity(now, self.pool.total);
        self.pool.grant(limit, now);
    }

    pub(super) fn record(
        &mut self,
        now: Instant,
        probe: bool,
        signal: Signal,
        random: &dyn Random,
    ) {
        let before = (self.breaker.state(), self.breaker.since());
        self.breaker.record(now, probe, signal, random);
        if (self.breaker.state(), self.breaker.since()) != before {
            self.pool.wake_all();
        }
        self.dispatch(now);
    }

    pub(super) fn next_change(&self, now: Instant) -> Option<Instant> {
        self.breaker.next_change(now, self.pool.total)
    }
}
