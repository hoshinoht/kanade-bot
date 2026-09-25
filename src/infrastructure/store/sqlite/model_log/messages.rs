//! The watched-message cache.

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteRow;

use super::{instant, optional_text, read_instant, read_optional_instant, text};
use crate::domain::model_log::{MessageUpsert, WatchedMessage};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::sqlite::rows::optional_instant;
use crate::infrastructure::store::sqlite::schedule::store_error;

const COLUMNS: &str = "id, channel_id, author_id, created_at, edited_at, content, processed_at";

fn message_of(row: &SqliteRow) -> Result<WatchedMessage, StoreError> {
    Ok(WatchedMessage {
        id: text(row, "id")?,
        channel_id: text(row, "channel_id")?,
        author_id: text(row, "author_id")?,
        created_at: read_instant(row, "created_at")?,
        edited_at: read_optional_instant(row, "edited_at")?,
        content: text(row, "content")?,
        processed_at: read_optional_instant(row, "processed_at")?,
    })
}

pub(super) async fn upsert(
    conn: &mut SqliteConnection,
    message: &WatchedMessage,
) -> Result<MessageUpsert, StoreError> {
    let found = sqlx::query("SELECT content FROM messages WHERE id = ?1")
        .bind(&message.id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?;
    match found {
        None => {
            sqlx::query(
                "INSERT INTO messages \
                 (id, channel_id, author_id, created_at, edited_at, content, processed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&message.id)
            .bind(&message.channel_id)
            .bind(&message.author_id)
            .bind(instant(&message.created_at)?)
            .bind(optional_instant(message.edited_at.as_ref())?)
            .bind(&message.content)
            .bind(optional_instant(message.processed_at.as_ref())?)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?;
            Ok(MessageUpsert::Inserted)
        }
        Some(row) if optional_text(&row, "content")?.as_deref() == Some(&message.content) => {
            Ok(MessageUpsert::Unchanged)
        }
        Some(_) => {
            sqlx::query(
                "UPDATE messages SET content = ?1, edited_at = ?2, processed_at = NULL \
                 WHERE id = ?3",
            )
            .bind(&message.content)
            .bind(optional_instant(message.edited_at.as_ref())?)
            .bind(&message.id)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?;
            Ok(MessageUpsert::Edited)
        }
    }
}

pub(super) async fn mark_processed(
    conn: &mut SqliteConnection,
    ids: &[String],
    at: &DateTime<Utc>,
) -> Result<u64, StoreError> {
    let ids = crate::infrastructure::store::sqlite::rows::list(ids);
    let done = sqlx::query(
        "UPDATE messages SET processed_at = ?1 WHERE id IN (SELECT value FROM json_each(?2))",
    )
    .bind(instant(at)?)
    .bind(ids)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(done.rows_affected())
}

pub(super) async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<bool, StoreError> {
    let done = sqlx::query("DELETE FROM messages WHERE id = ?1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(done.rows_affected() > 0)
}

pub(super) async fn in_channel(
    conn: &mut SqliteConnection,
    channel_id: &str,
    since: &DateTime<Utc>,
    unprocessed_only: bool,
) -> Result<Vec<WatchedMessage>, StoreError> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM messages WHERE channel_id = ?1 AND created_at >= ?2 \
         AND (?3 = 0 OR processed_at IS NULL) ORDER BY created_at, id"
    ))
    .bind(channel_id)
    .bind(instant(since)?)
    .bind(unprocessed_only)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    rows.iter().map(message_of).collect()
}
