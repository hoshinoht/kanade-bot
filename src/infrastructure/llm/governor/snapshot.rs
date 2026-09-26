use chrono::{DateTime, TimeDelta, Utc};
use tokio::time::Instant;

use super::{breaker::BreakerState, group::Counters, group::Group, pool::CallKind};

/// One backend group as the Limits page (`BackendGroup` DTO) shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupSnapshot {
    pub name: String,
    pub backend: String,
    pub models: Vec<String>,
    pub permits: PermitUsage,
    pub queue: Vec<QueuedCall>,
    /// Not in the DTO yet; the v5 decision asks the Limits page to show holders.
    pub holders: Vec<HeldPermit>,
    pub rate: RateLevel,
    pub retry: RetryLevel,
    pub breaker: BreakerView,
    pub counters: Counters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermitUsage {
    pub in_use: u32,
    pub total: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedCall {
    /// 1-based and dense within the group.
    pub position: u32,
    pub kind: CallKind,
    pub who: String,
    pub waiting_s: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldPermit {
    pub kind: CallKind,
    pub who: String,
    pub held_s: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLevel {
    pub available: u32,
    pub capacity: u32,
    pub refill_per_min: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryLevel {
    pub remaining: u32,
    pub capacity: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakerView {
    pub state: BreakerState,
    pub failures: u32,
    pub since: DateTime<Utc>,
    /// Set only while a probe is scheduled (open).
    pub retry_at: Option<DateTime<Utc>>,
}

/// Maps monotonic instants onto the caller's wall clock at `now`.
fn wall(at: Instant, now: Instant, wall_now: DateTime<Utc>) -> DateTime<Utc> {
    let delta = |d| TimeDelta::from_std(d).unwrap_or(TimeDelta::MAX);
    let shifted = if at <= now {
        wall_now.checked_sub_signed(delta(now - at))
    } else {
        wall_now.checked_add_signed(delta(at - now))
    };
    shifted.unwrap_or(wall_now)
}

pub(super) fn capture(group: &Group, now: Instant, wall_now: DateTime<Utc>) -> GroupSnapshot {
    let mut state = group.lock();
    state.breaker.refresh(now);
    let queue = state
        .pool
        .queue
        .values()
        .zip(1..)
        .map(|(entry, position)| QueuedCall {
            position,
            kind: entry.kind,
            who: entry.who.clone(),
            waiting_s: now.saturating_duration_since(entry.since).as_secs(),
        })
        .collect();
    let holders = state
        .pool
        .holders
        .values()
        .map(|holder| HeldPermit {
            kind: holder.kind,
            who: holder.who.clone(),
            held_s: now.saturating_duration_since(holder.since).as_secs(),
        })
        .collect();
    let rate = RateLevel {
        available: state.rate.available(now),
        capacity: state.rate.capacity(),
        refill_per_min: state.rate.per_min(),
    };
    let retry = RetryLevel {
        remaining: state.budget.remaining(now),
        capacity: state.budget.capacity(now),
    };
    let breaker = BreakerView {
        state: state.breaker.state(),
        failures: state.breaker.failures(),
        since: wall(state.breaker.since(), now, wall_now),
        retry_at: state.breaker.retry_at().map(|at| wall(at, now, wall_now)),
    };
    GroupSnapshot {
        name: group.name.clone(),
        backend: group.backend.clone(),
        models: group.aliases(),
        permits: PermitUsage {
            in_use: state.pool.in_use,
            total: state.pool.total,
        },
        queue,
        holders,
        rate,
        retry,
        breaker,
        counters: state.counters,
    }
}
