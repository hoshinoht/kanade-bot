//! Which runs a posted card is for: the reaction router's lookup and the
//! journal view bounded RSVP recovery reads, both implemented by storage.

use chrono::{DateTime, Utc};

/// A card-index lookup failure (storage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupError(pub String);

/// Which runs a posted message is a card for; implemented by storage.
pub trait CardIndex: Send + Sync {
    /// Run ids bound to `message_id` (a Discord snowflake's text), empty when
    /// it is not a run card.
    fn runs_for_message(
        &self,
        message_id: &str,
    ) -> impl Future<Output = Result<Vec<String>, LookupError>> + Send;
}

/// One remotely readable message that still maps to a run. `evidence` is the
/// narrow, fresh bound-card subset; every row remains mapped for removal
/// guards and conflict detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayCard {
    pub channel_id: Option<String>,
    pub message_id: String,
    pub evidence: bool,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// All message mappings for one candidate run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayRunCards {
    pub run_id: String,
    pub cards: Vec<ReplayCard>,
}

/// The journal view needed for bounded RSVP recovery.
pub trait ReplayCards: Send + Sync {
    /// Returns only runs with at least one fresh evidence card, but each
    /// returned run carries every mapped message needed by the removal guard.
    fn replay_cards(
        &self,
        not_before: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<ReplayRunCards>, LookupError>> + Send;
}
