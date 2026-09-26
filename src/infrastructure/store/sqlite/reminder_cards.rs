//! `ReminderCardStore` over `reminder_cards` (migration 0014), joined to the
//! journal's bound attempts and card→run rows for refreshes.

use chrono::{DateTime, Utc};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use crate::bot::delivery::cards::{CardRecord, PostedCard, ReminderCardStore};
use crate::domain::scheduler::StoreError;

async fn record_in(
    conn: &mut SqliteConnection,
    dedupe_key: &str,
) -> Result<Option<CardRecord>, StoreError> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT kind, heading FROM reminder_cards WHERE dedupe_key = ?1")
            .bind(dedupe_key)
            .fetch_optional(&mut *conn)
            .await
            .map_err(store_error)?;
    Ok(row.map(|(kind, heading)| CardRecord { kind, heading }))
}

impl ReminderCardStore for SqliteStore {
    async fn card_record(&self, dedupe_key: &str) -> Result<Option<CardRecord>, StoreError> {
        read_txn!(self, tx, record_in(&mut tx, dedupe_key))
    }

    async fn save_card_record(
        &self,
        dedupe_key: &str,
        record: &CardRecord,
        at: DateTime<Utc>,
    ) -> Result<CardRecord, StoreError> {
        let at = instant(&at)?;
        write_txn!(self, tx, async {
            sqlx::query(
                // Not `OR IGNORE`, which would also swallow CHECK failures.
                "INSERT INTO reminder_cards (dedupe_key, kind, heading, created_at) \
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT (dedupe_key) DO NOTHING",
            )
            .bind(dedupe_key)
            .bind(&record.kind)
            .bind(&record.heading)
            .bind(&at)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            record_in(&mut tx, dedupe_key)
                .await?
                .ok_or_else(|| StoreError::Backend("reminder card record vanished".into()))
        })
    }

    async fn posted_cards(&self, run_id: &str) -> Result<Vec<PostedCard>, StoreError> {
        read_txn!(self, tx, async {
            let rows = sqlx::query(
                "SELECT a.attempt_id, a.channel_id, a.message_id, c.kind, c.heading \
                 FROM delivery_attempts a JOIN reminder_cards c ON c.dedupe_key = a.dedupe_key \
                 WHERE a.state = 'bound' AND a.effect_kind = 'reminder' \
                 AND a.attempt_id IN (SELECT attempt_id FROM delivery_card_runs WHERE run_id = ?1) \
                 ORDER BY a.message_id",
            )
            .bind(run_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            let mut cards = Vec::with_capacity(rows.len());
            for row in rows {
                let attempt: String = row.try_get("attempt_id").map_err(store_error)?;
                let run_ids: Vec<String> = sqlx::query_scalar(
                    "SELECT run_id FROM delivery_card_runs WHERE attempt_id = ?1 ORDER BY run_id",
                )
                .bind(&attempt)
                .fetch_all(&mut *tx)
                .await
                .map_err(store_error)?;
                cards.push(PostedCard {
                    channel_id: row.try_get("channel_id").map_err(store_error)?,
                    message_id: row.try_get("message_id").map_err(store_error)?,
                    run_ids,
                    record: CardRecord {
                        kind: row.try_get("kind").map_err(store_error)?,
                        heading: row.try_get("heading").map_err(store_error)?,
                    },
                });
            }
            Ok(cards)
        })
    }
}
