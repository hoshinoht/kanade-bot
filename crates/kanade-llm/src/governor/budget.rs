use std::{collections::VecDeque, time::Duration};

use tokio::time::Instant;

/// Memory bound; the rate ceiling keeps real traffic far below it.
const MAX_TRACKED: usize = 65_536;

/// Sliding-window retry budget: retries ≤ max(floor, permille × first attempts).
#[derive(Debug)]
pub(super) struct RetryBudget {
    window: Duration,
    permille: u16,
    floor: u32,
    requests: VecDeque<Instant>,
    retries: VecDeque<Instant>,
}

impl RetryBudget {
    pub(super) fn new(window: Duration, permille: u16, floor: u32) -> Self {
        Self {
            window,
            permille,
            floor,
            requests: VecDeque::new(),
            retries: VecDeque::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        let window = self.window;
        let stale = |at: &Instant| now.saturating_duration_since(*at) >= window;
        while self.requests.front().is_some_and(stale) {
            self.requests.pop_front();
        }
        while self.retries.front().is_some_and(stale) {
            self.retries.pop_front();
        }
    }

    fn push(queue: &mut VecDeque<Instant>, now: Instant) {
        if queue.len() == MAX_TRACKED {
            queue.pop_front();
        }
        queue.push_back(now);
    }

    pub(super) fn record_request(&mut self, now: Instant) {
        self.prune(now);
        Self::push(&mut self.requests, now);
    }

    pub(super) fn capacity(&mut self, now: Instant) -> u32 {
        self.prune(now);
        let share = self.requests.len() as u64 * u64::from(self.permille) / 1_000;
        u32::try_from(share).unwrap_or(u32::MAX).max(self.floor)
    }

    pub(super) fn remaining(&mut self, now: Instant) -> u32 {
        let capacity = self.capacity(now);
        capacity.saturating_sub(u32::try_from(self.retries.len()).unwrap_or(u32::MAX))
    }

    pub(super) fn try_spend(&mut self, now: Instant) -> bool {
        if self.remaining(now) == 0 {
            return false;
        }
        Self::push(&mut self.retries, now);
        true
    }
}
