//! Operation leases and start-up recovery.
//!
//! Minimal lease model: OPEN mode only, generation 0; a lease is proven by a
//! random token whose SHA-256 is stored.

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;

use super::{backend, check_live, iso};
use crate::domain::notify::{AttemptId, JournalError, Lease, Recovery, check_resolution};

const LEASE_END_ACTOR: &str = "service:lease-end";
const LEASE_END_REASON: &str = "operation finished";

pub(super) async fn begin_lease(
    tx: &mut SqliteConnection,
    instance_id: &str,
    operation_kind: &str,
    at: DateTime<Utc>,
) -> Result<Lease, JournalError> {
    let generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM maintenance_state WHERE id = 1 AND mode = 'OPEN'",
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(backend)?
    .ok_or(JournalError::LeaseNotLive)?;
    let lease = Lease::new(
        uuid::Uuid::new_v4().to_string(),
        instance_id.to_owned(),
        generation,
        uuid::Uuid::new_v4().to_string(),
    );
    sqlx::query(
        "INSERT INTO maintenance_leases
         (operation_id, instance_id, owner_token_hash, generation, operation_kind, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&lease.operation_id)
    .bind(instance_id)
    .bind(lease.token_hash())
    .bind(generation)
    .bind(operation_kind)
    .bind(iso(&at)?)
    .execute(tx)
    .await
    .map_err(backend)?;
    Ok(lease)
}

pub(super) async fn end_lease(
    tx: &mut SqliteConnection,
    lease: &Lease,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_resolution(LEASE_END_ACTOR, LEASE_END_REASON)?;
    check_live(tx, lease).await?;
    sqlx::query(
        "UPDATE maintenance_leases SET lifecycle = 'retired', retired_at = ?1, retired_by = ?2,
         retirement_reason = ?3 WHERE operation_id = ?4 AND lifecycle = 'live'",
    )
    .bind(iso(&at)?)
    .bind(LEASE_END_ACTOR)
    .bind(LEASE_END_REASON)
    .bind(&lease.operation_id)
    .execute(tx)
    .await
    .map_err(backend)?;
    Ok(())
}

/// Every `intent` becomes `indeterminate`; every live lease is orphaned. Run
/// only after taking ownership, when no earlier process can still be sending.
pub(super) async fn on_start(
    tx: &mut SqliteConnection,
    at: DateTime<Utc>,
) -> Result<Recovery, JournalError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT attempt_id FROM delivery_attempts WHERE state = 'intent' ORDER BY attempt_id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(backend)?;
    sqlx::query("UPDATE delivery_attempts SET state = 'indeterminate' WHERE state = 'intent'")
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
    let orphaned = sqlx::query(
        "UPDATE maintenance_leases SET lifecycle = 'orphaned', orphaned_at = ?1
         WHERE lifecycle = 'live'",
    )
    .bind(iso(&at)?)
    .execute(tx)
    .await
    .map_err(backend)?
    .rows_affected();
    Ok(Recovery {
        indeterminate: ids.into_iter().map(AttemptId).collect(),
        orphaned_leases: usize::try_from(orphaned).unwrap_or(usize::MAX),
    })
}
