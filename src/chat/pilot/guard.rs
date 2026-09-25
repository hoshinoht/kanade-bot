//! Clean-retry storm control (parent design 2026-09-25, binding): at most
//! one clean retry per member per 10 minutes, and a guild-wide guard that
//! suspends clean retries for a cooldown once they exceed a small rate,
//! raising one admin alert. The governor's retry budget and breaker still
//! apply to every retry that is allowed here. Monotonic seconds passed in.

use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuardLimits {
    pub per_member_s: f64,
    /// Clean retries per `storm_window_s` that trip the guard.
    pub storm_threshold: usize,
    pub storm_window_s: f64,
    pub cooldown_s: f64,
}

impl Default for GuardLimits {
    fn default() -> Self {
        Self {
            per_member_s: 600.0,
            storm_threshold: 3,
            storm_window_s: 60.0,
            cooldown_s: 600.0,
        }
    }
}

/// Raised once when the guard trips.
#[derive(Clone, Debug, PartialEq)]
pub struct StormAlert {
    pub retries: usize,
    pub window_s: f64,
    /// Monotonic time clean retries resume.
    pub suspended_until: f64,
}

/// The Limits page's view.
#[derive(Clone, Debug, PartialEq)]
pub struct GuardView {
    pub recent: usize,
    pub suspended_until: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct CleanRetryGuard {
    limits: GuardLimits,
    last_by_member: HashMap<String, f64>,
    recent: VecDeque<f64>,
    suspended_until: Option<f64>,
}

impl CleanRetryGuard {
    pub fn new(limits: GuardLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    fn prune(&mut self, now: f64) {
        while self
            .recent
            .front()
            .is_some_and(|&at| now - at >= self.limits.storm_window_s)
        {
            self.recent.pop_front();
        }
        if self.suspended_until.is_some_and(|until| now >= until) {
            self.suspended_until = None;
        }
    }

    /// May `member`'s question spend its clean retry now?
    pub fn allows(&mut self, member: &str, now: f64) -> bool {
        self.prune(now);
        if self.suspended_until.is_some() {
            return false;
        }
        self.last_by_member
            .get(member)
            .is_none_or(|&at| now - at >= self.limits.per_member_s)
    }

    /// A clean retry was sent; returns the alert when this one trips the guard.
    pub fn record(&mut self, member: &str, now: f64) -> Option<StormAlert> {
        self.prune(now);
        self.last_by_member.insert(member.to_owned(), now);
        self.recent.push_back(now);
        if self.suspended_until.is_none() && self.recent.len() > self.limits.storm_threshold {
            let until = now + self.limits.cooldown_s;
            self.suspended_until = Some(until);
            return Some(StormAlert {
                retries: self.recent.len(),
                window_s: self.limits.storm_window_s,
                suspended_until: until,
            });
        }
        None
    }

    pub fn view(&mut self, now: f64) -> GuardView {
        self.prune(now);
        GuardView {
            recent: self.recent.len(),
            suspended_until: self.suspended_until,
        }
    }
}
