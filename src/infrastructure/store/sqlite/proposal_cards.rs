//! `ProposalCardStore` over `proposal_cards` (migration 0011). The message
//! binding is written by the journal's card bind (`journal/finalize.rs`).

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::drafts::instant as read_instant;
use super::rows::instant;
use super::schedule::store_error;
use crate::domain::proposals::{CardDetails, ProposalCardStore, StoredCard};
use crate::domain::scheduler::StoreError;

const COLUMNS: &str = "draft_id, channel_id, details, message_id, posted_at";

fn card_of(row: &SqliteRow) -> Result<StoredCard, StoreError> {
    let details: String = row.try_get("details").map_err(store_error)?;
    let details = serde_json::from_str(&details)
        .ok()
        .and_then(|value| CardDetails::from_json(&value))
        .ok_or_else(|| StoreError::Backend("proposal_cards.details is unreadable".into()))?;
    let posted_at: Option<String> = row.try_get("posted_at").map_err(store_error)?;
    Ok(StoredCard {
        proposal_id: row.try_get("draft_id").map_err(store_error)?,
        channel_id: row.try_get("channel_id").map_err(store_error)?,
        details,
        message_id: row.try_get("message_id").map_err(store_error)?,
        posted_at: posted_at
            .map(|at| read_instant(&at, "posted_at"))
            .transpose()?,
    })
}

async fn save_in(
    conn: &mut SqliteConnection,
    proposal_id: &str,
    channel_id: &str,
    details: &CardDetails,
    at: &DateTime<Utc>,
) -> Result<(), StoreError> {
    let text = details.to_json().to_string();
    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT channel_id, details FROM proposal_cards WHERE draft_id = ?1")
            .bind(proposal_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(store_error)?;
    if let Some((stored_channel, stored)) = existing {
        let same = stored_channel == channel_id
            && serde_json::from_str(&stored)
                .ok()
                .and_then(|value| CardDetails::from_json(&value))
                .as_ref()
                == Some(details);
        return if same {
            Ok(())
        } else {
            Err(StoreError::Constraint(format!(
                "proposal {proposal_id} already has another card"
            )))
        };
    }
    let known: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM draft_proposals WHERE draft_id = ?1")
            .bind(proposal_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(store_error)?;
    if known.is_none() {
        return Err(StoreError::Constraint(format!(
            "proposal {proposal_id} does not exist"
        )));
    }
    sqlx::query(
        "INSERT INTO proposal_cards (draft_id, channel_id, details, message_id, posted_at, \
         created_at) VALUES (?1, ?2, ?3, NULL, NULL, ?4)",
    )
    .bind(proposal_id)
    .bind(channel_id)
    .bind(text)
    .bind(instant(at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

impl ProposalCardStore for SqliteStore {
    async fn save_card(
        &self,
        proposal_id: &str,
        channel_id: &str,
        details: &CardDetails,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        write_txn!(
            self,
            tx,
            save_in(&mut tx, proposal_id, channel_id, details, &at)
        )
    }

    async fn load_cards(&self, proposal_ids: &[String]) -> Result<Vec<StoredCard>, StoreError> {
        read_txn!(self, tx, async {
            let mut cards = Vec::new();
            for id in proposal_ids {
                if let Some(row) = sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM proposal_cards WHERE draft_id = ?1"
                ))
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(store_error)?
                {
                    cards.push(card_of(&row)?);
                }
            }
            Ok(cards)
        })
    }

    async fn cards_on_message(&self, message_id: &str) -> Result<Vec<StoredCard>, StoreError> {
        read_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {COLUMNS} FROM proposal_cards WHERE message_id = ?1 ORDER BY draft_id"
            ))
            .bind(message_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(card_of)
            .collect()
        })
    }

    async fn unposted_cards(&self, channel_id: &str) -> Result<Vec<StoredCard>, StoreError> {
        read_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {COLUMNS} FROM proposal_cards c WHERE c.channel_id = ?1 \
                 AND c.message_id IS NULL AND EXISTS (SELECT 1 FROM drafts d \
                 WHERE d.id = c.draft_id AND d.status IN ('open', 'submitted')) \
                 ORDER BY c.created_at, c.draft_id"
            ))
            .bind(channel_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(card_of)
            .collect()
        })
    }
}
