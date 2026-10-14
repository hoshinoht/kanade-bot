//! The notice outbox (`notice_outbox`, migration 0012): rows are written by
//! [`enqueue`] inside the deciding transaction and drained under a lease.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqliteConnection};

use super::{backend, check_live, corrupt, iso, payload};
use crate::domain::notify::{
    DrainReason, JournalError, Lease, OutboxNotice, PendingNotices, UndecodableNotice,
};
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

const COLUMNS: &str = "source, ordinal, payload, created_at, drained_at, drained_reason";

fn key_of(row: &sqlx::sqlite::SqliteRow) -> Result<(String, i64), JournalError> {
    Ok((
        row.try_get("source").map_err(corrupt)?,
        row.try_get("ordinal").map_err(corrupt)?,
    ))
}

/// The row's fields past its key; `Err` carries why they do not decode.
fn decode_row(row: &sqlx::sqlite::SqliteRow, key: (String, i64)) -> Result<OutboxNotice, String> {
    let text = |column: &str| -> Result<Option<String>, String> {
        row.try_get(column).map_err(|error| error.to_string())
    };
    let instant = |value: String| from_iso(&value).map_err(|error| error.to_string());
    let payload = text("payload")?.ok_or("payload is null")?;
    let reason = match text("drained_reason")? {
        None => None,
        Some(value) => {
            Some(DrainReason::parse(&value).ok_or_else(|| format!("drained_reason {value}"))?)
        }
    };
    Ok(OutboxNotice {
        source: key.0,
        ordinal: key.1,
        notice: payload::decode(&payload)?,
        created_at: instant(text("created_at")?.ok_or("created_at is null")?)?,
        drained_at: text("drained_at")?.map(instant).transpose()?,
        drained_reason: reason,
    })
}

pub(super) async fn pending(conn: &mut SqliteConnection) -> Result<PendingNotices, JournalError> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM notice_outbox WHERE state = 'pending' ORDER BY id"
    ))
    .fetch_all(conn)
    .await
    .map_err(backend)?;
    let mut pending = PendingNotices::default();
    for row in &rows {
        let key = key_of(row)?;
        match decode_row(row, key.clone()) {
            Ok(notice) => pending.notices.push(notice),
            Err(detail) => pending.undecodable.push(UndecodableNotice {
                source: key.0,
                ordinal: key.1,
                detail,
            }),
        }
    }
    Ok(pending)
}

pub(super) async fn all(conn: &mut SqliteConnection) -> Result<Vec<OutboxNotice>, JournalError> {
    let rows = sqlx::query(&format!("SELECT {COLUMNS} FROM notice_outbox ORDER BY id"))
        .fetch_all(conn)
        .await
        .map_err(backend)?;
    rows.iter()
        .map(|row| decode_row(row, key_of(row)?).map_err(corrupt))
        .collect()
}

pub(super) async fn mark_drained(
    tx: &mut SqliteConnection,
    lease: &Lease,
    source: &str,
    ordinal: i64,
    reason: DrainReason,
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
                "UPDATE notice_outbox SET state = 'drained', drained_at = ?1, drained_reason = ?4
                 WHERE source = ?2 AND ordinal = ?3 AND state = 'pending'",
            )
            .bind(iso(&at)?)
            .bind(source)
            .bind(ordinal)
            .bind(reason.as_str())
            .execute(tx)
            .await
            .map_err(backend)?;
            Ok(())
        }
    }
}
