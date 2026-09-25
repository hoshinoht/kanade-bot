//! [`DeliveryJournal`] over the `delivery_attempts` / `delivery_attempt_targets`
//! tables. Every write is one `BEGIN IMMEDIATE` transaction on the store's
//! write connection, so journal and schedule writes are serialised together.

mod card_index;
mod claim;
mod finalize;
mod outbox;
mod payload;
mod recover;
mod retire;

pub(super) use claim::UNPROVEN_REMINDER;
pub(super) use outbox::enqueue;

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use crate::domain::notify::{
    ActiveClaims, AttemptId, AttemptRecord, AttemptState, Claim, DIGEST_MARKER_KEY,
    DeliveryJournal, DeliveryTarget, DigestLog, DrainReason, JournalError, Lease, NoticeOutbox,
    NotificationIntent, OutboxNotice, PendingNotices, Receipt, Recovery, WeeklyDigest,
};
use crate::domain::time::{from_iso, to_iso};

fn backend(error: sqlx::Error) -> JournalError {
    JournalError::Backend(error.to_string())
}

fn state_changed(detail: impl Into<String>) -> JournalError {
    JournalError::StateChanged(detail.into())
}

fn iso(at: &DateTime<Utc>) -> Result<String, JournalError> {
    Ok(to_iso(at)?)
}

fn corrupt(detail: impl std::fmt::Display) -> JournalError {
    JournalError::Backend(format!("stored journal row is unreadable: {detail}"))
}

/// `(binding_type, key_primary)`; reminders and digests have no secondary key.
fn encode(target: &DeliveryTarget) -> Result<(&'static str, String), JournalError> {
    Ok((target.binding_type(), target.key_primary()?))
}

/// Target families this journal plans; others (decline, debug) are ignored.
fn decode(binding_type: &str, key: &str) -> Result<Option<DeliveryTarget>, JournalError> {
    Ok(match binding_type {
        "reminder" => Some(DeliveryTarget::Reminder(key.to_owned())),
        "digest" => Some(DeliveryTarget::Digest(from_iso(key).map_err(corrupt)?)),
        "card" => Some(DeliveryTarget::Card(key.to_owned())),
        _ => None,
    })
}

/// Run `$body` (which may use `&mut *$tx`) in one write transaction; commit
/// on `Ok`, roll back on `Err`, and orphan the connection if either fails.
macro_rules! write_tx {
    ($store:expr, $tx:ident => $body:expr) => {{
        let mut lease = $store
            .writer
            .lease()
            .await
            .map_err(|error| JournalError::Backend(error.to_string()))?;
        let (result, healthy) = match lease.conn().begin_with("BEGIN IMMEDIATE").await {
            Ok(mut $tx) => match $body.await {
                Ok(value) => match $tx.commit().await {
                    Ok(()) => (Ok(value), true),
                    Err(error) => (Err(backend(error)), false),
                },
                Err(error) => {
                    let healthy = $tx.rollback().await.is_ok();
                    (Err(error), healthy)
                }
            },
            Err(error) => (Err(backend(error)), false),
        };
        lease.finish(healthy);
        result
    }};
}

/// The lease exists, is live, is this lease's, and the store is OPEN.
async fn check_live(tx: &mut SqliteConnection, lease: &Lease) -> Result<(), JournalError> {
    let live = sqlx::query(
        "SELECT 1 FROM maintenance_leases l JOIN maintenance_state m ON m.id = 1
         WHERE l.operation_id = ?1 AND l.owner_token_hash = ?2 AND l.lifecycle = 'live'
           AND l.generation = m.generation AND m.mode = 'OPEN'",
    )
    .bind(&lease.operation_id)
    .bind(lease.token_hash())
    .fetch_optional(tx)
    .await
    .map_err(backend)?;
    live.map(|_| ()).ok_or(JournalError::LeaseNotLive)
}

async fn bump_revision(tx: &mut SqliteConnection) -> Result<(), JournalError> {
    sqlx::query("UPDATE store_meta SET revision = revision + 1 WHERE id = 1")
        .execute(tx)
        .await
        .map_err(backend)?;
    Ok(())
}

/// v4 `raise_digest_marker`: never backwards, never to a future week.
async fn raise_marker(
    tx: &mut SqliteConnection,
    week: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    if week > at {
        return Ok(());
    }
    let current: Option<String> = sqlx::query_scalar("SELECT value FROM config WHERE key = ?1")
        .bind(DIGEST_MARKER_KEY)
        .fetch_optional(&mut *tx)
        .await
        .map_err(backend)?;
    if current
        .as_deref()
        .is_some_and(|marker| from_iso(marker).is_ok_and(|marker| marker >= week))
    {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(DIGEST_MARKER_KEY)
    .bind(iso(&week)?)
    .execute(tx)
    .await
    .map_err(backend)?;
    Ok(())
}

/// An attempt's targets in ordinal order, with whether each is released.
async fn attempt_targets(
    tx: &mut SqliteConnection,
    attempt: &AttemptId,
) -> Result<Vec<(DeliveryTarget, bool)>, JournalError> {
    let rows = sqlx::query(
        "SELECT binding_type, key_primary, released_at IS NOT NULL AS released
         FROM delivery_attempt_targets WHERE attempt_id = ?1 ORDER BY target_ordinal",
    )
    .bind(&attempt.0)
    .fetch_all(tx)
    .await
    .map_err(backend)?;
    let mut targets = Vec::new();
    for row in rows {
        let kind: String = row.try_get("binding_type").map_err(corrupt)?;
        let key: String = row.try_get("key_primary").map_err(corrupt)?;
        let released: bool = row.try_get("released").map_err(corrupt)?;
        if let Some(target) = decode(&kind, &key)? {
            targets.push((target, released));
        }
    }
    Ok(targets)
}

/// An attempt's state, or `StateChanged` when it does not exist.
async fn attempt_state(
    tx: &mut SqliteConnection,
    attempt: &AttemptId,
) -> Result<(AttemptState, bool), JournalError> {
    let row =
        sqlx::query("SELECT state, dedupe_active FROM delivery_attempts WHERE attempt_id = ?1")
            .bind(&attempt.0)
            .fetch_optional(tx)
            .await
            .map_err(backend)?
            .ok_or_else(|| state_changed(format!("attempt {attempt} does not exist")))?;
    let state: String = row.try_get("state").map_err(corrupt)?;
    let active: bool = row.try_get("dedupe_active").map_err(corrupt)?;
    let state = AttemptState::parse(&state).ok_or_else(|| corrupt(format!("state {state}")))?;
    Ok((state, active))
}

async fn require_intent(
    tx: &mut SqliteConnection,
    attempt: &AttemptId,
) -> Result<(), JournalError> {
    match attempt_state(tx, attempt).await? {
        (AttemptState::Intent, _) => Ok(()),
        (state, _) => Err(state_changed(format!(
            "attempt {attempt} is {}",
            state.as_str()
        ))),
    }
}

/// Mark each unsent reminder handled without a message (v4's skip shape) and
/// raise the digest marker, so a retired send is never retried.
async fn suppress_natives(
    tx: &mut SqliteConnection,
    targets: &[(DeliveryTarget, bool)],
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    for (target, _) in targets {
        match target {
            DeliveryTarget::Reminder(id) => {
                let bound: Option<Option<String>> =
                    sqlx::query_scalar("SELECT message_id FROM reminders WHERE id = ?1")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(backend)?;
                if matches!(bound, Some(Some(_))) {
                    return Err(state_changed(format!(
                        "reminder {id} is bound to another message"
                    )));
                }
                sqlx::query(
                    "UPDATE reminders SET sent_at = ?1
                     WHERE id = ?2 AND sent_at IS NULL AND message_id IS NULL",
                )
                .bind(iso(&at)?)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
            }
            DeliveryTarget::Digest(week) => raise_marker(tx, *week, at).await?,
            // A refused card stays unposted; the next pass in its channel
            // may claim it again.
            DeliveryTarget::Card(_) => {}
        }
    }
    Ok(())
}

/// Release every target and retire the attempt with `actor` and `reason`.
async fn release_and_retire(
    tx: &mut SqliteConnection,
    attempt: &AttemptId,
    actor: &str,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    let at = iso(&at)?;
    sqlx::query(
        "UPDATE delivery_attempt_targets SET released_at = ?1, release_actor = ?2,
         release_reason = ?3 WHERE attempt_id = ?4 AND released_at IS NULL",
    )
    .bind(&at)
    .bind(actor)
    .bind(reason)
    .bind(&attempt.0)
    .execute(&mut *tx)
    .await
    .map_err(backend)?;
    let retired = sqlx::query(
        "UPDATE delivery_attempts SET state = 'retired', dedupe_active = 0, resolved_at = ?1,
         resolved_by = ?2, resolution_reason = ?3 WHERE attempt_id = ?4 AND dedupe_active = 1",
    )
    .bind(&at)
    .bind(actor)
    .bind(reason)
    .bind(&attempt.0)
    .execute(tx)
    .await
    .map_err(backend)?
    .rows_affected();
    if retired != 1 {
        return Err(state_changed(format!(
            "attempt {attempt} was not retired once"
        )));
    }
    Ok(())
}

impl DeliveryJournal for SqliteStore {
    async fn load_view(&self) -> Result<ActiveClaims, JournalError> {
        let rows = sqlx::query(
            "SELECT t.binding_type, t.key_primary FROM delivery_attempt_targets t
             JOIN delivery_attempts a USING (attempt_id)
             WHERE t.released_at IS NULL AND a.dedupe_active = 1
               AND a.state IN ('intent', 'indeterminate')",
        )
        .fetch_all(&self.readers)
        .await
        .map_err(backend)?;
        let mut held = BTreeSet::new();
        for row in rows {
            let kind: String = row.try_get("binding_type").map_err(corrupt)?;
            let key: String = row.try_get("key_primary").map_err(corrupt)?;
            held.extend(decode(&kind, &key)?);
        }
        Ok(ActiveClaims::new(held))
    }

    async fn load_attempt(
        &self,
        attempt: &AttemptId,
    ) -> Result<Option<AttemptRecord>, JournalError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        let Some(row) = sqlx::query(
            "SELECT state, channel_id, message_id, resolved_by, resolution_reason
             FROM delivery_attempts WHERE attempt_id = ?1",
        )
        .bind(&attempt.0)
        .fetch_optional(&mut *conn)
        .await
        .map_err(backend)?
        else {
            return Ok(None);
        };
        let state: String = row.try_get("state").map_err(corrupt)?;
        let targets = attempt_targets(&mut conn, attempt).await?;
        Ok(Some(AttemptRecord {
            id: attempt.clone(),
            state: AttemptState::parse(&state).ok_or_else(|| corrupt(format!("state {state}")))?,
            channel_id: row
                .try_get::<Option<String>, _>("channel_id")
                .map_err(corrupt)?
                .unwrap_or_default(),
            message_id: row.try_get("message_id").map_err(corrupt)?,
            resolved_by: row.try_get("resolved_by").map_err(corrupt)?,
            resolution_reason: row.try_get("resolution_reason").map_err(corrupt)?,
            targets: targets.into_iter().map(|(target, _)| target).collect(),
        }))
    }

    async fn load_digests(&self) -> Result<DigestLog, JournalError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        let last_digest_week = sqlx::query_scalar("SELECT value FROM config WHERE key = ?1")
            .bind(DIGEST_MARKER_KEY)
            .fetch_optional(&mut *conn)
            .await
            .map_err(backend)?;
        let rows = sqlx::query(
            "SELECT week_start, channel_id, message_id, posted_at, retired_at
             FROM weekly_digests ORDER BY week_start",
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(backend)?;
        let instant =
            |row: &sqlx::sqlite::SqliteRow, column: &str| -> Result<DateTime<Utc>, JournalError> {
                let text: String = row.try_get(column).map_err(corrupt)?;
                from_iso(&text).map_err(corrupt)
            };
        let mut digests = Vec::new();
        for row in &rows {
            let retired: Option<String> = row.try_get("retired_at").map_err(corrupt)?;
            digests.push(WeeklyDigest {
                week_start: instant(row, "week_start")?,
                channel_id: row.try_get("channel_id").map_err(corrupt)?,
                message_id: row.try_get("message_id").map_err(corrupt)?,
                posted_at: instant(row, "posted_at")?,
                retired_at: retired
                    .map(|text| from_iso(&text).map_err(corrupt))
                    .transpose()?,
            });
        }
        digests.sort_by_key(|row| row.week_start);
        Ok(DigestLog {
            last_digest_week,
            digests,
        })
    }

    async fn begin_lease(
        &self,
        instance_id: &str,
        operation_kind: &str,
        at: DateTime<Utc>,
    ) -> Result<Lease, JournalError> {
        write_tx!(self, tx => recover::begin_lease(&mut tx, instance_id, operation_kind, at))
    }

    async fn end_lease(&self, lease: &Lease, at: DateTime<Utc>) -> Result<(), JournalError> {
        write_tx!(self, tx => recover::end_lease(&mut tx, lease, at))
    }

    async fn claim(
        &self,
        lease: &Lease,
        intent: &NotificationIntent,
        effect_ordinal: Option<i64>,
        at: DateTime<Utc>,
    ) -> Result<Claim, JournalError> {
        write_tx!(self, tx => claim::claim(&mut tx, lease, intent, effect_ordinal, at))
    }

    async fn claim_source(
        &self,
        lease: &Lease,
        intent: &NotificationIntent,
        source: &str,
        ordinal: i64,
        at: DateTime<Utc>,
    ) -> Result<Claim, JournalError> {
        write_tx!(self, tx => claim::claim_source(&mut tx, lease, intent, source, ordinal, at))
    }

    async fn bind(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        receipt: &Receipt,
        record_week: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => finalize::bind(&mut tx, lease, attempt, receipt, record_week, at))
    }

    async fn mark_indeterminate(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => finalize::mark_indeterminate(&mut tx, lease, attempt))
    }

    async fn retire_rejected(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => retire::rejected(&mut tx, lease, attempt, reason, at))
    }

    async fn retire_for_replacement(
        &self,
        lease: &Lease,
        digest: &WeeklyDigest,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => retire::for_replacement(&mut tx, lease, digest, at))
    }

    async fn retire_unproven(
        &self,
        attempt: &AttemptId,
        actor: &str,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => retire::unproven(&mut tx, attempt, actor, reason, at))
    }

    async fn release_unsent(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => retire::release_unsent(&mut tx, lease, attempt, reason, at))
    }

    async fn record_digest_week(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => finalize::record_digest_week(&mut tx, lease, week, at))
    }

    async fn retire_digests_before(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<usize, JournalError> {
        write_tx!(self, tx => retire::digests_before(&mut tx, lease, week, at))
    }

    async fn recover_on_start(&self, at: DateTime<Utc>) -> Result<Recovery, JournalError> {
        write_tx!(self, tx => recover::on_start(&mut tx, at))
    }
}

impl NoticeOutbox for SqliteStore {
    async fn pending_notices(&self) -> Result<PendingNotices, JournalError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        outbox::pending(&mut conn).await
    }

    async fn outbox_notices(&self) -> Result<Vec<OutboxNotice>, JournalError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        outbox::all(&mut conn).await
    }

    async fn mark_drained(
        &self,
        lease: &Lease,
        source: &str,
        ordinal: i64,
        reason: DrainReason,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        write_tx!(self, tx => outbox::mark_drained(&mut tx, lease, source, ordinal, reason, at))
    }
}
