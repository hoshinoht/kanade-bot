//! Rescan jobs, chat allowance overrides and self-service tips.

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};

use super::{
    instant, json_text, optional_text, read_instant, read_json, read_list, read_optional_instant,
    read_u64, signed, text,
};
use crate::domain::model_log::{AllowanceOverride, RescanJob, RescanStatus};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::sqlite::rows::{list as json_list, optional_instant};
use crate::infrastructure::store::sqlite::schedule::store_error;

const RESCAN_COLUMNS: &str = "id, channels, \"window\", source, automated, requested_by, status, \
    created_at, started_at, finished_at, results, error";

fn rescan_of(row: &SqliteRow) -> Result<RescanJob, StoreError> {
    let status = text(row, "status")?;
    Ok(RescanJob {
        id: text(row, "id")?,
        channels: read_list(row, "channels")?,
        window: text(row, "window")?,
        source: text(row, "source")?,
        automated: row
            .try_get("automated")
            .map_err(|error| StoreError::Backend(format!("rescan_jobs.automated: {error}")))?,
        requested_by: optional_text(row, "requested_by")?,
        status: RescanStatus::parse(&status)
            .ok_or_else(|| StoreError::Backend(format!("rescan status {status}")))?,
        created_at: read_instant(row, "created_at")?,
        started_at: read_optional_instant(row, "started_at")?,
        finished_at: read_optional_instant(row, "finished_at")?,
        results: read_json(row, "results")?,
        error: optional_text(row, "error")?,
    })
}

pub(super) async fn insert_rescan(
    conn: &mut SqliteConnection,
    job: &RescanJob,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO rescan_jobs (id, channels, \"window\", source, automated, requested_by, \
         status, created_at, started_at, finished_at, results, error) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&job.id)
    .bind(json_list(&job.channels))
    .bind(&job.window)
    .bind(&job.source)
    .bind(job.automated)
    .bind(&job.requested_by)
    .bind(job.status.as_str())
    .bind(instant(&job.created_at)?)
    .bind(optional_instant(job.started_at.as_ref())?)
    .bind(optional_instant(job.finished_at.as_ref())?)
    .bind(json_text(&job.results))
    .bind(&job.error)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

pub(super) async fn update_rescan(
    conn: &mut SqliteConnection,
    job: &RescanJob,
) -> Result<bool, StoreError> {
    let Some(current) = load_rescan(conn, &job.id).await? else {
        return Ok(false);
    };
    if current.status.is_final() {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE rescan_jobs SET status = ?1, started_at = ?2, finished_at = ?3, results = ?4, \
         error = ?5 WHERE id = ?6",
    )
    .bind(job.status.as_str())
    .bind(optional_instant(job.started_at.as_ref())?)
    .bind(optional_instant(job.finished_at.as_ref())?)
    .bind(json_text(&job.results))
    .bind(&job.error)
    .bind(&job.id)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(true)
}

pub(super) async fn load_rescan(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<RescanJob>, StoreError> {
    sqlx::query(&format!(
        "SELECT {RESCAN_COLUMNS} FROM rescan_jobs WHERE id = ?1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(rescan_of)
    .transpose()
}

pub(super) async fn recent_rescans(
    conn: &mut SqliteConnection,
    limit: u32,
) -> Result<Vec<RescanJob>, StoreError> {
    let rows = sqlx::query(&format!(
        "SELECT {RESCAN_COLUMNS} FROM rescan_jobs ORDER BY created_at DESC, id DESC LIMIT ?1"
    ))
    .bind(i64::from(limit))
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    rows.iter().map(rescan_of).collect()
}

fn allowance_of(row: &SqliteRow) -> Result<AllowanceOverride, StoreError> {
    Ok(AllowanceOverride {
        member_id: text(row, "member_id")?,
        count: u32::try_from(read_u64(row, "count")?)
            .map_err(|error| StoreError::Backend(format!("chat_allowance_overrides: {error}")))?,
        window_ms: read_u64(row, "window_ms")?,
        updated_at: read_instant(row, "updated_at")?,
    })
}

pub(super) async fn set_allowance(
    conn: &mut SqliteConnection,
    entry: &AllowanceOverride,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO chat_allowance_overrides (member_id, count, window_ms, updated_at) \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT (member_id) DO UPDATE SET \
         count = excluded.count, window_ms = excluded.window_ms, updated_at = excluded.updated_at",
    )
    .bind(&entry.member_id)
    .bind(i64::from(entry.count))
    .bind(signed(entry.window_ms, "window_ms")?)
    .bind(instant(&entry.updated_at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

pub(super) async fn clear_allowance(
    conn: &mut SqliteConnection,
    member_id: &str,
) -> Result<bool, StoreError> {
    let done = sqlx::query("DELETE FROM chat_allowance_overrides WHERE member_id = ?1")
        .bind(member_id)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(done.rows_affected() > 0)
}

pub(super) async fn allowances(
    conn: &mut SqliteConnection,
) -> Result<Vec<AllowanceOverride>, StoreError> {
    let rows = sqlx::query(
        "SELECT member_id, count, window_ms, updated_at FROM chat_allowance_overrides \
         ORDER BY member_id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    rows.iter().map(allowance_of).collect()
}

pub(super) async fn claim_tip(
    conn: &mut SqliteConnection,
    member_id: &str,
    week: &DateTime<Utc>,
    at: &DateTime<Utc>,
) -> Result<bool, StoreError> {
    let done = sqlx::query(
        "INSERT OR IGNORE INTO self_service_tips (member_id, week, sent_at) VALUES (?1, ?2, ?3)",
    )
    .bind(member_id)
    .bind(instant(week)?)
    .bind(instant(at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(done.rows_affected() == 1)
}

pub(super) async fn release_tip(
    conn: &mut SqliteConnection,
    member_id: &str,
    week: &DateTime<Utc>,
) -> Result<bool, StoreError> {
    let done = sqlx::query("DELETE FROM self_service_tips WHERE member_id = ?1 AND week = ?2")
        .bind(member_id)
        .bind(instant(week)?)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(done.rows_affected() == 1)
}
