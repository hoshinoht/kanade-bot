//! `DebugCardStore` over `debug_cards` (migration 0017); rows are written by
//! the journal's claim and bind (`journal/claim.rs`, `journal/finalize.rs`).

use chrono::{DateTime, Utc};
use sqlx::Connection;

use super::SqliteStore;
use super::drafts::instant as read_instant;
use super::rows::instant;
use super::schedule::store_error;
use crate::bot::delivery::{DebugCardStore, PostedDebugCard};
use crate::domain::scheduler::StoreError;

impl DebugCardStore for SqliteStore {
    async fn debug_cards_in(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<PostedDebugCard>, StoreError> {
        let since = instant(&since)?;
        read_txn!(self, tx, async {
            let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
                "SELECT channel_id, message_id, run_id, kind, posted_at FROM debug_cards \
                 WHERE channel_id = ?1 AND message_id IS NOT NULL AND cleared_at IS NULL \
                 AND posted_at >= ?2 ORDER BY posted_at, message_id",
            )
            .bind(channel_id)
            .bind(&since)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            rows.into_iter()
                .map(|(channel_id, message_id, run_id, kind, posted_at)| {
                    Ok(PostedDebugCard {
                        channel_id,
                        message_id,
                        run_id,
                        kind,
                        posted_at: read_instant(&posted_at, "posted_at")?,
                    })
                })
                .collect()
        })
    }

    async fn clear_debug_card(
        &self,
        message_id: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let at = instant(&at)?;
        write_txn!(self, tx, async {
            let changed = sqlx::query(
                "UPDATE debug_cards SET cleared_at = ?1 \
                 WHERE message_id = ?2 AND cleared_at IS NULL",
            )
            .bind(&at)
            .bind(message_id)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?
            .rows_affected();
            Ok(changed == 1)
        })
    }
}
