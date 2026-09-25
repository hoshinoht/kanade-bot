//! `kanade import v4 --refresh-logs`: rewrite imported (`v4-`) log rows in
//! place. Logs refuse UPDATE by trigger, so each row is deleted (children
//! first, as retention does) and inserted again.

use sqlx::{Connection, SqliteConnection};

use super::super::SqliteStore;
use super::super::schedule::store_error;
use super::{chat, extractions};
use crate::domain::model_log::{ChatInteraction, ExtractionLog};
use crate::domain::scheduler::StoreError;

/// The only ids this path may delete.
pub const IMPORTED_PREFIX: &str = "v4-";

const DELETE_CHAT: [&str; 3] = [
    "DELETE FROM chat_tools WHERE interaction_id = ?1",
    "DELETE FROM chat_rounds WHERE interaction_id = ?1",
    "DELETE FROM chat_interactions WHERE id = ?1",
];
const DELETE_EXTRACTION: [&str; 2] = [
    "DELETE FROM extraction_members WHERE extraction_id = ?1",
    "DELETE FROM extractions WHERE id = ?1",
];

/// Rows replaced (the rest of the given ones were inserted new).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refreshed {
    pub chats: u64,
    pub extractions: u64,
}

/// Deletes the row under `statements`; `true` when the parent (last) existed.
async fn delete(
    conn: &mut SqliteConnection,
    statements: &[&str],
    id: &str,
) -> Result<bool, StoreError> {
    let mut parent = 0;
    for sql in statements {
        parent = sqlx::query(sql)
            .bind(id)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?
            .rows_affected();
    }
    Ok(parent > 0)
}

async fn replace(
    conn: &mut SqliteConnection,
    chats: &[ChatInteraction],
    logs: &[ExtractionLog],
) -> Result<Refreshed, StoreError> {
    let mut done = Refreshed::default();
    for log in logs {
        done.extractions += u64::from(delete(conn, &DELETE_EXTRACTION, &log.id).await?);
        extractions::insert(conn, log).await?;
    }
    for chat in chats {
        done.chats += u64::from(delete(conn, &DELETE_CHAT, &chat.id).await?);
        chat::insert(conn, chat).await?;
    }
    Ok(done)
}

impl SqliteStore {
    /// Insert the given imported logs, replacing any stored row with the
    /// same id, all in one write transaction. Every id must carry
    /// [`IMPORTED_PREFIX`], so native rows are never touched.
    ///
    /// # Errors
    /// [`StoreError::Constraint`] for a non-imported id or an invalid shape
    /// (nothing written); store failures roll everything back.
    pub async fn refresh_imported_logs(
        &self,
        chats: &[ChatInteraction],
        logs: &[ExtractionLog],
    ) -> Result<Refreshed, StoreError> {
        let ids = chats
            .iter()
            .map(|chat| &chat.id)
            .chain(logs.iter().map(|log| &log.id));
        if let Some(id) = ids.into_iter().find(|id| !id.starts_with(IMPORTED_PREFIX)) {
            return Err(StoreError::Constraint(format!(
                "{id} is not an imported log id"
            )));
        }
        for chat in chats {
            chat.check_shape()?;
        }
        for log in logs {
            log.check_shape()?;
        }
        write_txn!(self, tx, replace(&mut tx, chats, logs))
    }
}
