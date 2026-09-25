//! The notice outbox (`notice_outbox`, migration 0012): rows are written by
//! [`enqueue`] inside the deciding transaction and drained under a lease.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqliteConnection};

use super::{backend, check_live, corrupt, iso, payload};
use crate::domain::notify::{JournalError, Lease, OutboxNotice};
use crate::domain::schedule::Notice;
use crate::domain::scheduler::StoreError;
use crate::domain::time::from_iso;
use crate::infrastructure::store::sqlite::rows;
use crate::infrastructure::store::sqlite::schedule::store_error;

/// Write `notices` as `(source, 0..)`, inside the caller's transaction.
pub(in crate::infrastructure::store::sqlite) async fn enqueue(
    conn: &mut SqliteConnection,
    source: &str,
    notices: &[Notice],
    at: &DateTime<Utc>,
) -> Result<(), StoreError> {
    let created_at = rows::instant(at)?;
    for (ordinal, notice) in notices.iter().enumerate() {
        let payload = payload::encode(notice).map_err(StoreError::Constraint)?;
        sqlx::query(
            "INSERT INTO notice_outbox (source, ordinal, effect_kind, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(source)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(notice.effect_kind())
        .bind(payload)
        .bind(&created_at)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    }
    Ok(())
}

fn row_of(row: &sqlx::sqlite::SqliteRow) -> Result<OutboxNotice, JournalError> {
    let payload: String = row.try_get("payload").map_err(corrupt)?;
    let created: String = row.try_get("created_at").map_err(corrupt)?;
    let drained: Option<String> = row.try_get("drained_at").map_err(corrupt)?;
    Ok(OutboxNotice {
        source: row.try_get("source").map_err(corrupt)?,
        ordinal: row.try_get("ordinal").map_err(corrupt)?,
        notice: payload::decode(&payload).map_err(corrupt)?,
        created_at: from_iso(&created).map_err(corrupt)?,
        drained_at: drained
            .map(|text| from_iso(&text).map_err(corrupt))
            .transpose()?,
    })
}

pub(super) async fn pending(
    conn: &mut SqliteConnection,
) -> Result<Vec<OutboxNotice>, JournalError> {
    let rows = sqlx::query(
        "SELECT source, ordinal, payload, created_at, drained_at FROM notice_outbox
         WHERE state = 'pending' ORDER BY id",
    )
    .fetch_all(conn)
    .await
    .map_err(backend)?;
    rows.iter().map(row_of).collect()
}

pub(super) async fn all(conn: &mut SqliteConnection) -> Result<Vec<OutboxNotice>, JournalError> {
    let rows = sqlx::query(
        "SELECT source, ordinal, payload, created_at, drained_at FROM notice_outbox ORDER BY id",
    )
    .fetch_all(conn)
    .await
    .map_err(backend)?;
    rows.iter().map(row_of).collect()
}

pub(super) async fn mark_drained(
    tx: &mut SqliteConnection,
    lease: &Lease,
    source: &str,
    ordinal: i64,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    check_live(tx, lease).await?;
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM notice_outbox WHERE source = ?1 AND ordinal = ?2")
            .bind(source)
            .bind(ordinal)
            .fetch_optional(&mut *tx)
            .await
            .map_err(backend)?;
    match state.as_deref() {
        None => Err(JournalError::StateChanged(format!(
            "outbox notice {source}#{ordinal} does not exist"
        ))),
        Some("drained") => Ok(()),
        Some(_) => {
            sqlx::query(
                "UPDATE notice_outbox SET state = 'drained', drained_at = ?1
                 WHERE source = ?2 AND ordinal = ?3 AND state = 'pending'",
            )
            .bind(iso(&at)?)
            .bind(source)
            .bind(ordinal)
            .execute(tx)
            .await
            .map_err(backend)?;
            Ok(())
        }
    }
}
