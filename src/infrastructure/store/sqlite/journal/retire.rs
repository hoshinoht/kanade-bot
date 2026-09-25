//! Retiring attempts: definite rejections, confirmed digest replacements
//! (v4 `_retire_digest_replacement`) and operator no-replay recovery.

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;

use super::{
    attempt_state, attempt_targets, backend, bump_revision, check_live, iso, release_and_retire,
    require_intent, state_changed, suppress_natives,
};
use crate::domain::notify::{
    AttemptId, DIGEST_REPLACEMENT_ACTOR, DIGEST_REPLACEMENT_REASON, EffectKind, JournalError,
    Lease, NOT_SENT_ACTOR, REJECTED_ACTOR, WeeklyDigest, check_resolution,
};

pub(super) async fn rejected(
    tx: &mut SqliteConnection,
    lease: &Lease,
    attempt: &AttemptId,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_resolution(REJECTED_ACTOR, reason)?;
    check_live(tx, lease).await?;
    require_intent(tx, attempt).await?;
    let targets = attempt_targets(tx, attempt).await?;
    suppress_natives(tx, &targets, at).await?;
    release_and_retire(tx, attempt, REJECTED_ACTOR, reason, at).await?;
    bump_revision(tx).await
}

pub(super) async fn for_replacement(
    tx: &mut SqliteConnection,
    lease: &Lease,
    digest: &WeeklyDigest,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_live(tx, lease).await?;
    let week = iso(&digest.week_start)?;
    let claim: Option<String> = sqlx::query_scalar(
        "SELECT a.attempt_id FROM weekly_digests d
         JOIN delivery_attempt_targets t ON t.binding_type = 'digest'
          AND t.key_primary = d.week_start AND COALESCE(t.key_secondary, '') = ''
         JOIN delivery_attempts a USING (attempt_id)
         WHERE d.week_start = ?1 AND d.channel_id = ?2 AND d.message_id = ?3
           AND d.retired_at IS NULL AND t.released_at IS NULL
           AND a.effect_kind = ?4 AND a.state = 'bound' AND a.dedupe_active = 1
           AND a.destination_kind = 'channel' AND a.channel_id = d.channel_id
           AND a.message_id = d.message_id",
    )
    .bind(&week)
    .bind(&digest.channel_id)
    .bind(&digest.message_id)
    .bind(EffectKind::Digest.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(backend)?;
    let Some(attempt) = claim.map(AttemptId) else {
        return Err(state_changed("no bound claim matches the digest card"));
    };
    let retired = sqlx::query(
        "UPDATE weekly_digests SET retired_at = ?1
         WHERE week_start = ?2 AND channel_id = ?3 AND message_id = ?4 AND retired_at IS NULL",
    )
    .bind(iso(&at)?)
    .bind(&week)
    .bind(&digest.channel_id)
    .bind(&digest.message_id)
    .execute(&mut *tx)
    .await
    .map_err(backend)?
    .rows_affected();
    if retired != 1 {
        return Err(state_changed("digest replacement lost its native row"));
    }
    release_and_retire(
        tx,
        &attempt,
        DIGEST_REPLACEMENT_ACTOR,
        DIGEST_REPLACEMENT_REASON,
        at,
    )
    .await?;
    bump_revision(tx).await
}

pub(super) async fn unproven(
    tx: &mut SqliteConnection,
    attempt: &AttemptId,
    actor: &str,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_resolution(actor, reason)?;
    let (state, active) = attempt_state(tx, attempt).await?;
    if !state.is_unresolved() || !active {
        return Err(state_changed(format!("attempt {attempt} is resolved")));
    }
    let own_lease_live: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM delivery_attempts a JOIN maintenance_leases l
          ON l.operation_id = a.operation_id
         WHERE a.attempt_id = ?1 AND l.lifecycle = 'live'",
    )
    .bind(&attempt.0)
    .fetch_optional(&mut *tx)
    .await
    .map_err(backend)?;
    if own_lease_live.is_some() {
        return Err(state_changed("the attempt's own lease is still live"));
    }
    let targets = attempt_targets(tx, attempt).await?;
    if targets.iter().any(|(_, released)| *released) {
        return Err(state_changed("an unresolved attempt has a released target"));
    }
    suppress_natives(tx, &targets, at).await?;
    release_and_retire(tx, attempt, actor, reason, at).await?;
    bump_revision(tx).await
}

pub(super) async fn release_unsent(
    tx: &mut SqliteConnection,
    lease: &Lease,
    attempt: &AttemptId,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_resolution(NOT_SENT_ACTOR, reason)?;
    check_live(tx, lease).await?;
    require_intent(tx, attempt).await?;
    let operation: String =
        sqlx::query_scalar("SELECT operation_id FROM delivery_attempts WHERE attempt_id = ?1")
            .bind(&attempt.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(backend)?;
    if operation != lease.operation_id {
        return Err(state_changed(format!(
            "attempt {attempt} belongs to another operation"
        )));
    }
    release_and_retire(tx, attempt, NOT_SENT_ACTOR, reason, at).await
}

/// v4 `retire_weekly_digests_before`, compared as ISO text as v4 does.
pub(super) async fn digests_before(
    tx: &mut SqliteConnection,
    lease: &Lease,
    week: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<usize, JournalError> {
    check_live(tx, lease).await?;
    let retired = sqlx::query(
        "UPDATE weekly_digests SET retired_at = ?1 WHERE week_start < ?2 AND retired_at IS NULL",
    )
    .bind(iso(&at)?)
    .bind(iso(&week)?)
    .execute(&mut *tx)
    .await
    .map_err(backend)?
    .rows_affected();
    if retired > 0 {
        bump_revision(tx).await?;
    }
    Ok(usize::try_from(retired).unwrap_or(usize::MAX))
}
