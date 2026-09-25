//! Time-based retention for the model logs (user decision 2026-09-25:
//! 90 days, replacing v4's 500-row cap).

use chrono::{DateTime, TimeDelta, Utc};

/// How long chat and extraction logs (and processed cached messages) are
/// kept until configured otherwise.
pub const DEFAULT_LOG_RETENTION: TimeDelta = TimeDelta::days(90);

/// Rows a store deletes per table per write transaction, so one prune never
/// holds the writer for long.
pub const PRUNE_BATCH: u32 = 500;

/// What one [`ModelLogStore::prune_model_logs`](super::ModelLogStore::prune_model_logs)
/// call deleted (parent rows; their rounds, tools and members go with them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneCounts {
    pub extractions: u64,
    pub chats: u64,
    pub messages: u64,
}

/// The prune cutoff for `now`; `None` when it would leave the representable
/// range (nothing is old enough).
pub fn retention_cutoff(now: DateTime<Utc>, retention: TimeDelta) -> Option<DateTime<Utc>> {
    now.checked_sub_signed(retention)
}
