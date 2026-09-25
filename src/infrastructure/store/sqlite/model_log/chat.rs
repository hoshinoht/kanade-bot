//! The chat log (interactions, rounds, the derived `chat_tools`) and its
//! filter query.

use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};

use super::extractions::{Keyed, distinct, page};
use super::{
    instant, json_text, optional_signed, optional_text, read_instant, read_json, read_list,
    read_optional_u64, read_u64, text,
};
use crate::domain::model_log::{
    ChatFilter, ChatInteraction, ChatOutcome, ChatRound, LogCursor, LogFacets, LogPage, page_size,
};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::sqlite::rows::{list as json_list, optional_instant};
use crate::infrastructure::store::sqlite::schedule::store_error;

const COLUMNS: &str = "c.id, c.at, c.channel_id, c.message_id, c.member_id, c.question, \
    c.reply, c.outcome, c.error, c.clean_retry, c.withheld, c.guardrail, c.request_count, \
    c.latency_ms, c.model_ms, c.tools_ms, c.prompt_tokens, c.completion_tokens";

const ROUND_COLUMNS: &str = "model, reasoning, finish_reason, latency_ms, tool_bundles, tools, \
    tool_calls, response";

impl Keyed for ChatInteraction {
    fn cursor(&self) -> LogCursor {
        LogCursor {
            at: self.at,
            id: self.id.clone(),
        }
    }
}

fn flag(row: &SqliteRow, column: &str) -> Result<bool, StoreError> {
    row.try_get(column)
        .map_err(|error| StoreError::Backend(format!("chat_interactions.{column}: {error}")))
}

fn interaction_of(row: &SqliteRow) -> Result<ChatInteraction, StoreError> {
    let outcome = text(row, "outcome")?;
    Ok(ChatInteraction {
        id: text(row, "id")?,
        at: read_instant(row, "at")?,
        channel_id: optional_text(row, "channel_id")?,
        message_id: optional_text(row, "message_id")?,
        member_id: optional_text(row, "member_id")?,
        question: text(row, "question")?,
        reply: text(row, "reply")?,
        outcome: ChatOutcome::parse(&outcome)
            .ok_or_else(|| StoreError::Backend(format!("chat outcome {outcome}")))?,
        error: optional_text(row, "error")?,
        clean_retry: flag(row, "clean_retry")?,
        withheld: flag(row, "withheld")?,
        guardrail: read_json(row, "guardrail")?,
        request_count: u32::try_from(read_u64(row, "request_count")?).map_err(|error| {
            StoreError::Backend(format!("chat_interactions.request_count: {error}"))
        })?,
        latency_ms: read_optional_u64(row, "latency_ms")?,
        model_ms: read_optional_u64(row, "model_ms")?,
        tools_ms: read_optional_u64(row, "tools_ms")?,
        prompt_tokens: read_optional_u64(row, "prompt_tokens")?,
        completion_tokens: read_optional_u64(row, "completion_tokens")?,
        rounds: Vec::new(),
    })
}

fn round_of(row: &SqliteRow) -> Result<ChatRound, StoreError> {
    Ok(ChatRound {
        model: text(row, "model")?,
        reasoning: optional_text(row, "reasoning")?,
        finish_reason: optional_text(row, "finish_reason")?,
        latency_ms: read_optional_u64(row, "latency_ms")?,
        tool_bundles: read_list(row, "tool_bundles")?,
        tools: read_list(row, "tools")?,
        tool_calls: read_json(row, "tool_calls")?,
        response: optional_text(row, "response")?,
    })
}

async fn with_rounds(
    conn: &mut SqliteConnection,
    mut interaction: ChatInteraction,
) -> Result<ChatInteraction, StoreError> {
    let rows = sqlx::query(&format!(
        "SELECT {ROUND_COLUMNS} FROM chat_rounds WHERE interaction_id = ?1 ORDER BY ord"
    ))
    .bind(&interaction.id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    interaction.rounds = rows.iter().map(round_of).collect::<Result<_, _>>()?;
    Ok(interaction)
}

pub(super) async fn insert(
    conn: &mut SqliteConnection,
    chat: &ChatInteraction,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO chat_interactions (id, at, channel_id, message_id, member_id, question, \
         reply, outcome, error, clean_retry, withheld, guardrail, request_count, latency_ms, \
         model_ms, tools_ms, prompt_tokens, completion_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
         ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
    )
    .bind(&chat.id)
    .bind(instant(&chat.at)?)
    .bind(&chat.channel_id)
    .bind(&chat.message_id)
    .bind(&chat.member_id)
    .bind(&chat.question)
    .bind(&chat.reply)
    .bind(chat.outcome.as_str())
    .bind(&chat.error)
    .bind(chat.clean_retry)
    .bind(chat.withheld)
    .bind(json_text(&chat.guardrail))
    .bind(i64::from(chat.request_count))
    .bind(optional_signed(chat.latency_ms, "latency_ms")?)
    .bind(optional_signed(chat.model_ms, "model_ms")?)
    .bind(optional_signed(chat.tools_ms, "tools_ms")?)
    .bind(optional_signed(chat.prompt_tokens, "prompt_tokens")?)
    .bind(optional_signed(
        chat.completion_tokens,
        "completion_tokens",
    )?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    for (ord, round) in chat.rounds.iter().enumerate() {
        sqlx::query(
            "INSERT INTO chat_rounds (interaction_id, ord, model, reasoning, finish_reason, \
             latency_ms, tool_bundles, tools, tool_calls, response) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(&chat.id)
        .bind(i64::try_from(ord).map_err(|_| StoreError::Constraint("too many rounds".into()))?)
        .bind(&round.model)
        .bind(&round.reasoning)
        .bind(&round.finish_reason)
        .bind(optional_signed(round.latency_ms, "latency_ms")?)
        .bind(json_list(&round.tool_bundles))
        .bind(json_list(&round.tools))
        .bind(json_text(&round.tool_calls))
        .bind(&round.response)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
        sqlx::query(
            "INSERT OR IGNORE INTO chat_tools (interaction_id, tool) \
             SELECT ?1, value FROM json_each(?2)",
        )
        .bind(&chat.id)
        .bind(json_list(&round.tools))
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    }
    Ok(())
}

pub(super) async fn load(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ChatInteraction>, StoreError> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM chat_interactions c WHERE c.id = ?1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(with_rounds(conn, interaction_of(&row)?).await?)),
    }
}

/// The rounds of a page's interactions (ids as a JSON array), in order.
pub(super) fn rounds_sql() -> String {
    format!(
        "SELECT interaction_id, {ROUND_COLUMNS} FROM chat_rounds \
         WHERE interaction_id IN (SELECT value FROM json_each(?1)) \
         ORDER BY interaction_id, ord"
    )
}

/// A bounded scan of `chat_recent` newest first, as the extraction list.
pub(super) fn list_sql() -> String {
    format!(
        "SELECT {COLUMNS} FROM chat_interactions c \
         WHERE (?1 IS NULL OR EXISTS (SELECT 1 FROM chat_rounds r \
              WHERE r.interaction_id = c.id AND r.model = ?1)) \
         AND (?2 IS NULL OR c.at >= ?2) \
         AND (?3 IS NULL OR c.at < ?3) \
         AND (?4 IS NULL OR c.outcome IN (SELECT value FROM json_each(?4)) \
              OR (c.clean_retry = 1 AND 'clean_retry' IN (SELECT value FROM json_each(?4))) \
              OR (c.withheld = 1 AND 'withheld' IN (SELECT value FROM json_each(?4)))) \
         AND (?5 IS NULL OR c.channel_id = ?5) \
         AND (?6 IS NULL OR c.member_id = ?6) \
         AND (?7 IS NULL OR instr(lower(c.question), lower(?7)) > 0 \
              OR instr(lower(c.reply), lower(?7)) > 0) \
         AND (?8 IS NULL OR EXISTS (SELECT 1 FROM chat_tools t \
              WHERE t.interaction_id = c.id AND t.tool = ?8)) \
         AND (?9 IS NULL OR c.latency_ms >= ?9) \
         AND (?10 IS NULL OR c.at < ?10 OR (c.at = ?10 AND c.id < ?11)) \
         ORDER BY c.at DESC, c.id DESC LIMIT ?12"
    )
}

pub(super) async fn list(
    conn: &mut SqliteConnection,
    filter: &ChatFilter,
) -> Result<LogPage<ChatInteraction>, StoreError> {
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
    let rows = sqlx::query(&list_sql())
        .bind(&filter.model)
        .bind(optional_instant(filter.from.as_ref())?)
        .bind(optional_instant(filter.to.as_ref())?)
        .bind(outcomes)
        .bind(&filter.channel)
        .bind(&filter.member)
        .bind(&filter.q)
        .bind(&filter.tool)
        .bind(optional_signed(filter.min_ms, "min_ms")?)
        .bind(optional_instant(
            filter.cursor.as_ref().map(|cursor| &cursor.at),
        )?)
        .bind(filter.cursor.as_ref().map(|cursor| cursor.id.as_str()))
        .bind(i64::from(size) + 1)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?;
    let mut items = rows
        .iter()
        .map(interaction_of)
        .collect::<Result<Vec<_>, _>>()?;
    // One query for the page's rounds, not one per interaction.
    let ids = json_list(&items.iter().map(|chat| chat.id.clone()).collect::<Vec<_>>());
    let rounds = sqlx::query(&rounds_sql())
        .bind(ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?;
    let mut by_id: std::collections::HashMap<String, Vec<ChatRound>> =
        std::collections::HashMap::new();
    for row in &rounds {
        by_id
            .entry(text(row, "interaction_id")?)
            .or_default()
            .push(round_of(row)?);
    }
    for chat in &mut items {
        chat.rounds = by_id.remove(&chat.id).unwrap_or_default();
    }
    Ok(page(items, size))
}

pub(super) async fn facets(conn: &mut SqliteConnection) -> Result<LogFacets, StoreError> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_interactions")
        .fetch_one(&mut *conn)
        .await
        .map_err(store_error)?;
    Ok(LogFacets {
        total: u64::try_from(total).unwrap_or_default(),
        models: distinct(conn, FACET_SQL[0]).await?,
        tools: distinct(conn, FACET_SQL[1]).await?,
        outcomes: distinct(conn, FACET_SQL[2]).await?,
        channels: distinct(conn, FACET_SQL[3]).await?,
    })
}

pub(super) const FACET_SQL: [&str; 4] = [
    "SELECT DISTINCT model FROM chat_rounds ORDER BY 1",
    "SELECT DISTINCT tool FROM chat_tools ORDER BY 1",
    "SELECT outcome FROM chat_interactions \
     UNION SELECT 'clean_retry' FROM chat_interactions WHERE clean_retry = 1 \
     UNION SELECT 'withheld' FROM chat_interactions WHERE withheld = 1 \
     ORDER BY 1",
    "SELECT DISTINCT channel_id FROM chat_interactions WHERE channel_id IS NOT NULL ORDER BY 1",
];
