//! `OwnerRequestStore` over `owner_requests` (0034): constant SQL, reads on a
//! reader connection, one `BEGIN IMMEDIATE` per write.

use chrono::{DateTime, Utc};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteRow};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use crate::domain::ownership::{OwnerRequest, OwnerRequestStatus, OwnerRequestStore};
use crate::domain::scheduler::StoreError;

const COLUMNS: &str = "id, fixed_run_id, requester, channel_id, message_id, created_at, \
     expires_at, status, decided_by, decided_at, message_settled";

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("owner_requests.{column} is unreadable: {detail}"))
}

fn decode(row: &SqliteRow) -> Result<OwnerRequest, StoreError> {
    let text = |column: &str| -> Result<String, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let optional = |column: &str| -> Result<Option<String>, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let at = |text: String, column: &str| {
        crate::domain::time::from_iso(&text).map_err(|error| corrupt(column, error))
    };
    let status = text("status")?;
    Ok(OwnerRequest {
        id: text("id")?,
        fixed_run_id: text("fixed_run_id")?,
        requester: text("requester")?,
        channel_id: optional("channel_id")?,
        message_id: optional("message_id")?,
        created_at: at(text("created_at")?, "created_at")?,
        expires_at: at(text("expires_at")?, "expires_at")?,
        status: OwnerRequestStatus::parse(&status).ok_or_else(|| corrupt("status", status))?,
        decided_by: optional("decided_by")?,
        decided_at: optional("decided_at")?
            .map(|text| at(text, "decided_at"))
            .transpose()?,
        message_settled: row
            .try_get::<i64, _>("message_settled")
            .map_err(|error| corrupt("message_settled", error))?
            != 0,
    })
}

async fn create(conn: &mut SqliteConnection, request: &OwnerRequest) -> Result<(), StoreError> {
    let open = sqlx::query(
        "SELECT 1 FROM owner_requests WHERE fixed_run_id = ?1 AND requester = ?2 \
         AND status = 'open'",
    )
    .bind(&request.fixed_run_id)
    .bind(&request.requester)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?;
    if open.is_some() {
        return Err(StoreError::Constraint(
            "an open owner request exists for this member and timing".into(),
        ));
    }
    sqlx::query(&format!(
        "INSERT INTO owner_requests ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
    ))
    .bind(&request.id)
    .bind(&request.fixed_run_id)
    .bind(&request.requester)
    .bind(&request.channel_id)
    .bind(&request.message_id)
    .bind(instant(&request.created_at)?)
    .bind(instant(&request.expires_at)?)
    .bind(request.status.as_str())
    .bind(&request.decided_by)
    .bind(request.decided_at.as_ref().map(instant).transpose()?)
    .bind(i64::from(request.message_settled))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

async fn close(
    conn: &mut SqliteConnection,
    id: &str,
    status: OwnerRequestStatus,
    decided_by: &str,
    at: &DateTime<Utc>,
) -> Result<bool, StoreError> {
    let changed = sqlx::query(
        "UPDATE owner_requests SET status = ?2, decided_by = ?3, decided_at = ?4 \
         WHERE id = ?1 AND status = 'open'",
    )
    .bind(id)
    .bind(status.as_str())
    .bind(decided_by)
    .bind(instant(at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?
    .rows_affected();
    Ok(changed == 1)
}

impl OwnerRequestStore for SqliteStore {
    async fn create_owner_request(&self, request: OwnerRequest) -> Result<(), StoreError> {
        request.check()?;
        write_txn!(self, tx, create(&mut tx, &request))
    }

    async fn owner_request(&self, id: &str) -> Result<Option<OwnerRequest>, StoreError> {
        read_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {COLUMNS} FROM owner_requests WHERE id = ?1"
            ))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .as_ref()
            .map(decode)
            .transpose()
        })
    }

    async fn open_owner_requests(&self) -> Result<Vec<OwnerRequest>, StoreError> {
        read_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {COLUMNS} FROM owner_requests WHERE status = 'open' \
                 ORDER BY created_at, id"
            ))
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(decode)
            .collect()
        })
    }

    async fn close_owner_request(
        &self,
        id: &str,
        status: OwnerRequestStatus,
        decided_by: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        if status == OwnerRequestStatus::Open {
            return Err(StoreError::Constraint("closing to open".into()));
        }
        write_txn!(self, tx, close(&mut tx, id, status, decided_by, &at))
    }

    async fn unsettled_owner_requests(&self) -> Result<Vec<OwnerRequest>, StoreError> {
        read_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {COLUMNS} FROM owner_requests WHERE status <> 'open' \
                 AND message_id IS NOT NULL AND message_settled = 0 ORDER BY decided_at, id"
            ))
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(decode)
            .collect()
        })
    }

    async fn settle_owner_request_message(&self, id: &str) -> Result<(), StoreError> {
        write_txn!(self, tx, async {
            sqlx::query("UPDATE owner_requests SET message_settled = 1 WHERE id = ?1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            Ok(())
        })
    }

    async fn set_owner_request_message(
        &self,
        id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), StoreError> {
        write_txn!(self, tx, async {
            sqlx::query("UPDATE owner_requests SET channel_id = ?2, message_id = ?3 WHERE id = ?1")
                .bind(id)
                .bind(channel_id)
                .bind(message_id)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            Ok(())
        })
    }
}
