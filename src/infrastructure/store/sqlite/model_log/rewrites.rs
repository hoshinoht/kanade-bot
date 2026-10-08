//! The Rewrites log (0027, prompt 0029) and its filter query.

use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteRow;

use super::extractions::{Keyed, distinct, page};
use super::{instant, optional_signed, optional_text, read_instant, read_optional_u64, text};
use crate::domain::model_log::{
    LogCursor, LogPage, RewriteFacets, RewriteFilter, RewriteKind, RewriteLog, RewriteStage,
    page_size,
};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::sqlite::rows::{list as json_list, optional_instant};
use crate::infrastructure::store::sqlite::schedule::store_error;

const COLUMNS: &str = "r.id, r.at, r.kind, r.stage, r.context, r.verdict, r.rule, r.code, \
    r.latency_ms, r.model, r.reasoning, r.prompt_tokens, r.completion_tokens, \
    r.reasoning_tokens, r.reservation, r.budget, r.max_output_tokens, r.seed, r.reply, \
    r.reasoning_content, r.line, r.request_id, r.prompt";

/// [`COLUMNS`] without the reasoning and prompt texts (list pages).
const LIST_COLUMNS: &str = "r.id, r.at, r.kind, r.stage, r.context, r.verdict, r.rule, r.code, \
    r.latency_ms, r.model, r.reasoning, r.prompt_tokens, r.completion_tokens, \
    r.reasoning_tokens, r.reservation, r.budget, r.max_output_tokens, r.seed, r.reply, \
    NULL AS reasoning_content, r.line, r.request_id, NULL AS prompt";

fn log_of(row: &SqliteRow) -> Result<RewriteLog, StoreError> {
    let kind = text(row, "kind")?;
    let stage = text(row, "stage")?;
    Ok(RewriteLog {
        id: text(row, "id")?,
        at: read_instant(row, "at")?,
        kind: RewriteKind::parse(&kind)
            .ok_or_else(|| StoreError::Backend(format!("rewrite kind {kind}")))?,
        stage: RewriteStage::parse(&stage)
            .ok_or_else(|| StoreError::Backend(format!("rewrite stage {stage}")))?,
        context: optional_text(row, "context")?,
        verdict: text(row, "verdict")?,
        rule: optional_text(row, "rule")?,
        code: optional_text(row, "code")?,
        latency_ms: read_optional_u64(row, "latency_ms")?,
        model: optional_text(row, "model")?,
        reasoning: optional_text(row, "reasoning")?,
        prompt_tokens: read_optional_u64(row, "prompt_tokens")?,
        completion_tokens: read_optional_u64(row, "completion_tokens")?,
        reasoning_tokens: read_optional_u64(row, "reasoning_tokens")?,
        reservation: read_optional_u64(row, "reservation")?,
        budget: read_optional_u64(row, "budget")?,
        max_output_tokens: read_optional_u64(row, "max_output_tokens")?,
        seed: text(row, "seed")?,
        reply: optional_text(row, "reply")?,
        reasoning_content: optional_text(row, "reasoning_content")?,
        line: optional_text(row, "line")?,
        request_id: optional_text(row, "request_id")?,
        prompt: optional_text(row, "prompt")?,
    })
}

pub(super) async fn insert(
    conn: &mut SqliteConnection,
    log: &RewriteLog,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO rewrites (id, at, kind, stage, context, verdict, rule, code, latency_ms, \
         model, reasoning, prompt_tokens, completion_tokens, reasoning_tokens, reservation, \
         budget, max_output_tokens, seed, reply, reasoning_content, line, request_id, prompt) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
         ?18, ?19, ?20, ?21, ?22, ?23)",
    )
    .bind(&log.id)
    .bind(instant(&log.at)?)
    .bind(log.kind.as_str())
    .bind(log.stage.as_str())
    .bind(&log.context)
    .bind(&log.verdict)
    .bind(&log.rule)
    .bind(&log.code)
    .bind(optional_signed(log.latency_ms, "latency_ms")?)
    .bind(&log.model)
    .bind(&log.reasoning)
    .bind(optional_signed(log.prompt_tokens, "prompt_tokens")?)
    .bind(optional_signed(log.completion_tokens, "completion_tokens")?)
    .bind(optional_signed(log.reasoning_tokens, "reasoning_tokens")?)
    .bind(optional_signed(log.reservation, "reservation")?)
    .bind(optional_signed(log.budget, "budget")?)
    .bind(optional_signed(log.max_output_tokens, "max_output_tokens")?)
    .bind(&log.seed)
    .bind(&log.reply)
    .bind(&log.reasoning_content)
    .bind(&log.line)
    .bind(&log.request_id)
    .bind(&log.prompt)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

pub(super) async fn load(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<RewriteLog>, StoreError> {
    sqlx::query(&format!("SELECT {COLUMNS} FROM rewrites r WHERE r.id = ?1"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?
        .as_ref()
        .map(log_of)
        .transpose()
}

/// A bounded newest-first walk of `rewrites_recent` (as the other logs; pinned
/// by `plans::lists_walk_the_time_index_without_sorting`).
pub(super) fn list_sql() -> String {
    format!(
        "SELECT {LIST_COLUMNS} FROM rewrites r \
         WHERE (?1 IS NULL OR r.model = ?1) \
         AND (?2 IS NULL OR r.at >= ?2) \
         AND (?3 IS NULL OR r.at < ?3) \
         AND (?4 IS NULL OR r.verdict IN (SELECT value FROM json_each(?4))) \
         AND (?5 IS NULL OR r.kind = ?5) \
         AND (?6 IS NULL OR r.stage = ?6) \
         AND (?7 IS NULL OR instr(lower(r.seed), lower(?7)) > 0 \
              OR instr(lower(coalesce(r.reply, '')), lower(?7)) > 0 \
              OR instr(lower(coalesce(r.line, '')), lower(?7)) > 0 \
              OR instr(lower(coalesce(r.context, '')), lower(?7)) > 0 \
              OR instr(lower(coalesce(r.rule, '')), lower(?7)) > 0 \
              OR instr(lower(coalesce(r.code, '')), lower(?7)) > 0) \
         AND (?8 IS NULL OR r.at < ?8 OR (r.at = ?8 AND r.id < ?9)) \
         ORDER BY r.at DESC, r.id DESC LIMIT ?10"
    )
}

pub(super) const FACET_SQL: [&str; 4] = [
    "SELECT DISTINCT model FROM rewrites WHERE model IS NOT NULL ORDER BY 1",
    "SELECT DISTINCT kind FROM rewrites ORDER BY 1",
    "SELECT DISTINCT stage FROM rewrites ORDER BY 1",
    "SELECT DISTINCT verdict FROM rewrites ORDER BY 1",
];

impl Keyed for RewriteLog {
    fn cursor(&self) -> LogCursor {
        RewriteLog::cursor(self)
    }
}

pub(super) async fn list(
    conn: &mut SqliteConnection,
    filter: &RewriteFilter,
) -> Result<LogPage<RewriteLog>, StoreError> {
    let size = page_size(filter.limit);
    let verdicts = (!filter.verdicts.is_empty()).then(|| json_list(&filter.verdicts));
    let rows = sqlx::query(&list_sql())
        .bind(&filter.model)
        .bind(optional_instant(filter.from.as_ref())?)
        .bind(optional_instant(filter.to.as_ref())?)
        .bind(verdicts)
        .bind(filter.kind.map(RewriteKind::as_str))
        .bind(filter.stage.map(RewriteStage::as_str))
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

pub(super) async fn facets(conn: &mut SqliteConnection) -> Result<RewriteFacets, StoreError> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rewrites")
        .fetch_one(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(RewriteFacets {
        total: u64::try_from(total).unwrap_or_default(),
        models: distinct(conn, FACET_SQL[0]).await?,
        kinds: distinct(conn, FACET_SQL[1]).await?,
        stages: distinct(conn, FACET_SQL[2]).await?,
        verdicts: distinct(conn, FACET_SQL[3]).await?,
    })
}
