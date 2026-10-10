//! Retention: one bounded batch per call; the caller loops in separate
//! write transactions. Each child DELETE re-selects the same oldest
//! `LIMIT ?2` parents (deterministic order inside one transaction) before
//! the parents go.

use sqlx::SqliteConnection;

use super::instant;
use crate::domain::model_log::PruneCounts;
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::sqlite::schedule::store_error;

const OLD_EXTRACTIONS: &str = "SELECT id FROM extractions WHERE at < ?1 ORDER BY at, id LIMIT ?2";
const OLD_CHATS: &str = "SELECT id FROM chat_interactions WHERE at < ?1 ORDER BY at, id LIMIT ?2";

/// `(statement, counted)`; only parent deletes count.
pub(super) fn statements() -> [(String, bool); 9] {
    [
        (
            format!("DELETE FROM extraction_members WHERE extraction_id IN ({OLD_EXTRACTIONS})"),
            false,
        ),
        (
            format!("DELETE FROM extractions WHERE id IN ({OLD_EXTRACTIONS})"),
            true,
        ),
        (
            format!("DELETE FROM chat_tools WHERE interaction_id IN ({OLD_CHATS})"),
            false,
        ),
        (
            format!("DELETE FROM chat_rounds WHERE interaction_id IN ({OLD_CHATS})"),
            false,
        ),
        (
            format!("DELETE FROM chat_masked WHERE interaction_id IN ({OLD_CHATS})"),
            false,
        ),
        (
            format!("DELETE FROM chat_interactions WHERE id IN ({OLD_CHATS})"),
            true,
        ),
        (
            "DELETE FROM messages WHERE id IN (SELECT id FROM messages \
             WHERE processed_at IS NOT NULL AND created_at < ?1 ORDER BY created_at, id LIMIT ?2)"
                .to_owned(),
            true,
        ),
        (
            "DELETE FROM notice_outbox WHERE id IN (SELECT id FROM notice_outbox \
             WHERE state = 'drained' AND drained_at < ?1 ORDER BY drained_at, id LIMIT ?2)"
                .to_owned(),
            true,
        ),
        (
            "DELETE FROM rewrites WHERE id IN (SELECT id FROM rewrites \
             WHERE at < ?1 ORDER BY at, id LIMIT ?2)"
                .to_owned(),
            true,
        ),
    ]
}

pub(super) async fn batch(
    conn: &mut SqliteConnection,
    before: &chrono::DateTime<chrono::Utc>,
    size: u32,
) -> Result<PruneCounts, StoreError> {
    let before = instant(before)?;
    let mut counted = Vec::with_capacity(5);
    for (sql, count) in statements() {
        let done = sqlx::query(&sql)
            .bind(&before)
            .bind(i64::from(size))
            .execute(&mut *conn)
            .await
            .map_err(store_error)?;
        if count {
            counted.push(done.rows_affected());
        }
    }
    Ok(PruneCounts {
        extractions: counted[0],
        chats: counted[1],
        messages: counted[2],
        notices: counted[3],
        rewrites: counted[4],
    })
}
