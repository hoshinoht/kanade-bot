//! The extraction log and its filter query.

use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteRow;

use super::{
    instant, json_text, optional_signed, optional_text, read_instant, read_json, read_list,
    read_optional_u64, read_u64, text,
};
use crate::domain::model_log::{
    ExtractionFilter, ExtractionLog, ExtractionOutcome, ExtractionRefusal, LogCursor, LogFacets,
    LogPage, page_size,
};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::sqlite::rows::{list as json_list, optional_instant};
use crate::infrastructure::store::sqlite::schedule::store_error;

const COLUMNS: &str = "e.id, e.at, e.channel_id, e.member_ids, e.model, e.reasoning, e.prompt, \
    e.raw_response, e.latency_ms, e.request_count, e.outcome, e.error, e.guardrail, \
    e.message_ids, e.proposal_ids, e.refusals";

/// [`COLUMNS`] without the prompt and response bodies (list pages).
const LIST_COLUMNS: &str = "e.id, e.at, e.channel_id, e.member_ids, e.model, e.reasoning, \
    '' AS prompt, '' AS raw_response, e.latency_ms, e.request_count, e.outcome, e.error, \
    e.guardrail, e.message_ids, e.proposal_ids, e.refusals";

fn refusals_of(row: &SqliteRow) -> Result<Vec<ExtractionRefusal>, StoreError> {
    let value = read_json(row, "refusals")?;
    value
        .as_array()
        .ok_or_else(|| StoreError::Backend("extractions.refusals is not an array".into()))?
        .iter()
        .map(|item| {
            ExtractionRefusal::from_json(item)
                .ok_or_else(|| StoreError::Backend("extractions.refusals item".into()))
        })
        .collect()
}

fn log_of(row: &SqliteRow) -> Result<ExtractionLog, StoreError> {
    let outcome = text(row, "outcome")?;
    Ok(ExtractionLog {
        id: text(row, "id")?,
        at: read_instant(row, "at")?,
        channel_id: optional_text(row, "channel_id")?,
        member_ids: read_list(row, "member_ids")?,
        model: text(row, "model")?,
        reasoning: optional_text(row, "reasoning")?,
        prompt: text(row, "prompt")?,
        raw_response: text(row, "raw_response")?,
        latency_ms: read_optional_u64(row, "latency_ms")?,
        request_count: u32::try_from(read_u64(row, "request_count")?)
            .map_err(|error| StoreError::Backend(format!("extractions.request_count: {error}")))?,
        outcome: ExtractionOutcome::parse(&outcome)
            .ok_or_else(|| StoreError::Backend(format!("extraction outcome {outcome}")))?,
        error: optional_text(row, "error")?,
        guardrail: read_json(row, "guardrail")?,
        message_ids: read_list(row, "message_ids")?,
        proposal_ids: read_list(row, "proposal_ids")?,
        refusals: refusals_of(row)?,
    })
}

pub(super) async fn insert(
    conn: &mut SqliteConnection,
    log: &ExtractionLog,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO extractions (id, at, channel_id, member_ids, model, reasoning, prompt, \
         raw_response, latency_ms, request_count, outcome, error, guardrail, message_ids, \
         proposal_ids, refusals) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )
    .bind(&log.id)
    .bind(instant(&log.at)?)
    .bind(&log.channel_id)
    .bind(json_list(&log.member_ids))
    .bind(&log.model)
    .bind(&log.reasoning)
    .bind(&log.prompt)
    .bind(&log.raw_response)
    .bind(optional_signed(log.latency_ms, "latency_ms")?)
    .bind(i64::from(log.request_count))
    .bind(log.outcome.as_str())
    .bind(&log.error)
    .bind(json_text(&log.guardrail))
    .bind(json_list(&log.message_ids))
    .bind(json_list(&log.proposal_ids))
    .bind(json_text(&serde_json::Value::Array(
        log.refusals
            .iter()
            .map(ExtractionRefusal::to_json)
            .collect(),
    )))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    sqlx::query(
        "INSERT OR IGNORE INTO extraction_members (extraction_id, member_id) \
         SELECT ?1, value FROM json_each(?2)",
    )
    .bind(&log.id)
    .bind(json_list(&log.member_ids))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

pub(super) async fn load(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ExtractionLog>, StoreError> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM extractions e WHERE e.id = ?1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(log_of)
    .transpose()
}

/// Optional filters are `?n IS NULL OR …`, so SQLite cannot use a per-column
/// index; the plan walks `extractions_recent` newest first and stops at the
/// page limit (a bounded scan, pinned by `plans::list_walks_the_time_index`).
pub(super) fn list_sql(omit_bodies: bool) -> String {
    let columns = if omit_bodies { LIST_COLUMNS } else { COLUMNS };
    format!(
        "SELECT {columns} FROM extractions e \
         WHERE (?1 IS NULL OR e.model = ?1) \
         AND (?2 IS NULL OR e.at >= ?2) \
         AND (?3 IS NULL OR e.at < ?3) \
         AND (?4 IS NULL OR e.outcome IN (SELECT value FROM json_each(?4))) \
         AND (?5 IS NULL OR e.channel_id = ?5) \
         AND (?6 IS NULL OR EXISTS (SELECT 1 FROM extraction_members m \
              WHERE m.extraction_id = e.id AND m.member_id = ?6)) \
         AND (?7 IS NULL OR instr(lower(e.prompt), lower(?7)) > 0 \
              OR instr(lower(e.raw_response), lower(?7)) > 0) \
         AND (?8 IS NULL OR e.at < ?8 OR (e.at = ?8 AND e.id < ?9)) \
         ORDER BY e.at DESC, e.id DESC LIMIT ?10"
    )
}

pub(super) const FACET_SQL: [&str; 3] = [
    "SELECT DISTINCT model FROM extractions ORDER BY 1",
    "SELECT DISTINCT outcome FROM extractions ORDER BY 1",
    "SELECT DISTINCT channel_id FROM extractions WHERE channel_id IS NOT NULL ORDER BY 1",
];

pub(super) async fn list(
    conn: &mut SqliteConnection,
    filter: &ExtractionFilter,
) -> Result<LogPage<ExtractionLog>, StoreError> {
    let size = page_size(filter.limit);
    let outcomes = (!filter.outcomes.is_empty()).then(|| {
        json_list(
            &filter
                .outcomes
                .iter()
                .map(|outcome| outcome.as_str().to_owned())
                .collect::<Vec<_>>(),
        )
    });
    let rows = sqlx::query(&list_sql(filter.omit_bodies))
        .bind(&filter.model)
        .bind(optional_instant(filter.from.as_ref())?)
        .bind(optional_instant(filter.to.as_ref())?)
        .bind(outcomes)
        .bind(&filter.channel)
        .bind(&filter.member)
        .bind(&filter.q)
        .bind(optional_instant(
            filter.cursor.as_ref().map(|cursor| &cursor.at),
        )?)
        .bind(filter.cursor.as_ref().map(|cursor| cursor.id.as_str()))
        .bind(i64::from(size) + 1)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?;
    let items = rows.iter().map(log_of).collect::<Result<Vec<_>, _>>()?;
    Ok(page(items, size))
}

/// Trim the one-extra probe row into a `next` cursor.
pub(super) fn page<T: Keyed>(mut items: Vec<T>, size: u32) -> LogPage<T> {
    let size = size as usize;
    let next = if items.len() > size {
        items.truncate(size);
        items.last().map(Keyed::cursor)
    } else {
        None
    };
    LogPage { items, next }
}

/// An item's position in the newest-first order.
pub(super) trait Keyed {
    fn cursor(&self) -> LogCursor;
}

impl Keyed for ExtractionLog {
    fn cursor(&self) -> LogCursor {
        LogCursor {
            at: self.at,
            id: self.id.clone(),
        }
    }
}

pub(super) async fn distinct(
    conn: &mut SqliteConnection,
    sql: &'static str,
) -> Result<Vec<String>, StoreError> {
    sqlx::query_scalar(sql)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)
}

pub(super) async fn facets(conn: &mut SqliteConnection) -> Result<LogFacets, StoreError> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM extractions")
        .fetch_one(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(LogFacets {
        total: u64::try_from(total).unwrap_or_default(),
        models: distinct(conn, FACET_SQL[0]).await?,
        tools: Vec::new(),
        outcomes: distinct(conn, FACET_SQL[1]).await?,
        channels: distinct(conn, FACET_SQL[2]).await?,
    })
}
