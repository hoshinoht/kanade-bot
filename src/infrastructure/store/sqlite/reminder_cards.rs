//! `ReminderCardStore` over `reminder_cards` (migration 0014), joined to the
//! journal's bound attempts and card→run rows for refreshes; digest phrases
//! (0018) and manual header overrides (0028). Every heading or phrase read
//! returns the key's latest override, else the original line, with that
//! line's slot map (0043).

use chrono::{DateTime, Utc};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
#[cfg(any(test, feature = "test-support"))]
use crate::domain::notify::HeaderHistory;
use crate::domain::notify::{
    CardRecord, DigestPhraseStore, HeaderOverrideStore, HeaderSlots, PostedCard, ReminderCardStore,
};
use crate::domain::scheduler::StoreError;

/// The line in effect for a card (`c` = its `reminder_cards` row): the
/// latest override's line and slot map, else the card's own.
const IN_EFFECT: &str = "COALESCE((SELECT o.line FROM header_overrides o \
     WHERE o.dedupe_key = c.dedupe_key ORDER BY o.seq DESC LIMIT 1), c.heading) AS heading, \
     CASE WHEN EXISTS (SELECT 1 FROM header_overrides o WHERE o.dedupe_key = c.dedupe_key) \
     THEN (SELECT o.slots FROM header_overrides o WHERE o.dedupe_key = c.dedupe_key \
     ORDER BY o.seq DESC LIMIT 1) ELSE c.slots END AS slots";

/// A stored slot map. An unreadable one reads as none: its line then keeps
/// a placeholder, which `resolve_heading` never posts (the seed is used).
fn slots_of(text: Option<String>) -> Option<HeaderSlots> {
    serde_json::from_str(text.as_deref()?).ok()
}

fn slots_text(slots: Option<&HeaderSlots>) -> Result<Option<String>, StoreError> {
    slots
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| StoreError::Backend(format!("header slots: {error}")))
}

async fn record_in(
    conn: &mut SqliteConnection,
    dedupe_key: &str,
) -> Result<Option<CardRecord>, StoreError> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT c.kind, {IN_EFFECT} FROM reminder_cards c WHERE c.dedupe_key = ?1"
    ))
    .bind(dedupe_key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(row.map(|(kind, heading, slots)| CardRecord {
        kind,
        heading,
        slots: slots_of(slots),
    }))
}

async fn phrase_in(
    conn: &mut SqliteConnection,
    dedupe_key: &str,
) -> Result<Option<String>, StoreError> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT COALESCE(\
         (SELECT line FROM header_overrides WHERE dedupe_key = ?1 ORDER BY seq DESC LIMIT 1), \
         (SELECT phrase FROM digest_card_phrases WHERE dedupe_key = ?1))",
    )
    .bind(dedupe_key)
    .fetch_one(&mut *conn)
    .await
    .map_err(store_error)
}

impl ReminderCardStore for SqliteStore {
    async fn card_record(&self, dedupe_key: &str) -> Result<Option<CardRecord>, StoreError> {
        #[cfg(any(test, feature = "test-support"))]
        self.faults.card_record_read()?;
        read_txn!(self, tx, record_in(&mut tx, dedupe_key))
    }

    async fn save_card_record(
        &self,
        dedupe_key: &str,
        record: &CardRecord,
        at: DateTime<Utc>,
    ) -> Result<CardRecord, StoreError> {
        #[cfg(any(test, feature = "test-support"))]
        self.faults.card_record_write()?;
        let at = instant(&at)?;
        let slots = slots_text(record.slots.as_ref())?;
        write_txn!(self, tx, async {
            sqlx::query(
                // Not `OR IGNORE`, which would also swallow CHECK failures.
                "INSERT INTO reminder_cards (dedupe_key, kind, heading, created_at, slots) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (dedupe_key) DO NOTHING",
            )
            .bind(dedupe_key)
            .bind(&record.kind)
            .bind(&record.heading)
            .bind(&at)
            .bind(&slots)
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
            let rows = sqlx::query(&format!(
                "SELECT a.attempt_id, a.channel_id, a.message_id, c.dedupe_key, c.kind, \
                 {IN_EFFECT} \
                 FROM delivery_attempts a JOIN reminder_cards c ON c.dedupe_key = a.dedupe_key \
                 WHERE a.state = 'bound' AND a.effect_kind = 'reminder' \
                 AND a.attempt_id IN (SELECT attempt_id FROM delivery_card_runs WHERE run_id = ?1) \
                 ORDER BY a.message_id",
            ))
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
                        slots: slots_of(row.try_get("slots").map_err(store_error)?),
                    },
                    dedupe_key: Some(row.try_get("dedupe_key").map_err(store_error)?),
                    test: false,
                });
            }
            let tests: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT d.channel_id, d.message_id, d.kind FROM debug_cards d \
                 JOIN delivery_attempts a ON a.attempt_id = d.attempt_id \
                 WHERE d.run_id = ?1 AND a.state = 'bound' AND d.message_id IS NOT NULL \
                 AND d.cleared_at IS NULL AND (d.kind = 'day_of' OR d.kind GLOB 'countdown_*')",
            )
            .bind(run_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            cards.extend(
                tests
                    .into_iter()
                    .map(|(channel_id, message_id, kind)| PostedCard {
                        channel_id,
                        message_id,
                        run_ids: vec![run_id.to_owned()],
                        record: CardRecord {
                            kind,
                            heading: None,
                            slots: None,
                        },
                        dedupe_key: None,
                        test: true,
                    }),
            );
            cards.sort_by(|a, b| a.message_id.cmp(&b.message_id));
            Ok(cards)
        })
    }
}

impl DigestPhraseStore for SqliteStore {
    async fn digest_phrase(&self, dedupe_key: &str) -> Result<Option<String>, StoreError> {
        #[cfg(any(test, feature = "test-support"))]
        self.faults.digest_phrase_read()?;
        read_txn!(self, tx, phrase_in(&mut tx, dedupe_key))
    }

    async fn save_digest_phrase(
        &self,
        dedupe_key: &str,
        phrase: &str,
        at: DateTime<Utc>,
    ) -> Result<String, StoreError> {
        #[cfg(any(test, feature = "test-support"))]
        self.faults.digest_phrase_write()?;
        let at = instant(&at)?;
        write_txn!(self, tx, async {
            sqlx::query(
                "INSERT INTO digest_card_phrases (dedupe_key, phrase, created_at) \
                 VALUES (?1, ?2, ?3) ON CONFLICT (dedupe_key) DO NOTHING",
            )
            .bind(dedupe_key)
            .bind(phrase)
            .bind(&at)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            phrase_in(&mut tx, dedupe_key)
                .await?
                .ok_or_else(|| StoreError::Backend("digest card phrase vanished".into()))
        })
    }
}

impl HeaderOverrideStore for SqliteStore {
    async fn override_header(
        &self,
        dedupe_key: &str,
        line: &str,
        slots: Option<&HeaderSlots>,
        actor: &str,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let at = instant(&at)?;
        let slots = slots_text(slots)?;
        write_txn!(self, tx, async {
            sqlx::query(
                "INSERT INTO header_overrides (dedupe_key, line, actor, created_at, slots) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(dedupe_key)
            .bind(line)
            .bind(actor)
            .bind(&at)
            .bind(&slots)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(())
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn header_history(&self, dedupe_key: &str) -> Result<HeaderHistory, StoreError> {
        read_txn!(self, tx, async {
            let original: Option<String> = sqlx::query_scalar(
                "SELECT COALESCE(\
                 (SELECT heading FROM reminder_cards WHERE dedupe_key = ?1), \
                 (SELECT phrase FROM digest_card_phrases WHERE dedupe_key = ?1))",
            )
            .bind(dedupe_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(store_error)?;
            let overrides: Vec<(String, String)> = sqlx::query_as(
                "SELECT line, actor FROM header_overrides WHERE dedupe_key = ?1 ORDER BY seq",
            )
            .bind(dedupe_key)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(HeaderHistory {
                original,
                overrides,
            })
        })
    }
}
