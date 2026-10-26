//! What a reminder card was posted as, stored before its claim (keyed by
//! the send's native dedupe key) so a retry and every later edit render the
//! same card: its kind and saved header (day-of heading or short phrase).
//! A manual rewrite appends an override (migration 0028): every read of a
//! record or digest phrase returns the latest override instead of the
//! original line, which is kept. A line written with estimate context keeps
//! its `{cN}` placeholders and carries its slot map (migration 0043); only
//! the cards' `resolve_heading` turns it into the posted line.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scheduler::StoreError;

/// The record kind of a day-of card; countdowns use their reminder kind
/// (`countdown_<minutes>`).
pub const DAY_OF_KIND: &str = "day_of";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRecord {
    /// `day_of` or `countdown_<minutes>`.
    pub kind: String,
    /// Day-of heading without framing, or the short countdown phrase; the
    /// latest manual override when one exists. May hold `{cN}`: read it
    /// through the cards' `resolve_heading`.
    pub heading: Option<String>,
    /// The slot map stored with `heading` (the override's with an override).
    pub slots: Option<HeaderSlots>,
}

/// One filled placeholder of a stored line (migration 0043).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredSlot {
    /// `{c1}`.
    pub slot: String,
    pub member_id: String,
    pub character_id: String,
    pub ign: String,
    /// A tease slot (D11): posted only while teasing is on and the member
    /// still opts in.
    pub tease: bool,
}

/// The slot map stored beside a header line written with estimate context:
/// the placeholders the line uses and whether context was sent, so every
/// render can re-check the switch and the party before filling (D6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderSlots {
    pub slots: Vec<StoredSlot>,
    pub used_estimates: bool,
}

/// A bound reminder card with a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostedCard {
    pub channel_id: String,
    pub message_id: String,
    /// Every run the card is for (its card→run rows).
    pub run_ids: Vec<String>,
    pub record: CardRecord,
    /// The record's dedupe key; `None` for a test card.
    pub dedupe_key: Option<String>,
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

/// Manual header rewrites (migration 0028 `header_overrides`), keyed by a
/// reminder card's or a digest's native dedupe key. Insert-only: the latest
/// override of a key is what [`ReminderCardStore`] and [`DigestPhraseStore`]
/// read back; the original line stays as history.
pub trait HeaderOverrideStore: Send + Sync {
    /// Append `line` (non-blank, at most 1 KiB) for `dedupe_key` by `actor`,
    /// with the slot map of a line written with estimate context.
    fn override_header(
        &self,
        dedupe_key: &str,
        line: &str,
        slots: Option<&HeaderSlots>,
        actor: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The original line and every override, oldest first.
    #[cfg(any(test, feature = "test-support"))]
    fn header_history(
        &self,
        dedupe_key: &str,
    ) -> impl Future<Output = Result<HeaderHistory, StoreError>> + Send;
}

/// A key's original line (record heading or digest phrase) and its overrides.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderHistory {
    pub original: Option<String>,
    /// `(line, actor)`, oldest first; the last one is in effect.
    pub overrides: Vec<(String, String)>,
}
