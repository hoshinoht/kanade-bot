//! In-memory [`SnapshotReads`]: each answer is read under one hold of the
//! tables lock (and the settings changes lock, taken after it), so no write
//! lands in the middle.

use super::settings::matching;
use super::{MemoryScheduleStore, count_in, head_in, list_in, snapshot};
use crate::domain::drafts::{DraftKind, DraftStatus};
use crate::domain::history::{ChangeFilter, ChangeQuery};
use crate::domain::scheduler::{Scope, StoreError};
use crate::infrastructure::store::snapshots::{HistoryView, HistoryViewQuery, SnapshotReads};

impl SnapshotReads for MemoryScheduleStore {
    async fn history_view(&self, query: HistoryViewQuery) -> Result<HistoryView, StoreError> {
        let tables = self.tables();
        let changes = self.changes();
        let head = head_in(&tables)?;
        let total = count_in(&tables, &query.filter);
        let run_known = match &query.filter {
            ChangeFilter::Run(run_id) => !snapshot(&tables, &Scope::Run(run_id.clone()))
                .runs
                .is_empty(),
            _ => true,
        };
        let mut page = ChangeQuery::new(query.filter.clone());
        page.newest_first = true;
        page.cursor = query.before;
        page.limit = query.limit;
        let found = list_in(&tables, &page);
        let records = found
            .records
            .into_iter()
            .filter(|record| record.seq > 0)
            .collect();
        // The next page may hold only genesis, which is never listed.
        let next_before = found.next_cursor.filter(|&cursor| {
            page.cursor = Some(cursor);
            page.limit = 1;
            list_in(&tables, &page)
                .records
                .iter()
                .any(|record| record.seq > 0)
        });
        let settings = query
            .settings
            .as_ref()
            .map(|settings| matching(&changes, settings))
            .unwrap_or_default();
        let cursor_at = query.before.and_then(|seq| {
            usize::try_from(seq)
                .ok()
                .and_then(|index| tables.history.records.get(index))
                .filter(|record| record.seq > 0)
                .map(|record| record.at)
        });
        Ok(HistoryView {
            head,
            total,
            run_known,
            records,
            next_before,
            settings,
            cursor_at,
        })
    }

    async fn inbox_count(&self) -> Result<u64, StoreError> {
        let tables = self.tables();
        let drafts = &tables.drafts;
        let proposals = drafts
            .proposals
            .keys()
            .filter(|id| {
                drafts
                    .drafts
                    .get(*id)
                    .is_some_and(|draft| draft.status.is_live())
            })
            .count();
        let requests = drafts
            .drafts
            .values()
            .filter(|draft| {
                draft.kind == DraftKind::Request && draft.status == DraftStatus::Submitted
            })
            .count();
        Ok(u64::try_from(proposals + requests).unwrap_or(u64::MAX))
    }
}
