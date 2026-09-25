use std::time::Duration;

use tokio::time::Instant;

use super::{
    config::GovernorPolicy,
    jitter::{Random, unit},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    HalfOpen,
    Open,
}

impl BreakerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::HalfOpen => "half_open",
            Self::Open => "open",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    /// `drain_from` is set after recovery while permits ramp back up.
    Closed {
        drain_from: Option<Instant>,
    },
    Open {
        retry_at: Instant,
    },
    HalfOpen {
        probing: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Admission {
    Normal,
    Probe,
    Refused { retry_at: Option<Instant> },
}

/// What an attempt's outcome says about the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Signal {
    Healthy,
    Failure,
    Down,
    Neutral,
}

#[derive(Debug)]
pub(super) struct Breaker {
    phase: Phase,
    since: Instant,
    failures: u32,
    trips: u32,
    threshold: u32,
    cooldown: Duration,
    max_cooldown: Duration,
    drain_step: Duration,
}

impl Breaker {
    pub(super) fn new(policy: &GovernorPolicy, now: Instant) -> Self {
        Self {
            phase: Phase::Closed { drain_from: None },
            since: now,
            failures: 0,
            trips: 0,
            threshold: policy.breaker_threshold,
            cooldown: policy.open_cooldown,
            max_cooldown: policy.max_open_cooldown,
            drain_step: policy.drain_step,
        }
    }

    pub(super) fn refresh(&mut self, now: Instant) {
        if let Phase::Open { retry_at } = self.phase
            && now >= retry_at
        {
            self.phase = Phase::HalfOpen { probing: false };
            self.since = retry_at;
        }
    }

    pub(super) fn state(&self) -> BreakerState {
        match self.phase {
            Phase::Closed { .. } => BreakerState::Closed,
            Phase::HalfOpen { .. } => BreakerState::HalfOpen,
            Phase::Open { .. } => BreakerState::Open,
        }
    }

    pub(super) fn since(&self) -> Instant {
        self.since
    }

    pub(super) fn failures(&self) -> u32 {
        self.failures
    }

    pub(super) fn retry_at(&self) -> Option<Instant> {
        match self.phase {
            Phase::Open { retry_at } => Some(retry_at),
            _ => None,
        }
    }

    fn drained_steps(&self, from: Instant, now: Instant) -> u128 {
        now.saturating_duration_since(from).as_nanos() / self.drain_step.as_nanos()
    }

    /// Permits that may be held right now.
    pub(super) fn capacity(&self, now: Instant, total: u32) -> u32 {
        match self.phase {
            Phase::Closed { drain_from: None } => total,
            Phase::Closed {
                drain_from: Some(from),
            } => {
                let steps = self.drained_steps(from, now).saturating_add(1);
                u32::try_from(steps).unwrap_or(u32::MAX).min(total)
            }
            Phase::HalfOpen { .. } => 1.min(total),
            Phase::Open { .. } => 0,
        }
    }

    /// When capacity next changes on its own (probe time or the next drain step).
    pub(super) fn next_change(&self, now: Instant, total: u32) -> Option<Instant> {
        match self.phase {
            Phase::Open { retry_at } => Some(retry_at),
            Phase::Closed {
                drain_from: Some(from),
            } if self.capacity(now, total) < total => {
                let next = u32::try_from(self.drained_steps(from, now) + 1).ok()?;
                from.checked_add(self.drain_step.checked_mul(next)?)
            }
            _ => None,
        }
    }

    pub(super) fn peek(&self) -> Admission {
        match self.phase {
            Phase::Closed { .. } => Admission::Normal,
            Phase::HalfOpen { probing: false } => Admission::Probe,
            Phase::HalfOpen { probing: true } => Admission::Refused { retry_at: None },
            Phase::Open { retry_at } => Admission::Refused {
                retry_at: Some(retry_at),
            },
        }
    }

    pub(super) fn admit(&mut self) -> Admission {
        let admission = self.peek();
        if admission == Admission::Probe {
            self.phase = Phase::HalfOpen { probing: true };
        }
        admission
    }

    /// Outcomes of attempts started before a trip are ignored while open.
    pub(super) fn record(
        &mut self,
        now: Instant,
        probe: bool,
        signal: Signal,
        random: &dyn Random,
    ) {
        let probing = matches!(self.phase, Phase::HalfOpen { probing: true });
        if probe && probing {
            match signal {
                Signal::Healthy => {
                    self.phase = Phase::Closed {
                        drain_from: Some(now),
                    };
                    self.since = now;
                    self.failures = 0;
                    self.trips = 0;
                }
                Signal::Failure | Signal::Down => {
                    self.failures = self.failures.saturating_add(1);
                    self.trip(now, random);
                }
                Signal::Neutral => self.phase = Phase::HalfOpen { probing: false },
            }
            return;
        }
        if !matches!(self.phase, Phase::Closed { .. }) {
            return;
        }
        match signal {
            Signal::Healthy => self.failures = 0,
            Signal::Failure => {
                self.failures = self.failures.saturating_add(1);
                if self.failures >= self.threshold {
                    self.trip(now, random);
                }
            }
            Signal::Down => {
                self.failures = self.failures.saturating_add(1);
                self.trip(now, random);
            }
            Signal::Neutral => {}
        }
    }

    /// Cooldown doubles per consecutive trip, capped, then jittered to [1, 1.5)×.
    fn trip(&mut self, now: Instant, random: &dyn Random) {
        let factor = 1u32.checked_shl(self.trips.min(16)).unwrap_or(u32::MAX);
        let base = self
            .cooldown
            .checked_mul(factor)
            .unwrap_or(self.max_cooldown)
            .min(self.max_cooldown);
        let wait = base + base.mul_f64(unit(random) / 2.0);
        self.trips = self.trips.saturating_add(1);
        self.since = now;
        self.phase = Phase::Open {
            retry_at: now + wait,
        };
    }
}
