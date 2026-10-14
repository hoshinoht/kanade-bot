//! `AuthAuditStore` over `auth_audit` (0032): constant SQL, reads on a reader
//! connection, one `BEGIN IMMEDIATE` per append, which first deletes a batch
//! of rows past retention.

use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteRow};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::auth_audit::{
    AUDIT_PRUNE_BATCH, AUDIT_RETENTION, AuditFilter, AuditKind, AuditRealm, AuditRow,
    AuthAuditStore,
};

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("auth_audit.{column} is unreadable: {detail}"))
}

fn decode(row: &SqliteRow) -> Result<AuditRow, StoreError> {
    let text = |column: &str| -> Result<String, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let optional = |column: &str| -> Result<Option<String>, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let realm = text("realm")?;
    let event = text("event")?;
    Ok(AuditRow {
        seq: row.try_get("seq").map_err(|error| corrupt("seq", error))?,
        at: crate::domain::time::from_iso(&text("at")?).map_err(|error| corrupt("at", error))?,
        realm: AuditRealm::parse(&realm).ok_or_else(|| corrupt("realm", realm))?,
        event: AuditKind::parse(&event).ok_or_else(|| corrupt("event", event))?,
        actor: optional("actor")?,
        method: optional("method")?,
        reason: optional("reason")?,
        request: optional("request")?,
        client: optional("client")?,
        device: optional("device")?,
        request_id: text("request_id")?,
    })
}

async fn append(conn: &mut SqliteConnection, row: &AuditRow) -> Result<i64, StoreError> {
    if let Some(cutoff) = row.at.checked_sub_signed(AUDIT_RETENTION) {
        sqlx::query(
            "DELETE FROM auth_audit WHERE seq IN \
             (SELECT seq FROM auth_audit WHERE at < ?1 ORDER BY seq LIMIT ?2)",
        )
        .bind(instant(&cutoff)?)
        .bind(AUDIT_PRUNE_BATCH)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    }
    let seq = sqlx::query(
        "INSERT INTO auth_audit \
         (at, realm, event, actor, method, reason, request, client, device, request_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(instant(&row.at)?)
    .bind(row.realm.as_str())
    .bind(row.event.as_str())
    .bind(&row.actor)
    .bind(&row.method)
    .bind(&row.reason)
    .bind(&row.request)
    .bind(&row.client)
    .bind(&row.device)
    .bind(&row.request_id)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?
    .last_insert_rowid();
    Ok(seq)
}

async fn page(
    conn: &mut SqliteConnection,
    filter: &AuditFilter,
) -> Result<Vec<AuditRow>, StoreError> {
    let from = filter.from.as_ref().map(instant).transpose()?;
    let to = filter.to.as_ref().map(instant).transpose()?;
    let rows = sqlx::query(
        "SELECT seq, at, realm, event, actor, method, reason, request, client, device, \
         request_id FROM auth_audit \
         WHERE (?1 IS NULL OR realm = ?1) AND (?2 IS NULL OR event = ?2) \
         AND (?3 IS NULL OR actor = ?3) AND (?4 IS NULL OR at >= ?4) \
         AND (?5 IS NULL OR at < ?5) AND (?6 IS NULL OR seq < ?6) \
         ORDER BY seq DESC LIMIT ?7",
    )
    .bind(filter.realm.map(AuditRealm::as_str))
    .bind(filter.event.map(AuditKind::as_str))
    .bind(&filter.actor)
    .bind(from)
    .bind(to)
    .bind(filter.before_seq)
    .bind(i64::from(filter.page_size()))
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    rows.iter().map(decode).collect()
}

impl AuthAuditStore for SqliteStore {
    async fn append_audit(&self, row: AuditRow) -> Result<i64, StoreError> {
        row.check()?;
        write_txn!(self, tx, append(&mut tx, &row))
    }

    async fn audit_page(&self, filter: &AuditFilter) -> Result<Vec<AuditRow>, StoreError> {
        read_txn!(self, tx, page(&mut tx, filter))
    }
}
