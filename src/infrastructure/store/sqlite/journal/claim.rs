//! Claiming targets before transport (v4 `_persist_intent`).

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;
use sqlx::error::ErrorKind;

use super::{backend, check_live, encode, iso};
use crate::domain::notify::{
    AttemptId, Claim, DedupeKey, DeliveryTarget, JournalError, Lease, NotificationIntent,
    REQUEST_FINGERPRINT_VERSION, claim_key, effect_ordinal, is_sandbox_kind, request_fingerprint,
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
    for target in targets.iter().filter(|target| target.is_native()) {
        let (kind, primary, secondary) = encode(target)?;
        let claimed: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM delivery_attempt_targets t JOIN delivery_attempts a USING (attempt_id)
             WHERE t.binding_type = ?1 AND t.key_primary = ?2
                AND COALESCE(t.key_secondary, '') = ?3 AND t.released_at IS NULL
               AND a.dedupe_active = 1",
        )
        .bind(kind)
        .bind(&primary)
        .bind(&secondary)
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
            DeliveryTarget::Card(id) => {
                let row: Option<(bool, bool, bool)> = sqlx::query_as(
                    "SELECT c.message_id IS NOT NULL,
                            d.status IN ('open', 'submitted'),
                            EXISTS (SELECT 1 FROM delivery_attempt_targets AS ut
                                JOIN delivery_attempts AS ua ON ua.attempt_id = ut.attempt_id
                                WHERE ut.binding_type = 'card' AND ut.key_primary = c.draft_id
                                  AND ut.released_at IS NOT NULL AND ua.state = 'retired'
                                  AND ua.message_id IS NULL
                                  AND COALESCE(ua.resolved_by, '') NOT IN
                                      ('service:delivery-not-sent', 'service:delivery-rejected'))
                     FROM proposal_cards c JOIN drafts d ON d.id = c.draft_id
                     WHERE c.draft_id = ?1",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(backend)?;
                match row {
                    None => return Err(unavailable(format!("proposal {id} has no card"))),
                    Some((true, _, _)) => {
                        return Err(unavailable(format!("proposal {id}'s card was posted")));
                    }
                    Some((_, false, _)) => {
                        return Err(unavailable(format!("proposal {id} is closed")));
                    }
                    Some((_, _, true)) => {
                        return Err(unavailable(format!(
                            "proposal {id}'s card was retired without proof of delivery"
                        )));
                    }
                    Some((false, true, false)) => {}
                }
            }
            DeliveryTarget::Decline { run_id, user_id } => {
                let row: Option<(Option<String>, bool)> = sqlx::query_as(
                    "SELECT d.message_id,
                            EXISTS (
                                SELECT 1 FROM delivery_attempt_targets t
                                JOIN delivery_attempts a USING (attempt_id)
                                WHERE t.binding_type = 'decline' AND t.key_primary = d.run_id
                                  AND t.key_secondary = d.user_id
                                  AND a.resolved_by = 'service:decline-retraction'
                                  AND a.intended_at >= d.notified_at
                            )
                     FROM decline_notices d WHERE d.run_id = ?1 AND d.user_id = ?2",
                )
                .bind(run_id)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(backend)?;
                match row {
                    None => {
                        return Err(unavailable(format!(
                            "decline notice for run {run_id} and member {user_id} does not exist"
                        )));
                    }
                    Some((Some(_), _)) => {
                        return Err(unavailable(format!(
                            "decline notice for run {run_id} and member {user_id} is already bound"
                        )));
                    }
                    Some((None, true)) => {
                        return Err(unavailable(format!(
                            "decline notice for run {run_id} and member {user_id} was retracted"
                        )));
                    }
                    Some((None, false)) => {}
                }
            }
            // A sandbox card is display only and may show a sample run.
            DeliveryTarget::DebugCard { kind, .. } if is_sandbox_kind(kind) => {}
            DeliveryTarget::DebugCard { run_id, .. } => {
                let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM runs WHERE id = ?1")
                    .bind(run_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(backend)?;
                if exists.is_none() {
                    return Err(unavailable(format!("run {run_id} does not exist")));
                }
            }
        }
    }
    Ok(())
}

/// Retired attempts under a source key still hold it unless the transport
/// proved them unsent.
const SOURCE_HELD: &str = "SELECT 1 FROM delivery_attempts WHERE dedupe_key = ?1
     AND (dedupe_active = 1 OR COALESCE(resolved_by, '') != 'service:delivery-not-sent')
     LIMIT 1";

pub(super) async fn claim_source(
    tx: &mut SqliteConnection,
    lease: &Lease,
    intent: &NotificationIntent,
    source: &str,
    source_ordinal: i64,
    at: DateTime<Utc>,
) -> Result<Claim, JournalError> {
    check_live(tx, lease).await?;
    if !intent.targets.is_empty() {
        return Err(JournalError::InvalidInput(
            "source keys claim target-less effects only".into(),
        ));
    }
    let key = DedupeKey::source(source, source_ordinal);
    let held: Option<i64> = sqlx::query_scalar(SOURCE_HELD)
        .bind(key.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(backend)?;
    if held.is_some() {
        return Ok(Claim::Held);
    }
    let next: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(effect_ordinal), -1) + 1 FROM delivery_attempts WHERE operation_id = ?1",
    )
    .bind(&lease.operation_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(backend)?;
    insert_attempt(tx, lease, intent, next, "source", &key, at).await
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
    intent.debug_card()?;
    let (scope, key) = if intent.operation_scoped() {
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
    insert_attempt(tx, lease, intent, ordinal, scope, &key, at).await
}

/// Record a fresh `intent` attempt; `Held` when another claim won the key.
async fn insert_attempt(
    tx: &mut SqliteConnection,
    lease: &Lease,
    intent: &NotificationIntent,
    ordinal: i64,
    scope: &str,
    key: &DedupeKey,
    at: DateTime<Utc>,
) -> Result<Claim, JournalError> {
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
        .filter(|target| target.is_native())
        .map(claim_key)
        .collect::<Result<Vec<_>, _>>()?;
    keys.sort();
    keys.dedup();
    for (ordinal, (kind, primary, secondary)) in keys.iter().enumerate() {
        sqlx::query(
            "INSERT INTO delivery_attempt_targets
             (attempt_id, target_ordinal, binding_type, key_primary, key_secondary)
              VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&attempt.0)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(kind)
        .bind(primary)
        .bind(secondary)
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
    }
    if let Some((run_id, kind)) = intent.debug_card()? {
        sqlx::query(
            "INSERT INTO debug_cards (attempt_id, run_id, kind, channel_id) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&attempt.0)
        .bind(run_id)
        .bind(kind)
        .bind(&intent.channel_id)
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
    }
    Ok(Claim::Fresh(attempt))
}
