//! Posted test cards (migration 0017 `debug_cards`): claimed with the
//! journal attempt, bound to the message, cleared by `/debug clear_test`.

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::domain::scheduler::StoreError;

/// A bound test card still in its channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostedDebugCard {
    pub channel_id: String,
    pub message_id: String,
    pub run_id: String,
    /// The `/debug ping` kind (`day_of`, `countdown_60`, `amend`, …).
    pub kind: String,
    pub posted_at: DateTime<Utc>,
}

pub trait DebugCardStore: Send + Sync {
    /// Bound, uncleared test cards in `channel_id` posted at or after
    /// `since`, oldest first.
    fn debug_cards_in(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<PostedDebugCard>, StoreError>> + Send;

    /// Mark a test card cleared (its message is gone): it is no longer
    /// listed or refreshed. `false` if unknown or already cleared.
    fn clear_debug_card(
        &self,
        message_id: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}
