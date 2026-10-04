//! Reading and verifying the change history.

use std::future::Future;

use chrono::{DateTime, Utc};

use super::origin::Actor;
use super::record::{ChangeRecord, ChangeRef, GENESIS_PREV_HASH};
use crate::domain::scheduler::StoreError;

/// The largest page a list query returns.
pub const MAX_PAGE: usize = 200;

/// Which records a list query selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeFilter {
    All,
    /// Records touching this boss week.
    Week(DateTime<Utc>),
    /// Records by this actor.
    Actor(Actor),
    /// Records by this actor touching this boss week (the history page with
    /// both filters).
    ActorInWeek(Actor, DateTime<Utc>),
    /// Records touching this run (see [`ChangeRecord::touches_run`]): a run's
    /// change log.
    Run(String),
    /// Records whose store revision is within `from..=to`.
    Revisions {
        from: u64,
        to: u64,
    },
}

/// One page of records in `seq` order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeQuery {
    pub filter: ChangeFilter,
    /// Continue after (or, newest first, before) this `seq`.
    pub cursor: Option<u64>,
    /// At most [`MAX_PAGE`].
    pub limit: usize,
    pub newest_first: bool,
}

impl ChangeQuery {
    pub fn new(filter: ChangeFilter) -> Self {
        Self {
            filter,
            cursor: None,
            limit: MAX_PAGE,
            newest_first: false,
        }
    }

    /// Whether `record` passes the filter and lies beyond the cursor.
    pub fn selects(&self, record: &ChangeRecord) -> bool {
        let beyond = match self.cursor {
            None => true,
            Some(cursor) if self.newest_first => record.seq < cursor,
            Some(cursor) => record.seq > cursor,
        };
        beyond
            && match &self.filter {
                ChangeFilter::All => true,
                ChangeFilter::Week(week) => record.touches_week(*week),
                ChangeFilter::Actor(actor) => &record.origin.actor == actor,
                ChangeFilter::ActorInWeek(actor, week) => {
                    &record.origin.actor == actor && record.touches_week(*week)
                }
                ChangeFilter::Run(run_id) => record.touches_run(run_id),
                ChangeFilter::Revisions { from, to } => (*from..=*to).contains(&record.revision),
            }
    }

    pub fn page_size(&self) -> usize {
        self.limit.clamp(1, MAX_PAGE)
    }
}

/// A page and the cursor for the next one (`None` at the end).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangePage {
    pub records: Vec<ChangeRecord>,
    pub next_cursor: Option<u64>,
}

impl ChangePage {
    /// Cut an over-fetched (`page_size + 1`) run of matching records.
    pub fn from_matches(mut records: Vec<ChangeRecord>, query: &ChangeQuery) -> Self {
        let size = query.page_size();
        let more = records.len() > size;
        records.truncate(size);
        let next_cursor = if more {
            records.last().map(|record| record.seq)
        } else {
            None
        };
        Self {
            records,
            next_cursor,
        }
    }
}

/// Why a rollback refused the records it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRefusal {
    /// No record has this position (or it is the genesis record).
    UnknownChange(u64),
    /// The record no longer matches its hash.
    Tampered(u64),
    /// The selection matched no change.
    NothingToRevert,
    /// No checkpoint has this name.
    UnknownCheckpoint(String),
}

impl std::fmt::Display for HistoryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownChange(seq) => write!(f, "no change {seq} to revert"),
            Self::Tampered(seq) => write!(f, "change {seq} does not match its hash"),
            Self::NothingToRevert => f.write_str("no change matches; nothing to revert"),
            Self::UnknownCheckpoint(name) => write!(f, "no checkpoint named {name:?}"),
        }
    }
}

/// Where the chain first fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokenLink {
    pub seq: u64,
    pub reason: String,
}

/// The result of re-checking every record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryVerification {
    pub records: u64,
    pub head: Option<ChangeRef>,
    pub first_broken: Option<BrokenLink>,
}

impl HistoryVerification {
    pub fn is_intact(&self) -> bool {
        self.first_broken.is_none()
    }
}

/// A record as read from storage: the parsed record and its stored bytes.
pub type StoredRecord = Result<(ChangeRecord, String), (u64, String)>;

/// Recompute the chain: contiguous `seq` from 0; each record's stored bytes
/// hash to its stored hash and are exactly its canonical encoding; each
/// `prev_hash` equals the previous hash. `Err((seq, reason))` entries are
/// records the store could not read or found inconsistent with their indexed
/// columns.
pub fn verify_chain(records: impl IntoIterator<Item = StoredRecord>) -> HistoryVerification {
    let mut result = HistoryVerification::default();
    let mut previous_hash = GENESIS_PREV_HASH.to_owned();
    for (expected_seq, entry) in (0u64..).zip(records) {
        let broken = |seq, reason: String| Some(BrokenLink { seq, reason });
        let (record, body) = match entry {
            Ok(stored) => stored,
            Err((seq, reason)) => {
                result.first_broken = broken(seq, reason);
                return result;
            }
        };
        let seq = record.seq;
        result.first_broken = if seq != expected_seq {
            broken(seq, format!("expected seq {expected_seq}"))
        } else if record.prev_hash != previous_hash {
            broken(seq, "prev_hash does not match the previous record".into())
        } else {
            record
                .check_stored(&body)
                .err()
                .and_then(|reason| broken(seq, reason))
        };
        if result.first_broken.is_some() {
            return result;
        }
        result.records += 1;
        previous_hash.clone_from(&record.hash);
        result.head = Some(record.reference());
    }
    result
}

/// A record loaded for a rollback, with its stored bytes checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckedChange {
    Missing,
    Intact(Box<ChangeRecord>),
    /// Its stored bytes, hash or indexed columns disagree.
    Tampered(String),
}

/// Read access to the append-only change history. There is deliberately no
/// way to update or delete a record.
pub trait ChangeHistory {
    fn load_change(
        &self,
        seq: u64,
    ) -> impl Future<Output = Result<Option<ChangeRecord>, StoreError>> + Send;

    /// Load one record and check its stored bytes (as [`verify_chain`] does
    /// for every record).
    fn load_checked(
        &self,
        seq: u64,
    ) -> impl Future<Output = Result<CheckedChange, StoreError>> + Send;

    fn list_changes(
        &self,
        query: &ChangeQuery,
    ) -> impl Future<Output = Result<ChangePage, StoreError>> + Send;

    /// How many records other than genesis `filter` selects (a history
    /// page's total).
    fn count_changes(
        &self,
        filter: &ChangeFilter,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Recompute the whole chain and report the first broken link.
    fn verify_history(
        &self,
    ) -> impl Future<Output = Result<HistoryVerification, StoreError>> + Send;

    /// The newest record, for backup manifests and periodic anchor logs.
    fn history_head(&self) -> impl Future<Output = Result<ChangeRef, StoreError>> + Send;

    /// Whether the chain still holds the anchored record with that hash; a
    /// history truncated behind an externally kept anchor fails this.
    fn contains_anchor(
        &self,
        anchor: &ChangeRef,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send
    where
        Self: Sync,
    {
        async move {
            Ok(self
                .load_change(anchor.seq)
                .await?
                .is_some_and(|record| record.hash == anchor.hash))
        }
    }
}

impl<T: ChangeHistory + Sync> ChangeHistory for &T {
    fn load_change(
        &self,
        seq: u64,
    ) -> impl Future<Output = Result<Option<ChangeRecord>, StoreError>> + Send {
        (**self).load_change(seq)
    }

    fn load_checked(
        &self,
        seq: u64,
    ) -> impl Future<Output = Result<CheckedChange, StoreError>> + Send {
        (**self).load_checked(seq)
    }

    fn list_changes(
        &self,
        query: &ChangeQuery,
    ) -> impl Future<Output = Result<ChangePage, StoreError>> + Send {
        (**self).list_changes(query)
    }

    fn count_changes(
        &self,
        filter: &ChangeFilter,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send {
        (**self).count_changes(filter)
    }

    fn verify_history(
        &self,
    ) -> impl Future<Output = Result<HistoryVerification, StoreError>> + Send {
        (**self).verify_history()
    }

    fn history_head(&self) -> impl Future<Output = Result<ChangeRef, StoreError>> + Send {
        (**self).history_head()
    }
}

/// A shared store (the API's writer holds one `Arc` beside its readers).
impl<T: ChangeHistory + Send + Sync> ChangeHistory for std::sync::Arc<T> {
    fn load_change(
        &self,
        seq: u64,
    ) -> impl Future<Output = Result<Option<ChangeRecord>, StoreError>> + Send {
        (**self).load_change(seq)
    }

    fn load_checked(
        &self,
        seq: u64,
    ) -> impl Future<Output = Result<CheckedChange, StoreError>> + Send {
        (**self).load_checked(seq)
    }

    fn list_changes(
        &self,
        query: &ChangeQuery,
    ) -> impl Future<Output = Result<ChangePage, StoreError>> + Send {
        (**self).list_changes(query)
    }

    fn count_changes(
        &self,
        filter: &ChangeFilter,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send {
        (**self).count_changes(filter)
    }

    fn verify_history(
        &self,
    ) -> impl Future<Output = Result<HistoryVerification, StoreError>> + Send {
        (**self).verify_history()
    }

    fn history_head(&self) -> impl Future<Output = Result<ChangeRef, StoreError>> + Send {
        (**self).history_head()
    }
}
