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

use chrono::{DateTime, Utc};

use super::journal::{JournalError, Lease};
use crate::domain::schedule::Notice;

/// The source of the notices a change record's commit wrote.
pub fn change_source(seq: u64) -> String {
    format!("change:{seq}")
}

/// The source of the notices a draft's close wrote (a draft closes once).
pub fn draft_source(draft_id: &str) -> String {
    format!("draft:{draft_id}")
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
}

pub trait NoticeOutbox {
    /// Undrained notices in write order.
    fn pending_notices(
        &self,
    ) -> impl Future<Output = Result<Vec<OutboxNotice>, JournalError>> + Send;

    /// Every notice, drained or not, in write order (admin reads and checks).
    fn outbox_notices(
        &self,
    ) -> impl Future<Output = Result<Vec<OutboxNotice>, JournalError>> + Send;

    /// Under a live lease, mark a notice drained at `at`. Draining a drained
    /// notice again is a no-op; an unknown one is [`JournalError::StateChanged`].
    fn mark_drained(
        &self,
        lease: &Lease,
        source: &str,
        ordinal: i64,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;
}
