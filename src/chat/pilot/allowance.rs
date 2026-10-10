//! The chat allowance: each member's window and the guild pool (v4
//! `ChatPilot.limiter`/`global_limiter`), per-member overrides from the
//! store, v5 refunds, the once-per-episode limited reply, and the Limits
//! snapshot the API reads.

use std::collections::HashMap;

use crate::chat::gate::{
    Budgets, ChatDecision, GLOBAL_KEY, POOL_SPENT, POOL_SPENT_REPLY, RATE_LIMITED_REPLY,
    RateLimiter, retry_note,
};
use crate::domain::model_log::AllowanceOverride;

/// v4 defaults: 4 answers per 300 s each, 12 per 900 s for the guild.
pub const DEFAULT_MEMBER_ALLOWANCE: (usize, f64) = (4, 300.0);
pub const DEFAULT_POOL_ALLOWANCE: (usize, f64) = (12, 900.0);

/// One member's window as the Limits page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct MemberUsage {
    pub member_id: String,
    pub used: usize,
    pub limit: usize,
    pub window_s: f64,
    /// Seconds until the oldest answer in the window frees up.
    pub resets_in_s: f64,
    /// Runs on its own override rather than the shared allowance.
    pub overridden: bool,
}

/// The guild pool's window.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolUsage {
    pub used: usize,
    pub limit: usize,
    pub window_s: f64,
    pub resets_in_s: f64,
}

/// Everything the Limits page reads about allowances; no HTTP here.
#[derive(Clone, Debug, PartialEq)]
pub struct AllowanceSnapshot {
    pub member_default: (usize, f64),
    /// Members with answers in their window, or with an override, by id.
    pub members: Vec<MemberUsage>,
    pub pool: PoolUsage,
}

#[derive(Clone, Debug)]
pub struct Allowance {
    person: RateLimiter,
    pool: RateLimiter,
    member_default: (usize, f64),
    /// Member → monotonic time until which they have been told they are limited.
    told_until: HashMap<String, f64>,
}

impl Default for Allowance {
    fn default() -> Self {
        Self::new(DEFAULT_MEMBER_ALLOWANCE, DEFAULT_POOL_ALLOWANCE)
    }
}

impl Allowance {
    pub fn new(member: (usize, f64), pool: (usize, f64)) -> Self {
        Self {
            person: RateLimiter::new(member.0, member.1),
            pool: RateLimiter::new(pool.0, pool.1),
            member_default: member,
            told_until: HashMap::new(),
        }
    }

    /// Runtime settings (v4 `apply_limits`); overrides replace the whole map.
    pub fn apply(
        &mut self,
        member: (usize, f64),
        pool: (usize, f64),
        overrides: &[AllowanceOverride],
    ) {
        self.member_default = member;
        self.person.set_limits(member.0, member.1);
        self.pool.set_limits(pool.0, pool.1);
        self.person.replace_overrides(overrides.iter().map(|row| {
            let count = usize::try_from(row.count).unwrap_or(usize::MAX);
            // Whole milliseconds in the store, seconds here.
            let window = row.window_ms as f64 / 1000.0;
            (row.member_id.clone(), count, window)
        }));
    }

    /// Both budgets for one `gate::decide` at `now`.
    pub fn budgets(&mut self, now: f64) -> Budgets<'_> {
        Budgets {
            person: Some(&mut self.person),
            pool: Some(&mut self.pool),
            now,
        }
    }

    /// Give back what `gate::decide` spent at `stamp` for `member`: a
    /// question shed, dropped from the queue, cancelled, or turned away
    /// before any model work does not cost the allowance (v5).
    pub fn refund(&mut self, member: &str, stamp: f64) {
        // Independent: the member's hit may already be gone (`forget`) while
        // the pool's is still live.
        self.person.refund(member, stamp);
        self.pool.refund(GLOBAL_KEY, stamp);
    }

    /// The static limited reply, once per refusal episode (v4 `_say_limited`);
    /// `None` when this member was already told.
    pub fn limited_reply(
        &mut self,
        member: &str,
        decision: &ChatDecision,
        now: f64,
    ) -> Option<String> {
        self.told_until.retain(|_, until| *until > now);
        if self.told_until.contains_key(member) {
            return None;
        }
        self.told_until
            .insert(member.to_owned(), now + decision.retry_after_s.max(0.0));
        // Their own allowance, not the default one.
        let count = self.person.limit_for(member).0;
        let template = if decision.reason == POOL_SPENT {
            POOL_SPENT_REPLY
        } else {
            RATE_LIMITED_REPLY
        };
        Some(
            template
                .replace("{count}", &count.to_string())
                .replace("{plural}", if count == 1 { "" } else { "s" })
                .replace("{wait}", &retry_note(decision.retry_after_s)),
        )
    }

    /// Reset one member's window and refusal notice (v4 `forget_limit`).
    pub fn forget(&mut self, member: &str) {
        self.person.reset(Some(member));
        self.told_until.remove(member);
    }

    pub fn snapshot(&self, now: f64) -> AllowanceSnapshot {
        let mut keys = self.person.active(now);
        keys.extend(self.person.overrides().keys().cloned());
        keys.sort();
        keys.dedup();
        let members = keys
            .into_iter()
            .map(|member| {
                let (limit, window_s) = self.person.limit_for(&member);
                MemberUsage {
                    used: self.person.used(&member, now),
                    limit,
                    window_s,
                    resets_in_s: self.person.resets_in(&member, now),
                    overridden: self.person.overrides().contains_key(&member),
                    member_id: member,
                }
            })
            .collect();
        let (limit, window_s) = self.pool.limit_for(GLOBAL_KEY);
        AllowanceSnapshot {
            member_default: self.member_default,
            members,
            pool: PoolUsage {
                used: self.pool.used(GLOBAL_KEY, now),
                limit,
                window_s,
                resets_in_s: self.pool.resets_in(GLOBAL_KEY, now),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    fn spend(allowance: &mut Allowance, member: &str, at: f64) {
        let budgets = allowance.budgets(at);
        assert!(budgets.person.unwrap().allow(member, at));
    }

    fn usage(allowance: &Allowance, member: &str, now: f64) -> MemberUsage {
        allowance
            .snapshot(now)
            .members
            .into_iter()
            .find(|usage| usage.member_id == member)
            .unwrap()
    }

    /// `resets_in_s` counts down to the oldest answer still in the window;
    /// the Limits API turns it into `resets_at`.
    #[test]
    fn resets_in_follows_the_oldest_answer_in_the_window() {
        let mut allowance = Allowance::default();
        for at in [10.0, 40.0, 90.0] {
            spend(&mut allowance, "ren", at);
        }
        let ren = usage(&allowance, "ren", 100.0);
        assert_eq!((ren.used, ren.window_s), (3, 300.0));
        assert_eq!(ren.resets_in_s, 210.0, "10 + 300 - 100");

        // At oldest + window that answer has left: the next one counts.
        let ren = usage(&allowance, "ren", 310.0);
        assert_eq!((ren.used, ren.resets_in_s), (2, 30.0));
        let ren = usage(&allowance, "ren", 389.5);
        assert_eq!((ren.used, ren.resets_in_s), (1, 0.5));
        assert!(allowance.snapshot(390.0).members.is_empty());
    }

    #[test]
    fn an_override_window_sets_when_the_oldest_answer_leaves() {
        let mut allowance = Allowance::default();
        allowance.apply(
            DEFAULT_MEMBER_ALLOWANCE,
            DEFAULT_POOL_ALLOWANCE,
            &[AllowanceOverride {
                member_id: "rin".into(),
                count: 20,
                window_ms: 3_600_000,
                updated_at: DateTime::UNIX_EPOCH,
            }],
        );
        let idle = usage(&allowance, "rin", 0.0);
        assert_eq!(
            (idle.used, idle.resets_in_s, idle.overridden),
            (0, 0.0, true)
        );
        spend(&mut allowance, "rin", 5.0);
        spend(&mut allowance, "rin", 1000.0);
        let rin = usage(&allowance, "rin", 1200.0);
        assert_eq!((rin.used, rin.window_s), (2, 3600.0));
        assert_eq!(rin.resets_in_s, 2405.0, "5 + 3600 - 1200");
    }
}
