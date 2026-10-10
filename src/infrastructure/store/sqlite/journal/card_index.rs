//! Which runs a posted card is for: the card→run rows written when the card
//! was bound (they survive reminder rebuilds), plus reminders stamped with the
//! message id without a journal binding (v4 `reminders_by_message`).
//!
//! [`CardIndex`] passes only the message id; Discord message ids are unique
//! snowflakes, so the lookup is not channel-qualified.

use chrono::{DateTime, Utc};
use sqlx::Row;
use twilight_model::id::Id;
use twilight_model::id::marker::MessageMarker;

use super::super::SqliteStore;
use crate::bot::events::{CardIndex, LookupError, ReplayCard, ReplayCards, ReplayRunCards};
use crate::domain::time::{from_iso, to_iso};

impl CardIndex for SqliteStore {
    async fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> Result<Vec<String>, LookupError> {
        sqlx::query_scalar(
            "SELECT run_id FROM delivery_card_runs WHERE message_id = ?1
             UNION
             SELECT run_id FROM reminders WHERE message_id = ?1
             ORDER BY 1",
        )
        .bind(message.get().to_string())
        .fetch_all(&self.readers)
        .await
        .map_err(|error| LookupError(error.to_string()))
    }
}

impl ReplayCards for SqliteStore {
    async fn replay_cards(
        &self,
        not_before: DateTime<Utc>,
    ) -> Result<Vec<ReplayRunCards>, LookupError> {
        let not_before = to_iso(&not_before).map_err(|error| LookupError(error.to_string()))?;
        // Keep the live CardIndex union intact. The aggregation makes a shared
        // message one read even when it has several historic journal rows.
        let rows = sqlx::query(
            r"WITH mapped AS (
                SELECT d.run_id, d.message_id, a.channel_id, a.resolved_at,
                  CASE WHEN a.state = 'bound' AND a.resolved_at >= ?1
                        AND ((c.kind = 'day_of' OR c.kind GLOB 'countdown_*')
                             OR (g.cleared_at IS NULL AND (g.kind = 'day_of' OR g.kind GLOB 'countdown_*')))
                       THEN 1 ELSE 0 END AS evidence
                FROM delivery_card_runs d
                JOIN delivery_attempts a ON a.attempt_id = d.attempt_id
                LEFT JOIN reminder_cards c ON c.dedupe_key = a.dedupe_key
                LEFT JOIN debug_cards g ON g.attempt_id = a.attempt_id
                UNION ALL
                SELECT r.run_id, r.message_id, a.channel_id, a.resolved_at,
                  CASE WHEN a.state = 'bound' AND a.resolved_at >= ?1
                        AND c.kind IS NOT NULL THEN 1 ELSE 0 END AS evidence
                FROM reminders r
                LEFT JOIN delivery_attempts a ON a.message_id = r.message_id
                LEFT JOIN reminder_cards c ON c.dedupe_key = a.dedupe_key
                WHERE r.message_id IS NOT NULL
              ), grouped AS (
                SELECT run_id, message_id, MAX(channel_id) AS channel_id, MAX(evidence) AS evidence,
                  MAX(CASE WHEN evidence = 1 THEN resolved_at END) AS resolved_at
                FROM mapped GROUP BY run_id, message_id
              )
              SELECT run_id, message_id, channel_id, evidence, resolved_at FROM grouped
              WHERE run_id IN (SELECT run_id FROM grouped WHERE evidence = 1)
              ORDER BY run_id, message_id",
        )
        .bind(not_before)
        .fetch_all(&self.readers)
        .await
        .map_err(|error| LookupError(error.to_string()))?;
        let mut runs: Vec<ReplayRunCards> = Vec::new();
        for row in rows {
            let run_id: String = row
                .try_get("run_id")
                .map_err(|error| LookupError(error.to_string()))?;
            let card = ReplayCard {
                channel_id: row
                    .try_get("channel_id")
                    .map_err(|error| LookupError(error.to_string()))?,
                message_id: row
                    .try_get("message_id")
                    .map_err(|error| LookupError(error.to_string()))?,
                evidence: row
                    .try_get::<i64, _>("evidence")
                    .map_err(|error| LookupError(error.to_string()))?
                    != 0,
                resolved_at: row
                    .try_get::<Option<String>, _>("resolved_at")
                    .map_err(|error| LookupError(error.to_string()))?
                    .map(|at| from_iso(&at).map_err(|error| LookupError(error.to_string())))
                    .transpose()?,
            };
            match runs.last_mut() {
                Some(run) if run.run_id == run_id => run.cards.push(card),
                _ => runs.push(ReplayRunCards {
                    run_id,
                    cards: vec![card],
                }),
            }
        }
        Ok(runs)
    }
}
