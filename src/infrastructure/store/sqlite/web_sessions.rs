//! `WebSessionStore` over migration 0009: constant SQL, one `BEGIN IMMEDIATE`
//! per write.

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::web_sessions::{
    LoginMethod, SessionFuture, SessionOrigin, WebSession, WebSessionStore,
};

const COLUMNS: &str = "id_hash, origin, method, subject, display, created_at, last_seen_at, \
    checked_at, expires_at, avatar_hash, device";

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("web_sessions.{column} is unreadable: {detail}"))
}

fn text(row: &SqliteRow, column: &str) -> Result<String, StoreError> {
    row.try_get(column).map_err(|error| corrupt(column, error))
}

fn at(row: &SqliteRow, column: &str) -> Result<DateTime<Utc>, StoreError> {
    crate::domain::time::from_iso(&text(row, column)?).map_err(|error| corrupt(column, error))
}

fn session_of(row: &SqliteRow) -> Result<WebSession, StoreError> {
    let origin = text(row, "origin")?;
    let method = text(row, "method")?;
    Ok(WebSession {
        id_hash: text(row, "id_hash")?,
        origin: SessionOrigin::parse(&origin).ok_or_else(|| corrupt("origin", origin))?,
        method: LoginMethod::parse(&method).ok_or_else(|| corrupt("method", method))?,
        subject: text(row, "subject")?,
        display: text(row, "display")?,
        created_at: at(row, "created_at")?,
        last_seen_at: at(row, "last_seen_at")?,
        checked_at: at(row, "checked_at")?,
        expires_at: at(row, "expires_at")?,
        avatar_hash: row
            .try_get("avatar_hash")
            .map_err(|error| corrupt("avatar_hash", error))?,
        device: row
            .try_get("device")
            .map_err(|error| corrupt("device", error))?,
    })
}

async fn put(
    conn: &mut SqliteConnection,
    session: &WebSession,
    replaces: Option<&str>,
) -> Result<(), StoreError> {
    if let Some(old) = replaces {
        sqlx::query("DELETE FROM web_sessions WHERE id_hash = ?1")
            .bind(old)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?;
    }
    sqlx::query(
        "INSERT INTO web_sessions (id_hash, origin, method, subject, display, created_at, \
         last_seen_at, checked_at, expires_at, avatar_hash, device) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(&session.id_hash)
    .bind(session.origin.as_str())
    .bind(session.method.as_str())
    .bind(&session.subject)
    .bind(&session.display)
    .bind(instant(&session.created_at)?)
    .bind(instant(&session.last_seen_at)?)
    .bind(instant(&session.checked_at)?)
    .bind(instant(&session.expires_at)?)
    .bind(&session.avatar_hash)
    .bind(&session.device)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

async fn load(
    conn: &mut SqliteConnection,
    id_hash: &str,
) -> Result<Option<WebSession>, StoreError> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM web_sessions WHERE id_hash = ?1"
    ))
    .bind(id_hash)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(session_of)
    .transpose()
}

async fn touch(
    conn: &mut SqliteConnection,
    id_hash: &str,
    last_seen_at: &DateTime<Utc>,
    checked_at: &DateTime<Utc>,
) -> Result<bool, StoreError> {
    let Some(current) = load(&mut *conn, id_hash).await? else {
        return Ok(false);
    };
    let seen = current.last_seen_at.max(*last_seen_at);
    let done = sqlx::query(
        "UPDATE web_sessions SET last_seen_at = ?1, checked_at = ?2 WHERE id_hash = ?3",
    )
    .bind(instant(&seen)?)
    .bind(instant(checked_at)?)
    .bind(id_hash)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(done.rows_affected() > 0)
}

async fn delete(conn: &mut SqliteConnection, id_hash: &str) -> Result<bool, StoreError> {
    let done = sqlx::query("DELETE FROM web_sessions WHERE id_hash = ?1")
        .bind(id_hash)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(done.rows_affected() > 0)
}

async fn list_subject(
    conn: &mut SqliteConnection,
    origin: SessionOrigin,
    method: LoginMethod,
    subject: &str,
) -> Result<Vec<WebSession>, StoreError> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM web_sessions \
         WHERE origin = ?1 AND method = ?2 AND subject = ?3 ORDER BY created_at, id_hash"
    ))
    .bind(origin.as_str())
    .bind(method.as_str())
    .bind(subject)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?
    .iter()
    .map(session_of)
    .collect()
}

async fn delete_subject(
    conn: &mut SqliteConnection,
    origin: SessionOrigin,
    method: LoginMethod,
    subject: &str,
) -> Result<u64, StoreError> {
    let done =
        sqlx::query("DELETE FROM web_sessions WHERE origin = ?1 AND method = ?2 AND subject = ?3")
            .bind(origin.as_str())
            .bind(method.as_str())
            .bind(subject)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?;
    Ok(done.rows_affected())
}

async fn prune(
    conn: &mut SqliteConnection,
    now: &DateTime<Utc>,
    idle_before: &DateTime<Utc>,
) -> Result<u64, StoreError> {
    // Instants compare as ISO text, as elsewhere in this store.
    let done = sqlx::query("DELETE FROM web_sessions WHERE expires_at <= ?1 OR last_seen_at <= ?2")
        .bind(instant(now)?)
        .bind(instant(idle_before)?)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(done.rows_affected())
}

impl WebSessionStore for SqliteStore {
    fn put_session<'a>(
        &'a self,
        session: &'a WebSession,
        replaces: Option<&'a str>,
    ) -> SessionFuture<'a, ()> {
        Box::pin(async move { write_txn!(self, tx, put(&mut tx, session, replaces)) })
    }

    fn load_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, Option<WebSession>> {
        Box::pin(async move { read_txn!(self, tx, load(&mut tx, id_hash)) })
    }

    fn touch_session<'a>(
        &'a self,
        id_hash: &'a str,
        last_seen_at: DateTime<Utc>,
        checked_at: DateTime<Utc>,
    ) -> SessionFuture<'a, bool> {
        Box::pin(async move {
            write_txn!(
                self,
                tx,
                touch(&mut tx, id_hash, &last_seen_at, &checked_at)
            )
        })
    }

    fn delete_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, bool> {
        Box::pin(async move { write_txn!(self, tx, delete(&mut tx, id_hash)) })
    }

    fn subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, Vec<WebSession>> {
        Box::pin(async move { read_txn!(self, tx, list_subject(&mut tx, origin, method, subject)) })
    }

    fn delete_subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, u64> {
        Box::pin(
            async move { write_txn!(self, tx, delete_subject(&mut tx, origin, method, subject)) },
        )
    }

    fn prune_sessions(
        &self,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'_, u64> {
        Box::pin(async move { write_txn!(self, tx, prune(&mut tx, &now, &idle_before)) })
    }
}
