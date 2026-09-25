//! Problems an operator must see, kept out of public posts.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, TimeDelta, Utc};

use crate::bot::transport::RejectionKind;
use crate::domain::notify::{AttemptId, EffectKind};

/// Identical alerts (same [`AdminAlert::key`]) pass at most once per window.
pub const ALERT_WINDOW: TimeDelta = TimeDelta::hours(1);

/// One admin-visible event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminAlert {
    /// Discord definitely refused a post; it was retired and is not retried.
    SendRejected {
        effect: EffectKind,
        channel_id: String,
        reason: RejectionKind,
    },
    /// The runs' home channel is unset or unreachable, so the post went to
    /// the post channel instead.
    HomeChannelUnavailable {
        home_channel_id: Option<String>,
        used_channel_id: String,
        run_ids: Vec<String>,
    },
    /// The week's digest card could not be confirmed deleted, so no
    /// replacement was posted.
    DigestReplacementSuppressed {
        week_start: DateTime<Utc>,
        channel_id: String,
        message_id: String,
        reason: String,
    },
    /// The clock reads a boss week before the last posted digest; nothing
    /// was posted (v5 deviation: v4 re-posted).
    DigestClockRollback {
        current_week: DateTime<Utc>,
        last_digest_week: DateTime<Utc>,
    },
    /// The boss week's automatic history checkpoint could not be written;
    /// the tick carried on and the next tick retries.
    CheckpointFailed {
        week_start: DateTime<Utc>,
        detail: String,
    },
    /// Expiring past-week drafts failed; the tick carried on and the next
    /// tick retries.
    DraftExpiryFailed { detail: String },
    /// v5 attendance: recounting run statuses failed; the tick carried on
    /// and the next tick retries.
    AttendanceRecountFailed { detail: String },
    /// A journal write for one send failed and the tick moved on. With an
    /// `attempt`, that intent stays held until restart recovery marks it
    /// indeterminate; it is never resent.
    JournalFailure {
        attempt: Option<AttemptId>,
        effect: EffectKind,
        channel_id: String,
        detail: String,
    },
    /// The extraction backlog was full and dropped its oldest messages
    /// (unread; a rescan can still read them from the cache).
    BacklogDropped { messages: usize, capacity: usize },
    /// A ✅/❌ on a proposal card failed for a reason members must not see
    /// (store failure, retries exhausted); nothing is posted publicly.
    CardAnswerFailed { proposal_id: String, detail: String },
}

impl AdminAlert {
    /// What makes two alerts "the same" for throttling: kind plus target.
    pub fn key(&self) -> String {
        match self {
            Self::SendRejected {
                channel_id, reason, ..
            } => format!("rejected:{channel_id}:{reason:?}"),
            Self::HomeChannelUnavailable {
                home_channel_id,
                used_channel_id,
                ..
            } => format!("home:{home_channel_id:?}:{used_channel_id}"),
            Self::DigestReplacementSuppressed {
                week_start,
                message_id,
                ..
            } => format!("replacement:{week_start}:{message_id}"),
            Self::DigestClockRollback {
                current_week,
                last_digest_week,
            } => format!("rollback:{current_week}:{last_digest_week}"),
            Self::JournalFailure {
                attempt,
                channel_id,
                ..
            } => format!("journal:{attempt:?}:{channel_id}"),
            Self::CheckpointFailed { week_start, .. } => format!("checkpoint:{week_start}"),
            Self::DraftExpiryFailed { .. } => "draft-expiry".to_owned(),
            Self::AttendanceRecountFailed { .. } => "attendance-recount".to_owned(),
            Self::BacklogDropped { .. } => "extraction-backlog".to_owned(),
            Self::CardAnswerFailed { proposal_id, .. } => format!("card-answer:{proposal_id}"),
        }
    }
}

/// Rate-limits repeated alerts on the tick's own clock.
#[derive(Debug, Default)]
pub struct AlertThrottle {
    last: Mutex<BTreeMap<String, DateTime<Utc>>>,
}

impl AlertThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when `alert` has not passed within [`ALERT_WINDOW`] before `now`.
    /// A clock that moved backwards counts as outside the window.
    pub fn admit(&self, alert: &AdminAlert, now: DateTime<Utc>) -> bool {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let key = alert.key();
        if last
            .get(&key)
            .is_some_and(|at| *at <= now && now - *at < ALERT_WINDOW)
        {
            return false;
        }
        last.insert(key, now);
        true
    }
}

/// Where alerts go. Implementations must not block or fail the tick.
pub trait AlertSink: Send + Sync {
    fn alert(&self, alert: AdminAlert);
}

/// Keeps every alert in memory, in order.
#[derive(Debug, Default)]
pub struct AlertRecorder {
    alerts: Mutex<Vec<AdminAlert>>,
}

impl AlertRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alerts(&self) -> Vec<AdminAlert> {
        self.alerts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl AlertSink for AlertRecorder {
    fn alert(&self, alert: AdminAlert) {
        self.alerts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(alert);
    }
}
