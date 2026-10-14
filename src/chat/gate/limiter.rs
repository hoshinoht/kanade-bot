//! A per-key sliding window of answers (v4 `chat/ratelimit.py`), in memory.
//! Monotonic seconds are passed in; nothing here reads a clock. A key may
//! run on its own `(count, window)` override (v4 `chat_rate_limits`, v5
//! `chat_allowance_overrides`), and a spent answer can be refunded (v5).

use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Clone, Debug)]
pub struct RateLimiter {
    count: usize,
    window: f64,
    hits: HashMap<String, VecDeque<f64>>,
    overrides: BTreeMap<String, (usize, f64)>,
}

impl RateLimiter {
    pub fn new(count: usize, window: f64) -> Self {
        Self {
            count,
            window,
            hits: HashMap::new(),
            overrides: BTreeMap::new(),
        }
    }

    /// Replace the shared `(count, window)` (v4 `apply_limits`).
    pub fn set_limits(&mut self, count: usize, window: f64) {
        self.count = count;
        self.window = window;
    }

    /// The `(count, window)` this key runs on.
    pub fn limit_for(&self, key: &str) -> (usize, f64) {
        self.overrides
            .get(key)
            .copied()
            .unwrap_or((self.count, self.window))
    }

    /// Give one key its own allowance; hits already recorded still count.
    pub fn set_override(&mut self, key: &str, count: usize, window: f64) {
        self.overrides.insert(key.to_owned(), (count, window));
    }

    /// Put one key back on the shared allowance; `false` if it already was.
    pub fn clear_override(&mut self, key: &str) -> bool {
        self.overrides.remove(key).is_some()
    }

    /// Swap the whole override map, as loading it from the store does.
    pub fn replace_overrides(&mut self, overrides: impl IntoIterator<Item = (String, usize, f64)>) {
        self.overrides = overrides
            .into_iter()
            .map(|(key, count, window)| (key, (count, window)))
            .collect();
    }

    pub fn overrides(&self) -> &BTreeMap<String, (usize, f64)> {
        &self.overrides
    }

    /// Record an answer for `key` at `now` and say whether it may go out.
    pub fn allow(&mut self, key: &str, now: f64) -> bool {
        let (count, window) = self.limit_for(key);
        let hits = self.hits.entry(key.to_owned()).or_default();
        let cutoff = now - window;
        while hits.front().is_some_and(|&stamp| stamp <= cutoff) {
            hits.pop_front();
        }
        if hits.len() >= count {
            return false;
        }
        hits.push_back(now);
        true
    }

    /// Give back the answer recorded for `key` at `stamp` (shed, turned away
    /// or failed before any model work); `false` when there was none.
    pub fn refund(&mut self, key: &str, stamp: f64) -> bool {
        let Some(hits) = self.hits.get_mut(key) else {
            return false;
        };
        match hits.iter().rposition(|&hit| hit == stamp) {
            Some(at) => {
                hits.remove(at);
                true
            }
            None => false,
        }
    }

    fn live(&self, key: &str, now: f64) -> Vec<f64> {
        let cutoff = now - self.limit_for(key).1;
        self.hits
            .get(key)
            .map(|hits| hits.iter().copied().filter(|&s| s > cutoff).collect())
            .unwrap_or_default()
    }

    /// Answers used in the key's current window; never mutates.
    pub fn used(&self, key: &str, now: f64) -> usize {
        self.live(key, now).len()
    }

    /// Answers left in the current window; never mutates.
    pub fn remaining(&self, key: &str, now: f64) -> usize {
        self.limit_for(key).0.saturating_sub(self.used(key, now))
    }

    /// Seconds until the oldest live hit frees a slot, or `0.0` with room left.
    pub fn retry_after(&self, key: &str, now: f64) -> f64 {
        let (count, window) = self.limit_for(key);
        let live = self.live(key, now);
        if live.len() < count {
            return 0.0;
        }
        live.first()
            .map_or(0.0, |oldest| (oldest + window - now).max(0.0))
    }

    /// Seconds until the oldest live hit expires, room left or not (`0.0`
    /// with none): what a member's own Limits view shows.
    pub fn resets_in(&self, key: &str, now: f64) -> f64 {
        let window = self.limit_for(key).1;
        self.live(key, now)
            .first()
            .map_or(0.0, |oldest| (oldest + window - now).max(0.0))
    }

    /// Keys with live hits, by key.
    pub fn active(&self, now: f64) -> Vec<String> {
        let mut keys: Vec<String> = self
            .hits
            .keys()
            .filter(|key| self.used(key, now) > 0)
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    /// Forget one key's hits (its override stays), or everyone's.
    pub fn reset(&mut self, key: Option<&str>) {
        match key {
            Some(key) => {
                self.hits.remove(key);
            }
            None => self.hits.clear(),
        }
    }
}
