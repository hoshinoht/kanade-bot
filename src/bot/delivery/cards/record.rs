//! What a reminder card was posted as, stored before its claim (keyed by
//! the send's native dedupe key) so a retry and every later edit render the
//! same card: its kind and, for a day-of card, the chosen heading line.

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
    /// The day-of heading without its `📅 ` and bold (`Today — Fri 25 Sep`).
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
