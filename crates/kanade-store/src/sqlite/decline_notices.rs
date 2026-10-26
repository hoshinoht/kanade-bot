//! Decline-notice candidates and their delivery bindings (schema v20).

use chrono::{DateTime, Utc};
use sqlx::sqlite::{Sqlite, SqliteArguments};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::schedule::store_error;
use crate::rules::check_decline_candidate;
use crate::sqlite::rows;
use kanade_domain::notify::{
    DECLINE_NOTICE_COOLDOWN, DeclineNotice, DeclineNoticeStore, MAX_PENDING_DECLINE_NOTICES,
};
use kanade_domain::scheduler::{Committed, StoreError};

const COLUMNS: &str = "run_id, user_id, channel_id, reference_id, display_name, message_id, notified_at, retract_pending";

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<DeclineNotice, StoreError> {
    let text =
        |column: &str| -> Result<String, StoreError> { row.try_get(column).map_err(store_error) };
    let optional_text = |column: &str| -> Result<Option<String>, StoreError> {
        row.try_get(column).map_err(store_error)
    };
    let notified_at = kanade_domain::time::from_iso(&text("notified_at")?).map_err(|error| {
        StoreError::Backend(format!("stored decline notice is unreadable: {error}"))
    })?;
    let retract_pending: i64 = row.try_get("retract_pending").map_err(store_error)?;
    let retract_pending = match retract_pending {
        0 => false,
        1 => true,
        value => {
            return Err(StoreError::Backend(format!(
                "stored decline notice has invalid retract_pending {value}"
            )));
        }
    };
    Ok(DeclineNotice {
        run_id: text("run_id")?,
        user_id: text("user_id")?,
        channel_id: optional_text("channel_id")?,
        reference_id: optional_text("reference_id")?,
        display_name: optional_text("display_name")?,
        message_id: optional_text("message_id")?,
        notified_at,
        retract_pending,
    })
}

/// Upsert a candidate inside its deciding schedule transaction. Bound rows are
/// retained: S3 only creates candidates after its cooldown/binding checks.
pub(super) async fn upsert(
    conn: &mut SqliteConnection,
    candidates: &[DeclineNotice],
) -> Result<(), StoreError> {
    for candidate in candidates {
        check_decline_candidate(candidate)?;
        sqlx::query(
            "INSERT INTO decline_notices
             (run_id, user_id, channel_id, reference_id, display_name, message_id, notified_at, retract_pending)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, 0)
             ON CONFLICT(run_id, user_id) DO UPDATE SET
               channel_id = excluded.channel_id,
               reference_id = excluded.reference_id,
               display_name = excluded.display_name,
               message_id = NULL,
               notified_at = excluded.notified_at,
               retract_pending = 0
             WHERE decline_notices.message_id IS NULL",
        )
        .bind(&candidate.run_id)
        .bind(&candidate.user_id)
        .bind(&candidate.channel_id)
        .bind(&candidate.reference_id)
        .bind(&candidate.display_name)
        .bind(rows::instant(&candidate.notified_at)?)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    }
    Ok(())
}

/// Mark existing candidates while the deciding RSVP transaction is still live.
pub(super) async fn mark_retractions(
    conn: &mut SqliteConnection,
    retractions: &[(String, String)],
) -> Result<(), StoreError> {
    for (run_id, user_id) in retractions {
        sqlx::query(
            "UPDATE decline_notices SET retract_pending = 1 WHERE run_id = ?1 AND user_id = ?2",
        )
        .bind(run_id)
        .bind(user_id)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    }
    Ok(())
}

/// One write transaction running `query`; true when it changed exactly one row.
async fn update_one<'q>(
    store: &SqliteStore,
    query: sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>,
) -> Result<bool, StoreError> {
    write_txn!(store, tx, async {
        query
            .execute(&mut *tx)
            .await
            .map(|done| done.rows_affected() == 1)
            .map_err(store_error)
    })
}

impl DeclineNoticeStore for SqliteStore {
    async fn commit_with_decline_notices(
        &self,
        expected_revision: u64,
        changes: kanade_domain::schedule::ChangeSet,
        meta: kanade_domain::history::ChangeMeta,
        candidates: Vec<DeclineNotice>,
        retractions: Vec<(String, String)>,
    ) -> Result<Option<Committed>, StoreError> {
        self.commit_schedule(expected_revision, changes, meta, &candidates, &retractions)
            .await
    }

    async fn decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<Option<DeclineNotice>, StoreError> {
        let mut conn = self.reader().await.map_err(store_error)?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM decline_notices WHERE run_id = ?1 AND user_id = ?2"
        ))
        .bind(run_id)
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?
        .map(|row| decode(&row))
        .transpose()
    }

    async fn pending_decline_notices(
        &self,
        limit: usize,
    ) -> Result<Vec<DeclineNotice>, StoreError> {
        let mut conn = self.reader().await.map_err(store_error)?;
        let limit = i64::try_from(limit.min(MAX_PENDING_DECLINE_NOTICES))
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM decline_notices
             WHERE message_id IS NULL OR retract_pending = 1
             ORDER BY notified_at, run_id, user_id LIMIT ?1"
        ))
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?
        .iter()
        .map(decode)
        .collect()
    }

    async fn bind_decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<bool, StoreError> {
        update_one(
            self,
            sqlx::query(
                "UPDATE decline_notices SET channel_id = ?1, message_id = ?2
                 WHERE run_id = ?3 AND user_id = ?4 AND message_id IS NULL",
            )
            .bind(channel_id)
            .bind(message_id)
            .bind(run_id)
            .bind(user_id),
        )
        .await
    }

    async fn mark_decline_retract_pending(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<bool, StoreError> {
        update_one(
            self,
            sqlx::query(
                "UPDATE decline_notices SET retract_pending = 1 WHERE run_id = ?1 AND user_id = ?2",
            )
            .bind(run_id)
            .bind(user_id),
        )
        .await
    }

    async fn clear_decline_notice_message(
        &self,
        run_id: &str,
        user_id: &str,
        message_id: &str,
    ) -> Result<bool, StoreError> {
        update_one(
            self,
            sqlx::query(
                "UPDATE decline_notices SET message_id = NULL, retract_pending = 0
                 WHERE run_id = ?1 AND user_id = ?2 AND message_id = ?3",
            )
            .bind(run_id)
            .bind(user_id)
            .bind(message_id),
        )
        .await
    }

    async fn decline_notice_on_cooldown(
        &self,
        run_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        Ok(self
            .decline_notice(run_id, user_id)
            .await?
            .is_some_and(|notice| {
                now.signed_duration_since(notice.notified_at) < DECLINE_NOTICE_COOLDOWN
            }))
    }
}
