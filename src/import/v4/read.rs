//! The v4 snapshot, opened read-only and `immutable` so SQLite neither
//! writes nor locks it (no `-wal`/`-shm` side files). Every column is read
//! as optional; mapping decides what a missing or mistyped value means.

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{ConnectOptions, Connection, Row, SqliteConnection};

use super::ImportError;

const TABLES: [&str; 4] = ["fixed_runs", "chat_interactions", "extractions", "messages"];

pub struct Snapshot {
    conn: SqliteConnection,
}

pub struct V4Fixed {
    pub id: Option<String>,
    pub owner_id: Option<String>,
    pub channel_id: Option<String>,
    pub bosses: Option<String>,
    pub weekday: Option<i64>,
    pub time: Option<String>,
    pub participants: Option<String>,
    pub note: Option<String>,
}

/// A log row's id and timestamp, read before its body so rows outside the
/// window are never loaded.
pub struct Head {
    pub id: Option<String>,
    pub at: Option<String>,
}

pub struct V4Chat {
    pub id: String,
    pub at: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub author_id: Option<String>,
    pub model: Option<String>,
    pub question: Option<String>,
    pub reply: Option<String>,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub rounds: Option<i64>,
    pub latency_ms: Option<i64>,
    pub model_ms: Option<i64>,
    pub tools_ms: Option<i64>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub tool_calls: Option<String>,
    pub model_rounds: Option<String>,
}

pub struct V4Extraction {
    pub id: String,
    pub at: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub raw_response: Option<String>,
    pub latency_ms: Option<i64>,
    pub message_ids: Option<String>,
    pub amendment_ids: Option<String>,
}

pub struct V4Message {
    pub id: String,
    pub channel_id: Option<String>,
    pub author_id: Option<String>,
    pub created_at: Option<String>,
    pub content: Option<String>,
    pub processed_at: Option<String>,
}

fn text(row: &SqliteRow, column: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(column).ok().flatten()
}

fn int(row: &SqliteRow, column: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(column).ok().flatten()
}

fn source(error: sqlx::Error) -> ImportError {
    ImportError::Source(format!("the v4 snapshot could not be read: {error}"))
}

/// Ids bound as one JSON array, so each query stays constant SQL.
fn id_list(ids: &[String]) -> String {
    serde_json::Value::from(ids.to_vec()).to_string()
}

impl Snapshot {
    pub async fn open(path: &Path) -> Result<Self, ImportError> {
        if !path.is_file() {
            return Err(ImportError::Source(
                "the v4 snapshot is not a readable file".into(),
            ));
        }
        let mut conn = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .immutable(true)
            .create_if_missing(false)
            .connect()
            .await
            .map_err(source)?;
        let found: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN (?1, ?2, ?3, ?4)",
        )
        .bind(TABLES[0])
        .bind(TABLES[1])
        .bind(TABLES[2])
        .bind(TABLES[3])
        .fetch_one(&mut conn)
        .await
        .map_err(source)?;
        if found != TABLES.len() as i64 {
            let _ = conn.close().await;
            return Err(ImportError::Source(
                "not a v4 database: fixed_runs, chat_interactions, extractions or messages is missing"
                    .into(),
            ));
        }
        Ok(Self { conn })
    }

    pub async fn close(self) {
        let _ = self.conn.close().await;
    }

    pub async fn fixed_runs(&mut self) -> Result<Vec<V4Fixed>, ImportError> {
        let rows = sqlx::query(
            "SELECT id, owner_id, channel_id, bosses, weekday, time, participants, note \
             FROM fixed_runs ORDER BY weekday, time, id",
        )
        .fetch_all(&mut self.conn)
        .await
        .map_err(source)?;
        Ok(rows
            .iter()
            .map(|row| V4Fixed {
                id: text(row, "id"),
                owner_id: text(row, "owner_id"),
                channel_id: text(row, "channel_id"),
                bosses: text(row, "bosses"),
                weekday: int(row, "weekday"),
                time: text(row, "time"),
                participants: text(row, "participants"),
                note: text(row, "note"),
            })
            .collect())
    }

    pub async fn chat_heads(&mut self) -> Result<Vec<Head>, ImportError> {
        let rows = sqlx::query("SELECT id, at FROM chat_interactions")
            .fetch_all(&mut self.conn)
            .await
            .map_err(source)?;
        Ok(rows.iter().map(head).collect())
    }

    pub async fn extraction_heads(&mut self) -> Result<Vec<Head>, ImportError> {
        let rows = sqlx::query("SELECT id, at FROM extractions")
            .fetch_all(&mut self.conn)
            .await
            .map_err(source)?;
        Ok(rows.iter().map(head).collect())
    }

    pub async fn chats(&mut self, ids: &[String]) -> Result<Vec<V4Chat>, ImportError> {
        let rows = sqlx::query(
            "SELECT id, at, channel_id, message_id, author_id, model, question, reply, outcome, \
             error, rounds, latency_ms, model_ms, tools_ms, prompt_tokens, completion_tokens, \
             tool_calls, model_rounds \
             FROM chat_interactions WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY at, id",
        )
        .bind(id_list(ids))
        .fetch_all(&mut self.conn)
        .await
        .map_err(source)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(V4Chat {
                    id: text(row, "id")?,
                    at: text(row, "at"),
                    channel_id: text(row, "channel_id"),
                    message_id: text(row, "message_id"),
                    author_id: text(row, "author_id"),
                    model: text(row, "model"),
                    question: text(row, "question"),
                    reply: text(row, "reply"),
                    outcome: text(row, "outcome"),
                    error: text(row, "error"),
                    rounds: int(row, "rounds"),
                    latency_ms: int(row, "latency_ms"),
                    model_ms: int(row, "model_ms"),
                    tools_ms: int(row, "tools_ms"),
                    prompt_tokens: int(row, "prompt_tokens"),
                    completion_tokens: int(row, "completion_tokens"),
                    tool_calls: text(row, "tool_calls"),
                    model_rounds: text(row, "model_rounds"),
                })
            })
            .collect())
    }

    pub async fn extractions(&mut self, ids: &[String]) -> Result<Vec<V4Extraction>, ImportError> {
        let rows = sqlx::query(
            "SELECT id, at, model, prompt, raw_response, latency_ms, message_ids, amendment_ids \
             FROM extractions WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY at, id",
        )
        .bind(id_list(ids))
        .fetch_all(&mut self.conn)
        .await
        .map_err(source)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(V4Extraction {
                    id: text(row, "id")?,
                    at: text(row, "at"),
                    model: text(row, "model"),
                    prompt: text(row, "prompt"),
                    raw_response: text(row, "raw_response"),
                    latency_ms: int(row, "latency_ms"),
                    message_ids: text(row, "message_ids"),
                    amendment_ids: text(row, "amendment_ids"),
                })
            })
            .collect())
    }

    pub async fn messages(&mut self, ids: &[String]) -> Result<Vec<V4Message>, ImportError> {
        let rows = sqlx::query(
            "SELECT id, channel_id, author_id, created_at, content, processed_at \
             FROM messages WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id",
        )
        .bind(id_list(ids))
        .fetch_all(&mut self.conn)
        .await
        .map_err(source)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(V4Message {
                    id: text(row, "id")?,
                    channel_id: text(row, "channel_id"),
                    author_id: text(row, "author_id"),
                    created_at: text(row, "created_at"),
                    content: text(row, "content"),
                    processed_at: text(row, "processed_at"),
                })
            })
            .collect())
    }
}

fn head(row: &SqliteRow) -> Head {
    Head {
        id: text(row, "id"),
        at: text(row, "at"),
    }
}
