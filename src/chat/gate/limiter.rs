//! A per-key sliding window of answers (v4 `chat/ratelimit.py`), in memory.
//! Monotonic seconds are passed in; nothing here reads a clock.

use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug)]
pub struct RateLimiter {
    count: usize,
    window: f64,
    hits: HashMap<String, VecDeque<f64>>,
}

impl RateLimiter {
    pub fn new(count: usize, window: f64) -> Self {
        Self {
            count,
            window,
            hits: HashMap::new(),
        }
    }

    /// Record an answer for `key` at `now` and say whether it may go out.
    pub fn allow(&mut self, key: &str, now: f64) -> bool {
        let hits = self.hits.entry(key.to_owned()).or_default();
        let cutoff = now - self.window;
        while hits.front().is_some_and(|&stamp| stamp <= cutoff) {
            hits.pop_front();
        }
        if hits.len() >= self.count {
            return false;
        }
        hits.push_back(now);
        true
    }

    fn live(&self, key: &str, now: f64) -> Vec<f64> {
        let cutoff = now - self.window;
        self.hits
            .get(key)
            .map(|hits| hits.iter().copied().filter(|&s| s > cutoff).collect())
            .unwrap_or_default()
    }

    /// Answers left in the current window; never mutates.
    pub fn remaining(&self, key: &str, now: f64) -> usize {
        self.count.saturating_sub(self.live(key, now).len())
    }

    /// Seconds until the oldest live hit frees a slot, or `0.0` with room left.
    pub fn retry_after(&self, key: &str, now: f64) -> f64 {
        let live = self.live(key, now);
        if live.len() < self.count {
            return 0.0;
        }
        live.first()
            .map_or(0.0, |oldest| (oldest + self.window - now).max(0.0))
    }
}
