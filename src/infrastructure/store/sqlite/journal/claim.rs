//! Claiming targets before transport (v4 `_persist_intent`).

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;
use sqlx::error::ErrorKind;

use super::{backend, check_live, encode, iso};
use crate::domain::notify::{
    AttemptId, Claim, DedupeKey, DeliveryTarget, JournalError, Lease, NotificationIntent,
    REQUEST_FINGERPRINT_VERSION, claim_key, effect_ordinal, request_fingerprint,
};

fn unavailable(detail: String) -> JournalError {
    JournalError::TargetUnavailable(detail)
}

async fn held(
    tx: &mut SqliteConnection,
    key: &DedupeKey,
    targets: &[DeliveryTarget],
) -> Result<bool, JournalError> {
    let active: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM delivery_attempts WHERE dedupe_key = ?1 AND dedupe_active = 1",
    )
    .bind(key.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(backend)?;
    if active.is_some() {
        return Ok(true);
    }
    for target in targets {
        let (kind, primary) = encode(target)?;
        let claimed: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM delivery_attempt_targets t JOIN delivery_attempts a USING (attempt_id)
             WHERE t.binding_type = ?1 AND t.key_primary = ?2
               AND COALESCE(t.key_secondary, '') = '' AND t.released_at IS NULL
               AND a.dedupe_active = 1",
        )
        .bind(kind)
        .bind(&primary)
        .fetch_optional(&mut *tx)
        .await
        .map_err(backend)?;
        if claimed.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// v4 `unproven_retirement_exists` for one reminder, excluding releases the
/// transport proved unsent ([`crate::domain::notify::NOT_SENT_ACTOR`]).
pub(in crate::infrastructure::store::sqlite) const UNPROVEN_REMINDER: &str =
    "EXISTS (SELECT 1 FROM delivery_attempt_targets AS ut
     JOIN delivery_attempts AS ua ON ua.attempt_id = ut.attempt_id
     WHERE ut.binding_type = 'reminder' AND ut.key_primary = reminders.id
       AND ut.released_at IS NOT NULL AND ua.origin = 'runtime'
       AND ua.state = 'retired' AND ua.message_id IS NULL
       AND COALESCE(ua.resolved_by, '') != 'service:delivery-not-sent')";

/// Each target still exists and is unsent, re-read under the write lock.
async fn check_targets(
    tx: &mut SqliteConnection,
    targets: &[DeliveryTarget],
) -> Result<(), JournalError> {
    for target in targets {
        match target {
            DeliveryTarget::Reminder(id) => {
                let row: Option<(bool, bool)> = sqlx::query_as(&format!(
                    "SELECT sent_at IS NOT NULL OR message_id IS NOT NULL, {UNPROVEN_REMINDER}
                     FROM reminders WHERE id = ?1"
                ))
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(backend)?;
                match row {
                    None => return Err(unavailable(format!("reminder {id} does not exist"))),
                    Some((true, _)) => {
                        return Err(unavailable(format!("reminder {id} was already sent")));
                    }
                    Some((_, true)) => {
                        return Err(unavailable(format!(
                            "reminder {id} was retired without proof of delivery"
                        )));
                    }
                    Some((false, false)) => {}
                }
            }
            DeliveryTarget::Digest(week) => {
                let week = iso(week)?;
                let active: Option<i64> = sqlx::query_scalar(
                    "SELECT 1 FROM weekly_digests WHERE week_start = ?1 AND retired_at IS NULL",
                )
                .bind(&week)
                .fetch_optional(&mut *tx)
                .await
                .map_err(backend)?;
                if active.is_some() {
                    return Err(unavailable(format!(
                        "digest for {week} already has an active card"
                    )));
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn claim(
    tx: &mut SqliteConnection,
    lease: &Lease,
    intent: &NotificationIntent,
    requested: Option<i64>,
    at: DateTime<Utc>,
) -> Result<Claim, JournalError> {
    check_live(tx, lease).await?;
    let next: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(effect_ordinal), -1) + 1 FROM delivery_attempts WHERE operation_id = ?1",
    )
    .bind(&lease.operation_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(backend)?;
    let ordinal = effect_ordinal(intent, requested, next)?;
    if ordinal < next {
        // The operation already ran this effect (v4 `_operation_attempt`).
        return Ok(Claim::Held);
    }
    let (scope, key) = if intent.targets.is_empty() {
        (
            "operation",
            DedupeKey::operation(&lease.operation_id, ordinal),
        )
    } else {
        ("native", DedupeKey::native(&intent.targets)?)
    };
    if held(tx, &key, &intent.targets).await? {
        return Ok(Claim::Held);
    }
    check_targets(tx, &intent.targets).await?;

    let attempt = AttemptId(uuid::Uuid::new_v4().to_string());
    let guild: Option<String> = sqlx::query_scalar("SELECT guild_id FROM store_meta WHERE id = 1")
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
    let inserted = sqlx::query(
        "INSERT INTO delivery_attempts
         (attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, effect_kind,
          dedupe_scope, dedupe_key, dedupe_active, state, destination_kind, guild_id,
          channel_id, fingerprint_version, request_fingerprint, intended_at)
         VALUES (?1, ?2, ?3, ?4, 'runtime', ?5, ?6, ?7, 1, 'intent', 'channel', ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&attempt.0)
    .bind(&lease.operation_id)
    .bind(ordinal)
    .bind(&lease.instance_id)
    .bind(intent.effect.as_str())
    .bind(scope)
    .bind(key.as_str())
    .bind(guild)
    .bind(&intent.channel_id)
    .bind(REQUEST_FINGERPRINT_VERSION)
    .bind(request_fingerprint(intent))
    .bind(iso(&at)?)
    .execute(&mut *tx)
    .await;
    if let Err(sqlx::Error::Database(error)) = &inserted
        && error.kind() == ErrorKind::UniqueViolation
    {
        return Ok(Claim::Held);
    }
    inserted.map_err(backend)?;

    let mut keys = intent
        .targets
        .iter()
        .map(claim_key)
        .collect::<Result<Vec<_>, _>>()?;
    keys.sort();
    keys.dedup();
    for (ordinal, (kind, primary, _)) in keys.iter().enumerate() {
        sqlx::query(
            "INSERT INTO delivery_attempt_targets
             (attempt_id, target_ordinal, binding_type, key_primary, key_secondary)
             VALUES (?1, ?2, ?3, ?4, NULL)",
        )
        .bind(&attempt.0)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(kind)
        .bind(primary)
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
    }
    Ok(Claim::Fresh(attempt))
}
