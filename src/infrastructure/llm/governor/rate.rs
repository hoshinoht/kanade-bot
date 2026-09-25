use std::time::Duration;

use tokio::time::Instant;

/// One token in bucket units: a token refills after `UNIT / per_min` nanoseconds.
const UNIT: i128 = 60_000_000_000;

/// Request-rate ceiling. Reservations may drive the level negative (bounded by
/// the caller's wait), which queues takers in reservation order without polling.
#[derive(Debug)]
pub(super) struct TokenBucket {
    level: i128,
    capacity: i128,
    per_min: i128,
    updated: Instant,
}

impl TokenBucket {
    pub(super) fn new(capacity: u32, per_min: u32, now: Instant) -> Self {
        let capacity = i128::from(capacity) * UNIT;
        Self {
            level: capacity,
            capacity,
            per_min: i128::from(per_min),
            updated: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_nanos() as i128;
        self.level = (self.level + elapsed * self.per_min).min(self.capacity);
        self.updated = self.updated.max(now);
    }

    /// Takes one token, returning how long the caller must wait before sending;
    /// refuses (taking nothing) with the needed wait when it exceeds `max_wait`.
    pub(super) fn reserve(
        &mut self,
        now: Instant,
        max_wait: Duration,
    ) -> Result<Duration, Duration> {
        self.refill(now);
        let after = self.level - UNIT;
        if after >= 0 {
            self.level = after;
            return Ok(Duration::ZERO);
        }
        let nanos = (-after + self.per_min - 1) / self.per_min;
        let wait = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
        if wait > max_wait {
            return Err(wait);
        }
        self.level = after;
        Ok(wait)
    }

    pub(super) fn available(&mut self, now: Instant) -> u32 {
        self.refill(now);
        u32::try_from(self.level.max(0) / UNIT).unwrap_or(u32::MAX)
    }

    pub(super) fn capacity(&self) -> u32 {
        u32::try_from(self.capacity / UNIT).unwrap_or(u32::MAX)
    }

    pub(super) fn per_min(&self) -> u32 {
        u32::try_from(self.per_min).unwrap_or(u32::MAX)
    }
}
