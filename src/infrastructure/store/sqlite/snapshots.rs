//! [`SnapshotReads`] over one read transaction each: WAL fixes the
//! snapshot at the first statement, so a commit that lands between the
//! statements is in none of them.

use sqlx::Connection;

use super::schedule::store_error;
use super::{SqliteStore, history, schedule, settings};
use crate::infrastructure::store::snapshots::{HistoryView, HistoryViewQuery, SnapshotReads};
use kanade_domain::history::{ChangeFilter, ChangeQuery};
use kanade_domain::scheduler::{Scope, StoreError};

/// Test support: a composed read stopped after its first statement (its
/// snapshot taken). `reached` fires there; the read goes on once `resume`
/// is sent or dropped.
#[cfg(any(test, feature = "test-support"))]
pub struct ReadPause {
    pub reached: tokio::sync::oneshot::Receiver<()>,
    pub resume: tokio::sync::oneshot::Sender<()>,
}

/// The armed pause, taken by the next composed read.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub(super) struct PauseSlot(
    std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
);

impl SqliteStore {
    /// Test support: the next composed read ([`SnapshotReads`]) stops
    /// after its first statement until the pause is resumed, so a test can
    /// commit a write in the middle of it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pause_next_snapshot_read(&self) -> ReadPause {
        let (reached_tx, reached) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        *self
            .read_pause
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((reached_tx, resume_rx));
        ReadPause { reached, resume }
    }

    async fn paused(&self) {
        #[cfg(any(test, feature = "test-support"))]
        {
            let armed = self
                .read_pause
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some((reached, resume)) = armed {
                let _ = reached.send(());
                let _ = resume.await;
            }
        }
    }
}

impl SnapshotReads for SqliteStore {
    async fn history_view(&self, query: HistoryViewQuery) -> Result<HistoryView, StoreError> {
        read_txn!(self, tx, async {
            let head = history::head_ref(&mut tx).await?;
            self.paused().await;
            let total = history::count_in(&mut tx, &query.filter).await?;
            let run_known = match &query.filter {
                ChangeFilter::Run(run_id) => !schedule::load(&mut tx, &Scope::Run(run_id.clone()))
                    .await?
                    .runs
                    .is_empty(),
                _ => true,
            };
            let mut page = ChangeQuery::new(query.filter.clone());
            page.newest_first = true;
            page.cursor = query.before;
            page.limit = query.limit;
            let found = history::list_in(&mut tx, &page).await?;
            let records = found
                .records
                .into_iter()
                .filter(|record| record.seq > 0)
                .collect();
            // The next page may hold only genesis, which is never listed.
            let next_before = match found.next_cursor {
                Some(cursor) => {
                    page.cursor = Some(cursor);
                    page.limit = 1;
                    history::list_in(&mut tx, &page)
                        .await?
                        .records
                        .iter()
                        .any(|record| record.seq > 0)
                        .then_some(cursor)
                }
                None => None,
            };
            let settings = match &query.settings {
                Some(settings) => settings::list(&mut tx, settings).await?,
                None => Vec::new(),
            };
            let cursor_at = match query.before {
                Some(seq) => history::change_in(&mut tx, seq)
                    .await?
                    .filter(|record| record.seq > 0)
                    .map(|record| record.at),
                None => None,
            };
            Ok(HistoryView {
                head,
                total,
                run_known,
                records,
                next_before,
                settings,
                cursor_at,
            })
        })
    }

    async fn inbox_count(&self) -> Result<u64, StoreError> {
        read_txn!(self, tx, async {
            let proposals: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM drafts JOIN draft_proposals p ON p.draft_id = drafts.id \
                 WHERE drafts.status IN ('open', 'submitted')",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(store_error)?;
            self.paused().await;
            let requests: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM drafts WHERE kind = 'request' AND status = 'submitted'",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(store_error)?;
            u64::try_from(proposals + requests)
                .map_err(|error| StoreError::Backend(error.to_string()))
        })
    }
}
