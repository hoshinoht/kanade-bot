//! Admin answers composed from several tables are read in one SQLite
//! transaction. A concurrent write is either wholly included or wholly
//! excluded, so one response never mixes revisions (History head, total and
//! page with their settings saves; the Inbox count's proposals and requests).

use chrono::{DateTime, Utc};

use crate::domain::history::{ChangeFilter, ChangeRecord, ChangeRef};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{SettingsChange, SettingsChangeQuery};

/// One History page to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryViewQuery {
    pub filter: ChangeFilter,
    /// Records older than this seq (the page cursor).
    pub before: Option<u64>,
    pub limit: usize,
    /// The settings saves to read beside the records; `None` reads none.
    pub settings: Option<SettingsChangeQuery>,
}

/// One History page, read from one snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryView {
    pub head: ChangeRef,
    /// Records matching the filter, genesis never counted.
    pub total: u64,
    /// For a run filter: the run still has a row. `true` for other filters.
    pub run_known: bool,
    /// Newest first, older than `before`, genesis never included.
    pub records: Vec<ChangeRecord>,
    /// The seq to pass as `before` for the next page.
    pub next_before: Option<u64>,
    /// The settings saves matching the query, newest first.
    pub settings: Vec<SettingsChange>,
    /// When the `before` record was made; `None` without a cursor or when
    /// it names no record.
    pub cursor_at: Option<DateTime<Utc>>,
}

pub trait SnapshotReads {
    fn history_view(
        &self,
        query: HistoryViewQuery,
    ) -> impl Future<Output = Result<HistoryView, StoreError>> + Send;

    /// Live extractor proposals plus submitted member requests.
    fn inbox_count(&self) -> impl Future<Output = Result<u64, StoreError>> + Send;
}
