//! The notice outbox port: notices a decision asks for, written by the store
//! in that decision's transaction and drained by the delivery tick.
//!
//! A notice is keyed by its source (the change record or draft close that
//! wrote it) and its position there. The drain claims it through the journal
//! with that key ([`DeliveryJournal::claim_source`]), so exactly-once and
//! ambiguous-send handling stay the journal's; draining only records that
//! no further claim is needed.
//!
//! [`DeliveryJournal::claim_source`]: super::DeliveryJournal::claim_source

use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};

use super::journal::{JournalError, Lease};
use crate::schedule::Notice;

/// Parent decision 2026-09-25: a notice older than this when the drain
/// reaches it is retired as [`DrainReason::Stale`] instead of posted.
pub const DEFAULT_MAX_NOTICE_AGE: TimeDelta = TimeDelta::hours(6);

/// The source of the notices a change record's commit wrote.
pub fn change_source(seq: u64) -> String {
    format!("change:{seq}")
}

/// The source of the notices a draft's close wrote (a draft closes once).
pub fn draft_source(draft_id: &str) -> String {
    format!("draft:{draft_id}")
}

spelled! {
    /// Why a notice left the outbox.
    DrainReason {
        /// It went through the journal (posted, held, uncertain or rejected).
        Journal => "journal",
        /// Too old when reached; never claimed or posted.
        Stale => "stale",
        /// Nothing to post any more (its run or timing is gone); never claimed.
        Silent => "silent",
    }
}

/// One outbox row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxNotice {
    pub source: String,
    pub ordinal: i64,
    pub notice: Notice,
    pub created_at: DateTime<Utc>,
    /// Set once the drain is done with it; final.
    pub drained_at: Option<DateTime<Utc>>,
    /// `None` while pending (and for rows drained before migration 0013).
    pub drained_reason: Option<DrainReason>,
}

/// A pending row whose payload no longer decodes; it stays pending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndecodableNotice {
    pub source: String,
    pub ordinal: i64,
    pub detail: String,
}

/// A position in the outbox's write order: a keyset cursor for paging the
/// pending rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoticeCursor {
    pub id: i64,
}

/// Pending rows, decoded one by one, in write order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingNotices {
    pub notices: Vec<OutboxNotice>,
    pub undecodable: Vec<UndecodableNotice>,
    /// Where the next page starts (pass it as `after`); `None` once a read
    /// came back short of its limit: nothing further was pending.
    pub next: Option<NoticeCursor>,
}

pub trait NoticeOutbox {
    /// At most `limit` undrained notices written after `after` (from the
    /// first when `None`), in write order. A row that does not decode is
    /// reported in `undecodable` instead of failing the read, and counts
    /// toward `limit` and `next` like any other, so paging passes it.
    fn pending_notices_after(
        &self,
        after: Option<NoticeCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<PendingNotices, JournalError>> + Send;

    /// Every undrained notice in one read, as [`Self::pending_notices_after`]
    /// decodes them (checks and inspection; the drain pages instead, so a
    /// backlog is never held whole).
    fn pending_notices(&self) -> impl Future<Output = Result<PendingNotices, JournalError>> + Send;

    /// Every notice, drained or not, in write order (admin reads and checks).
    /// Any undecodable row fails the read.
    fn outbox_notices(
        &self,
    ) -> impl Future<Output = Result<Vec<OutboxNotice>, JournalError>> + Send;

    /// Under a live lease, mark a notice drained at `at` for `reason`.
    /// Draining a drained notice again is a no-op; an unknown one is
    /// [`JournalError::StateChanged`].
    fn mark_drained(
        &self,
        lease: &Lease,
        source: &str,
        ordinal: i64,
        reason: DrainReason,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;
}
