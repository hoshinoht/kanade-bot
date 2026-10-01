//! Durable decline-notice candidates and their delivery bindings.

use chrono::{DateTime, TimeDelta, Utc};

use crate::domain::history::ChangeMeta;
use crate::domain::schedule::ChangeSet;
use crate::domain::scheduler::{Committed, StoreError};

/// v4's interval before another decline post for the same member and run.
pub const DECLINE_NOTICE_COOLDOWN: TimeDelta = TimeDelta::hours(6);
/// Maximum candidates one recovery tick examines before the next tick resumes.
pub const MAX_PENDING_DECLINE_NOTICES: usize = 100;

/// One member's decline-notice state for one run.
///
/// `display_name` is absent on rows written before schema v20. `message_id` is
/// absent before a confirmed post and after a confirmed deletion. The
/// notification timestamp deliberately survives that deletion for cooldown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclineNotice {
    pub run_id: String,
    pub user_id: String,
    pub channel_id: Option<String>,
    pub reference_id: Option<String>,
    pub display_name: Option<String>,
    pub message_id: Option<String>,
    pub notified_at: DateTime<Utc>,
    pub retract_pending: bool,
}

impl DeclineNotice {
    /// A new candidate is unbound and has no pending retraction.
    pub fn candidate(
        run_id: impl Into<String>,
        user_id: impl Into<String>,
        channel_id: Option<String>,
        reference_id: Option<String>,
        display_name: impl Into<String>,
        notified_at: DateTime<Utc>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            user_id: user_id.into(),
            channel_id,
            reference_id,
            display_name: Some(display_name.into()),
            message_id: None,
            notified_at,
            retract_pending: false,
        }
    }
}

/// Store operations for decline-notice candidates and delivery bindings.
///
/// A candidate is committed beside the RSVP change through
/// [`Self::commit_with_decline_notices`]. Binding and retraction are delivery
/// state, so they do not write a second schedule history record.
pub trait DeclineNoticeStore {
    /// Commit schedule changes, candidate upserts and retraction marks in one
    /// transaction. Retraction rows name an existing `(run_id, user_id)` and
    /// are harmless when the candidate is absent.
    fn commit_with_decline_notices(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        candidates: Vec<DeclineNotice>,
        retractions: Vec<(String, String)>,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send;

    /// Read one decline-notice row.
    fn decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<DeclineNotice>, StoreError>> + Send;

    /// Recovery work in `(notified_at, run_id, user_id)` order, capped at
    /// `limit` and [`MAX_PENDING_DECLINE_NOTICES`]. It includes unbound
    /// candidates and bound rows whose retraction still needs a delete.
    fn pending_decline_notices(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<DeclineNotice>, StoreError>> + Send;

    /// Bind the exact unbound candidate to Discord's confirmed receipt.
    /// Returns false when it no longer exists or was already bound.
    fn bind_decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Remember a retraction while the candidate is still being delivered.
    /// Returns false when no candidate exists.
    fn mark_decline_retract_pending(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Clear only this exact, confirmed-deleted message binding. The cooldown
    /// timestamp is retained. Returns false for a missing or different binding.
    fn clear_decline_notice_message(
        &self,
        run_id: &str,
        user_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Whether the stored notification timestamp still suppresses a new post.
    fn decline_notice_on_cooldown(
        &self,
        run_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}

impl<T: DeclineNoticeStore + Send + Sync> DeclineNoticeStore for std::sync::Arc<T> {
    fn commit_with_decline_notices(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        candidates: Vec<DeclineNotice>,
        retractions: Vec<(String, String)>,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send {
        (**self).commit_with_decline_notices(
            expected_revision,
            changes,
            meta,
            candidates,
            retractions,
        )
    }

    fn decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<DeclineNotice>, StoreError>> + Send {
        (**self).decline_notice(run_id, user_id)
    }

    fn pending_decline_notices(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<DeclineNotice>, StoreError>> + Send {
        (**self).pending_decline_notices(limit)
    }

    fn bind_decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).bind_decline_notice(run_id, user_id, channel_id, message_id)
    }

    fn mark_decline_retract_pending(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).mark_decline_retract_pending(run_id, user_id)
    }

    fn clear_decline_notice_message(
        &self,
        run_id: &str,
        user_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).clear_decline_notice_message(run_id, user_id, message_id)
    }

    fn decline_notice_on_cooldown(
        &self,
        run_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).decline_notice_on_cooldown(run_id, user_id, now)
    }
}

impl<T: DeclineNoticeStore + Sync> DeclineNoticeStore for &T {
    fn commit_with_decline_notices(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        candidates: Vec<DeclineNotice>,
        retractions: Vec<(String, String)>,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send {
        (**self).commit_with_decline_notices(
            expected_revision,
            changes,
            meta,
            candidates,
            retractions,
        )
    }

    fn decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<DeclineNotice>, StoreError>> + Send {
        (**self).decline_notice(run_id, user_id)
    }

    fn pending_decline_notices(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<DeclineNotice>, StoreError>> + Send {
        (**self).pending_decline_notices(limit)
    }

    fn bind_decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).bind_decline_notice(run_id, user_id, channel_id, message_id)
    }

    fn mark_decline_retract_pending(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).mark_decline_retract_pending(run_id, user_id)
    }

    fn clear_decline_notice_message(
        &self,
        run_id: &str,
        user_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).clear_decline_notice_message(run_id, user_id, message_id)
    }

    fn decline_notice_on_cooldown(
        &self,
        run_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send {
        (**self).decline_notice_on_cooldown(run_id, user_id, now)
    }
}
