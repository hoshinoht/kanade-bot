//! Binding a delivered attempt to its native rows (v4 `_finalize`), or
//! recording that its outcome is unknown.

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;

use super::{
    attempt_targets, backend, bump_revision, check_live, iso, raise_marker, require_intent,
    state_changed,
};
use crate::domain::notify::{AttemptId, DeliveryTarget, JournalError, Lease, Receipt};

pub(super) async fn bind(
    tx: &mut SqliteConnection,
    lease: &Lease,
    attempt: &AttemptId,
    receipt: &Receipt,
    record_week: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_live(tx, lease).await?;
    require_intent(tx, attempt).await?;
    let claimed: Option<String> =
        sqlx::query_scalar("SELECT channel_id FROM delivery_attempts WHERE attempt_id = ?1")
            .bind(&attempt.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(backend)?;
    if claimed.as_deref() != Some(receipt.channel_id.as_str()) {
        return Err(state_changed(format!(
            "receipt channel {} is not the claimed channel {}",
            receipt.channel_id,
            claimed.unwrap_or_default()
        )));
    }
    let stamp = iso(&at)?;
    for (target, _) in attempt_targets(tx, attempt).await? {
        match target {
            DeliveryTarget::Reminder(id) => {
                let changed = sqlx::query(
                    "UPDATE reminders SET sent_at = ?1, message_id = ?2
                     WHERE id = ?3 AND sent_at IS NULL AND message_id IS NULL",
                )
                .bind(&stamp)
                .bind(&receipt.message_id)
                .bind(&id)
                .execute(&mut *tx)
                .await
                .map_err(backend)?
                .rows_affected();
                if changed != 1 {
                    return Err(state_changed(format!("reminder {id} is gone or sent")));
                }
                sqlx::query(
                    "INSERT OR IGNORE INTO delivery_card_runs
                     (attempt_id, channel_id, message_id, run_id)
                     SELECT ?1, ?2, ?3, run_id FROM reminders WHERE id = ?4",
                )
                .bind(&attempt.0)
                .bind(&receipt.channel_id)
                .bind(&receipt.message_id)
                .bind(&id)
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
            }
            DeliveryTarget::Digest(week) => {
                let week = iso(&week)?;
                let changed = sqlx::query(
                    "INSERT INTO weekly_digests
                     (week_start, channel_id, message_id, posted_at, retired_at)
                     VALUES (?1, ?2, ?3, ?4, NULL)
                     ON CONFLICT(week_start) DO UPDATE SET
                         channel_id = excluded.channel_id, message_id = excluded.message_id,
                         posted_at = excluded.posted_at, retired_at = NULL
                     WHERE weekly_digests.retired_at IS NOT NULL",
                )
                .bind(&week)
                .bind(&receipt.channel_id)
                .bind(&receipt.message_id)
                .bind(&stamp)
                .execute(&mut *tx)
                .await
                .map_err(backend)?
                .rows_affected();
                if changed != 1 {
                    return Err(state_changed("the week already has an active digest"));
                }
            }
            DeliveryTarget::Card(id) => {
                let changed = sqlx::query(
                    "UPDATE proposal_cards SET message_id = ?1, posted_at = ?2
                     WHERE draft_id = ?3 AND message_id IS NULL AND channel_id = ?4",
                )
                .bind(&receipt.message_id)
                .bind(&stamp)
                .bind(&id)
                .bind(&receipt.channel_id)
                .execute(&mut *tx)
                .await
                .map_err(backend)?
                .rows_affected();
                if changed != 1 {
                    return Err(state_changed(format!(
                        "proposal {id}'s card is gone or posted"
                    )));
                }
            }
        }
    }
    if let Some(week) = record_week {
        raise_marker(tx, week, at).await?;
    }
    let bound = sqlx::query(
        "UPDATE delivery_attempts SET state = 'bound', channel_id = ?1, message_id = ?2,
         resolved_at = ?3 WHERE attempt_id = ?4 AND state = 'intent'",
    )
    .bind(&receipt.channel_id)
    .bind(&receipt.message_id)
    .bind(&stamp)
    .bind(&attempt.0)
    .execute(&mut *tx)
    .await
    .map_err(backend)?
    .rows_affected();
    if bound != 1 {
        return Err(state_changed(format!(
            "attempt {attempt} was not bound once"
        )));
    }
    bump_revision(tx).await
}

pub(super) async fn mark_indeterminate(
    tx: &mut SqliteConnection,
    lease: &Lease,
    attempt: &AttemptId,
) -> Result<(), JournalError> {
    check_live(tx, lease).await?;
    require_intent(tx, attempt).await?;
    sqlx::query("UPDATE delivery_attempts SET state = 'indeterminate' WHERE attempt_id = ?1")
        .bind(&attempt.0)
        .execute(tx)
        .await
        .map_err(backend)?;
    Ok(())
}

pub(super) async fn record_digest_week(
    tx: &mut SqliteConnection,
    lease: &Lease,
    week: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_live(tx, lease).await?;
    raise_marker(tx, week, at).await
}
