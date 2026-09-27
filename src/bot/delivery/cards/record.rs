//! What a reminder card was posted as, stored before its claim (keyed by
//! the send's native dedupe key) so a retry and every later edit render the
//! same card: its kind and saved header (day-of heading or short phrase).

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::domain::scheduler::StoreError;

/// The record kind of a day-of card; countdowns use their reminder kind
/// (`countdown_<minutes>`).
pub const DAY_OF_KIND: &str = "day_of";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRecord {
    /// `day_of` or `countdown_<minutes>`.
    pub kind: String,
    /// Day-of heading without framing, or the short countdown phrase.
    pub heading: Option<String>,
}

/// A bound reminder card with a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostedCard {
    pub channel_id: String,
    pub message_id: String,
    /// Every run the card is for (its card→run rows).
    pub run_ids: Vec<String>,
    pub record: CardRecord,
    /// A `/debug ping` test card (no record row; the heading is the seed).
    pub test: bool,
}

/// Reminder card records (migration 0014 `reminder_cards`).
pub trait ReminderCardStore: Send + Sync {
    fn card_record(
        &self,
        dedupe_key: &str,
    ) -> impl Future<Output = Result<Option<CardRecord>, StoreError>> + Send;

    /// Insert unless a record exists; the first write wins and is returned.
    fn save_card_record(
        &self,
        dedupe_key: &str,
        record: &CardRecord,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<CardRecord, StoreError>> + Send;

    /// Bound reminder cards naming `run_id` that have a record, and its
    /// bound, uncleared day-of/countdown test cards, by message id.
    fn posted_cards(
        &self,
        run_id: &str,
    ) -> impl Future<Output = Result<Vec<PostedCard>, StoreError>> + Send;
}

/// Digest phrases live before `weekly_digests` exists and are keyed by the
/// native digest target's dedupe hash. Missing rows mean a legacy fallback.
pub trait DigestPhraseStore: Send + Sync {
    fn digest_phrase(
        &self,
        dedupe_key: &str,
    ) -> impl Future<Output = Result<Option<String>, StoreError>> + Send;

    /// Insert unless a phrase exists; the first persisted phrase wins.
    fn save_digest_phrase(
        &self,
        dedupe_key: &str,
        phrase: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<String, StoreError>> + Send;
}
