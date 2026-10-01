//! Decline-notice candidates and their delivery bindings (schema v20).

use chrono::{DateTime, Utc};
use sqlx::error::ErrorKind;
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use crate::domain::notify::{
    DECLINE_NOTICE_COOLDOWN, DeclineNotice, DeclineNoticeStore, MAX_PENDING_DECLINE_NOTICES,
};
use crate::domain::scheduler::{Committed, StoreError};
use crate::infrastructure::store::sqlite::rows;

const COLUMNS: &str = "run_id, user_id, channel_id, reference_id, display_name, message_id, notified_at, retract_pending";

fn store_error(error: sqlx::Error) -> StoreError {
    match &error {
        sqlx::Error::Database(db)
            if matches!(
                db.kind(),
                ErrorKind::UniqueViolation
                    | ErrorKind::ForeignKeyViolation
                    | ErrorKind::NotNullViolation
                    | ErrorKind::CheckViolation
            ) =>
        {
            StoreError::Constraint(db.message().to_owned())
        }
        _ => StoreError::Backend(error.to_string()),
    }
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<DeclineNotice, StoreError> {
    let text =
        |column: &str| -> Result<String, StoreError> { row.try_get(column).map_err(store_error) };
    let optional_text = |column: &str| -> Result<Option<String>, StoreError> {
        row.try_get(column).map_err(store_error)
    };
    let notified_at = crate::domain::time::from_iso(&text("notified_at")?).map_err(|error| {
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

fn check_candidate(candidate: &DeclineNotice) -> Result<(), StoreError> {
    let Some(display_name) = &candidate.display_name else {
        return Err(StoreError::Constraint(
            "decline notice candidate needs a display name".into(),
        ));
    };
    if display_name.chars().count() > 128 {
        return Err(StoreError::Constraint(
            "decline notice display name exceeds 128 characters".into(),
        ));
    }
    Ok(())
}

/// Upsert a candidate inside its deciding schedule transaction. Bound rows are
/// retained: S3 only creates candidates after its cooldown/binding checks.
pub(super) async fn upsert(
    conn: &mut SqliteConnection,
    candidates: &[DeclineNotice],
) -> Result<(), StoreError> {
    for candidate in candidates {
        check_candidate(candidate)?;
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

async fn begin(store: &SqliteStore) -> Result<super::writer::WriterLease<'_>, StoreError> {
    store
        .writer
        .lease()
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))
}

impl DeclineNoticeStore for SqliteStore {
    async fn commit_with_decline_notices(
        &self,
        expected_revision: u64,
        changes: crate::domain::schedule::ChangeSet,
        meta: crate::domain::history::ChangeMeta,
        candidates: Vec<DeclineNotice>,
        retractions: Vec<(String, String)>,
    ) -> Result<Option<Committed>, StoreError> {
        let mut lease = begin(self).await?;
        let runs = crate::infrastructure::store::observer::touched_runs(&changes);
        let (result, healthy) = super::schedule::commit_on(
            lease.conn(),
            expected_revision,
            changes,
            meta,
            &candidates,
            &retractions,
        )
        .await;
        lease.finish(healthy);
        if let Ok(Some(committed)) = &result
            && !committed.replayed
        {
            self.runs_written(&runs);
        }
        result
    }

    async fn decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<Option<DeclineNotice>, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(store_error)?;
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
        let mut conn = self.readers.acquire().await.map_err(store_error)?;
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
        let mut lease = begin(self).await?;
        let (result, healthy) = match lease.conn().begin_with("BEGIN IMMEDIATE").await {
            Ok(mut tx) => {
                let result = sqlx::query(
                    "UPDATE decline_notices SET channel_id = ?1, message_id = ?2
                     WHERE run_id = ?3 AND user_id = ?4 AND message_id IS NULL",
                )
                .bind(channel_id)
                .bind(message_id)
                .bind(run_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map(|done| done.rows_affected() == 1)
                .map_err(store_error);
                match result {
                    Ok(value) => match tx.commit().await {
                        Ok(()) => (Ok(value), true),
                        Err(error) => (Err(store_error(error)), false),
                    },
                    Err(error) => (Err(error), tx.rollback().await.is_ok()),
                }
            }
            Err(error) => (Err(store_error(error)), false),
        };
        lease.finish(healthy);
        result
    }

    async fn mark_decline_retract_pending(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<bool, StoreError> {
        let mut lease = begin(self).await?;
        let (result, healthy) = match lease.conn().begin_with("BEGIN IMMEDIATE").await {
            Ok(mut tx) => {
                let result = sqlx::query(
                    "UPDATE decline_notices SET retract_pending = 1 WHERE run_id = ?1 AND user_id = ?2",
                )
                .bind(run_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map(|done| done.rows_affected() == 1)
                .map_err(store_error);
                match result {
                    Ok(value) => match tx.commit().await {
                        Ok(()) => (Ok(value), true),
                        Err(error) => (Err(store_error(error)), false),
                    },
                    Err(error) => (Err(error), tx.rollback().await.is_ok()),
                }
            }
            Err(error) => (Err(store_error(error)), false),
        };
        lease.finish(healthy);
        result
    }

    async fn clear_decline_notice_message(
        &self,
        run_id: &str,
        user_id: &str,
        message_id: &str,
    ) -> Result<bool, StoreError> {
        let mut lease = begin(self).await?;
        let (result, healthy) = match lease.conn().begin_with("BEGIN IMMEDIATE").await {
            Ok(mut tx) => {
                let result = sqlx::query(
                    "UPDATE decline_notices SET message_id = NULL, retract_pending = 0
                     WHERE run_id = ?1 AND user_id = ?2 AND message_id = ?3",
                )
                .bind(run_id)
                .bind(user_id)
                .bind(message_id)
                .execute(&mut *tx)
                .await
                .map(|done| done.rows_affected() == 1)
                .map_err(store_error);
                match result {
                    Ok(value) => match tx.commit().await {
                        Ok(()) => (Ok(value), true),
                        Err(error) => (Err(store_error(error)), false),
                    },
                    Err(error) => (Err(error), tx.rollback().await.is_ok()),
                }
            }
            Err(error) => (Err(store_error(error)), false),
        };
        lease.finish(healthy);
        result
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
