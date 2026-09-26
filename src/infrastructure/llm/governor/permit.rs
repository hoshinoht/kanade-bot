use std::{fmt, sync::Arc, time::Duration};

use tokio::time::Instant;

use super::{
    breaker::{Admission, BreakerState, Signal},
    group::Group,
    pool::{CallKind, Priority, QueueKey},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    UnknownRole,
    /// The role's alias is in no backend group.
    Ungrouped,
    /// Pre-screening must stay on local routes.
    ExternalForbidden,
    /// Rewrite and pre-screen calls may only use `try_acquire`.
    MustNotWait,
    /// No free permit right now (`try_acquire`).
    Busy,
    /// Still queued when the wait ran out.
    Timeout,
    /// Breaker open (`retry_at` is the probe time) or a probe is in flight.
    Unavailable {
        retry_at: Option<Instant>,
    },
    /// The rate ceiling would need `wait`, longer than the caller allowed.
    RateLimited {
        wait: Duration,
    },
    RetryBudgetExhausted,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownRole => "role is not configured",
            Self::Ungrouped => "role's model is in no backend group",
            Self::ExternalForbidden => "pre-screen may not use an external route",
            Self::MustNotWait => "this call kind may not queue",
            Self::Busy => "no free model permit",
            Self::Timeout => "timed out waiting for a model permit",
            Self::Unavailable { .. } => "model unavailable",
            Self::RateLimited { .. } => "model request rate ceiling reached",
            Self::RetryBudgetExhausted => "retry budget exhausted",
        })
    }
}

impl std::error::Error for Refused {}

impl Refused {
    /// Configuration or caller errors that no amount of waiting clears; the
    /// rest mean "unavailable now".
    pub fn is_misconfiguration(&self) -> bool {
        matches!(
            self,
            Self::UnknownRole | Self::Ungrouped | Self::ExternalForbidden | Self::MustNotWait
        )
    }
}

/// A queued request for a permit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub priority: Priority,
    pub kind: CallKind,
    /// Human description (a run, a member), never a prompt.
    pub who: String,
}

/// How one provider request ended, as far as backend health is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// The backend answered but the request failed permanently (4xx, invalid output).
    Rejected,
    TransientFailure,
    Timeout,
    BackendUnavailable,
    /// Backend down with the gateway's own breaker cooldown; ours opens for at
    /// least that long (capped at the policy's `max_open_cooldown`).
    BackendUnavailableFor(Duration),
    /// Turned away by gateway admission; says nothing about backend health.
    AdmissionRefused,
}

impl Outcome {
    fn signal(self) -> Signal {
        match self {
            Self::Success | Self::Rejected => Signal::Healthy,
            Self::TransientFailure | Self::Timeout => Signal::Failure,
            Self::BackendUnavailable => Signal::Down(None),
            Self::BackendUnavailableFor(retry_after) => Signal::Down(Some(retry_after)),
            Self::AdmissionRefused => Signal::Neutral,
        }
    }
}

/// One model permit for a whole interaction (all tool rounds); dropping it
/// hands the permit to the next waiter.
pub struct Permit {
    group: Arc<Group>,
    id: u64,
    alias: String,
}

impl fmt::Debug for Permit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Permit")
            .field("group", &self.group.name)
            .field("alias", &self.alias)
            .finish()
    }
}

impl Permit {
    pub fn group(&self) -> &str {
        &self.group.name
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// The group and alias a requeue must return to, whatever the role's
    /// route is by then (sessions keep the model they opened with).
    pub(super) fn pinned(&self) -> (Arc<Group>, String) {
        (self.group.clone(), self.alias.clone())
    }

    /// Admits one first-attempt request, waiting at most `max_wait` for the rate ceiling.
    pub async fn begin_request(&self, max_wait: Duration) -> Result<Attempt, Refused> {
        self.begin(false, max_wait).await
    }

    /// Like `begin_request`, but spends the group's retry budget.
    pub async fn begin_retry(&self, max_wait: Duration) -> Result<Attempt, Refused> {
        self.begin(true, max_wait).await
    }

    /// Admits a request only if a rate token is available now.
    pub fn try_begin_request(&self) -> Result<Attempt, Refused> {
        self.reserve(false, Duration::ZERO)?.0.commit()
    }

    /// Asks the retry gate before backing off: refused unless the breaker admits
    /// work (only a closed one when `closed_only`) and retry budget remains.
    /// `begin_retry` still re-checks and spends.
    pub(in crate::infrastructure::llm) fn check_retry(
        &self,
        closed_only: bool,
    ) -> Result<(), Refused> {
        let now = Instant::now();
        let mut state = self.group.lock();
        state.breaker.refresh(now);
        let closed = state.breaker.state() == BreakerState::Closed;
        let shed = matches!(state.breaker.peek(), Admission::Refused { .. });
        if shed || (closed_only && !closed) {
            // A policy skip (half-open, closed_only) sheds no admissible work.
            if shed {
                state.counters.shed_unavailable += 1;
            }
            return Err(Refused::Unavailable {
                retry_at: state.breaker.retry_at(),
            });
        }
        if state.budget.remaining(now) == 0 {
            state.counters.retries_denied += 1;
            return Err(Refused::RetryBudgetExhausted);
        }
        Ok(())
    }

    async fn begin(&self, retry: bool, max_wait: Duration) -> Result<Attempt, Refused> {
        let (reservation, wait) = self.reserve(retry, max_wait)?;
        if !wait.is_zero() {
            // Cancelled here, the reservation's drop refunds the token.
            tokio::time::sleep(wait).await;
        }
        reservation.commit()
    }

    /// Checks breaker and budget and reserves a rate token; the request is
    /// recorded only when the reservation commits.
    fn reserve(&self, retry: bool, max_wait: Duration) -> Result<(Reservation, Duration), Refused> {
        let now = Instant::now();
        let mut state = self.group.lock();
        state.breaker.refresh(now);
        if let Admission::Refused { retry_at } = state.breaker.peek() {
            state.counters.shed_unavailable += 1;
            return Err(Refused::Unavailable { retry_at });
        }
        if retry && state.budget.remaining(now) == 0 {
            state.counters.retries_denied += 1;
            return Err(Refused::RetryBudgetExhausted);
        }
        let wait = match state.rate.reserve(now, max_wait) {
            Ok(wait) => wait,
            Err(wait) => {
                state.counters.shed_rate += 1;
                return Err(Refused::RateLimited { wait });
            }
        };
        let probe = state.breaker.admit() == Admission::Probe;
        Ok((
            Reservation {
                group: self.group.clone(),
                retry,
                probe,
                live: true,
            },
            wait,
        ))
    }
}

/// A rate token (and possibly the probe slot) held while waiting to send.
struct Reservation {
    group: Arc<Group>,
    retry: bool,
    probe: bool,
    live: bool,
}

impl Reservation {
    fn commit(mut self) -> Result<Attempt, Refused> {
        let now = Instant::now();
        let mut state = self.group.lock();
        state.breaker.refresh(now);
        // A breaker that opened while this request waited for its token wins.
        if !self.probe && state.breaker.state() != BreakerState::Closed {
            state.counters.shed_unavailable += 1;
            return Err(Refused::Unavailable {
                retry_at: state.breaker.retry_at(),
            });
        }
        if self.retry {
            if !state.budget.try_spend(now) {
                state.counters.retries_denied += 1;
                return Err(Refused::RetryBudgetExhausted);
            }
            state.counters.retries += 1;
        } else {
            state.budget.record_request(now);
            state.counters.requests += 1;
        }
        drop(state);
        self.live = false;
        Ok(Attempt {
            group: self.group.clone(),
            probe: self.probe,
            done: false,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        let now = Instant::now();
        let mut state = self.group.lock();
        state.rate.refund(now);
        if self.probe {
            state.record(now, true, Signal::Neutral, self.group.random.as_ref());
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.group.lock();
        state.pool.release(self.id);
        state.dispatch(Instant::now());
    }
}

/// One admitted provider request; report how it ended with `finish`.
/// Dropping it unfinished counts as abandoned (frees a half-open probe).
pub struct Attempt {
    group: Arc<Group>,
    probe: bool,
    done: bool,
}

impl fmt::Debug for Attempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attempt")
            .field("group", &self.group.name)
            .field("probe", &self.probe)
            .finish()
    }
}

impl Attempt {
    /// This request is the breaker's single half-open probe.
    pub fn is_probe(&self) -> bool {
        self.probe
    }

    pub fn finish(mut self, outcome: Outcome) {
        self.done = true;
        let now = Instant::now();
        let mut state = self.group.lock();
        let counters = &mut state.counters;
        match outcome {
            Outcome::Success | Outcome::Rejected => counters.successes += 1,
            Outcome::TransientFailure => counters.transient_failures += 1,
            Outcome::Timeout => counters.timeouts += 1,
            Outcome::BackendUnavailable | Outcome::BackendUnavailableFor(_) => {
                counters.backend_unavailable += 1
            }
            Outcome::AdmissionRefused => counters.admission_refused += 1,
        }
        state.record(
            now,
            self.probe,
            outcome.signal(),
            self.group.random.as_ref(),
        );
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let now = Instant::now();
        let mut state = self.group.lock();
        state.record(now, self.probe, Signal::Neutral, self.group.random.as_ref());
    }
}

/// Cancellation guard: a dropped waiter leaves the queue, or gives back a
/// permit granted in the same instant.
struct Waiter<'a> {
    group: &'a Arc<Group>,
    key: QueueKey,
    armed: bool,
}

impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.group.lock();
        if !state.pool.cancel(&self.key) {
            state.pool.release(self.key.1);
            state.dispatch(Instant::now());
        }
    }
}

pub(super) async fn acquire(
    group: Arc<Group>,
    alias: String,
    ticket: Ticket,
    wait: Duration,
) -> Result<Permit, Refused> {
    let now = Instant::now();
    let deadline = now
        .checked_add(wait)
        .unwrap_or(now + Duration::from_secs(86_400));
    let (key, wake) = {
        let mut state = group.lock();
        state.breaker.refresh(now);
        if state.breaker.state() == BreakerState::Open {
            state.counters.shed_unavailable += 1;
            return Err(Refused::Unavailable {
                retry_at: state.breaker.retry_at(),
            });
        }
        let queued = state
            .pool
            .enqueue(ticket.priority, ticket.kind, ticket.who, now);
        state.dispatch(now);
        queued
    };
    let mut waiter = Waiter {
        group: &group,
        key,
        armed: true,
    };
    loop {
        let wake_at = {
            let state = group.lock();
            if !state.pool.is_queued(&key) {
                break;
            }
            let now = Instant::now();
            state
                .next_change(now)
                .map_or(deadline, |at| at.min(deadline))
        };
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep_until(wake_at) => {}
        }
        let now = Instant::now();
        let mut state = group.lock();
        if !state.pool.is_queued(&key) {
            break;
        }
        state.dispatch(now);
        if !state.pool.is_queued(&key) {
            break;
        }
        if now >= deadline {
            state.pool.cancel(&key);
            state.counters.shed_timeout += 1;
            waiter.armed = false;
            return Err(Refused::Timeout);
        }
    }
    waiter.armed = false;
    Ok(Permit {
        group: group.clone(),
        id: key.1,
        alias,
    })
}

pub(super) fn try_acquire(
    group: Arc<Group>,
    alias: String,
    kind: CallKind,
    who: String,
) -> Result<Permit, Refused> {
    let now = Instant::now();
    let mut state = group.lock();
    state.breaker.refresh(now);
    // Half-open with no probe in flight: a try-only group (rewrite) must be able
    // to probe too, or it would never leave half-open.
    if let Admission::Refused { .. } = state.breaker.peek() {
        state.counters.shed_unavailable += 1;
        return Err(Refused::Unavailable {
            retry_at: state.breaker.retry_at(),
        });
    }
    let limit = state.breaker.capacity(now, state.pool.total);
    let Some(id) = state.pool.try_take(limit, kind, who, now) else {
        state.counters.shed_busy += 1;
        return Err(Refused::Busy);
    };
    drop(state);
    Ok(Permit { group, id, alias })
}
