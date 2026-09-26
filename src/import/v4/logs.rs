//! Chat and extraction logs plus the watched messages they reference,
//! mapped onto the v5 model-log rows. Imported logs keep the v4 id behind a
//! `v4-` prefix, so a re-run finds them and they are recognisable.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::read::{V4Chat, V4Extraction, V4Message};
use crate::domain::model_log::{
    ChatInteraction, ChatOutcome, ChatRound, ExtractionLog, ExtractionOutcome, WatchedMessage,
};
use crate::domain::time::from_iso;

pub fn log_id(v4_id: &str) -> String {
    format!("v4-{v4_id}")
}

pub fn instant(text: Option<&str>) -> Option<DateTime<Utc>> {
    from_iso(text?).ok()
}

fn count(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

/// A JSON array of ids as strings (v4 `_dump(str(..))`); `None` when unreadable.
pub fn id_list(text: Option<&str>) -> Option<Vec<String>> {
    let Value::Array(items) = serde_json::from_str(text.unwrap_or("[]")).ok()? else {
        return None;
    };
    items
        .into_iter()
        .map(|item| match item {
            Value::String(text) => Some(text),
            Value::Number(number) if number.is_u64() => Some(number.to_string()),
            _ => None,
        })
        .collect()
}

fn array(text: Option<&str>) -> Vec<Value> {
    match text.map(serde_json::from_str::<Value>) {
        Some(Ok(Value::Array(items))) => items,
        _ => Vec::new(),
    }
}

/// v4 `answered` stays answered; `failed` is `error` when v4 recorded an
/// error, else (an empty reply) `unknown`, as is anything else.
fn chat_outcome(row: &V4Chat) -> ChatOutcome {
    match row.outcome.as_deref() {
        Some("answered") => ChatOutcome::Answered,
        Some("failed") if row.error.as_deref().is_some_and(|e| !e.is_empty()) => ChatOutcome::Error,
        _ => ChatOutcome::Unknown,
    }
}

/// v4 `model_rounds` (`{round, content, thinking, requested_tools}`) as v5
/// rounds, each with the tool calls v4 traced for its round (their names
/// stand in for an empty or missing `requested_tools`). Calls whose
/// round is unknown go to the last round; with no rounds recorded, one round
/// carries them all so the model and tool filters still match.
fn rounds(row: &V4Chat, model: &str) -> Vec<ChatRound> {
    let calls = array(row.tool_calls.as_deref());
    let recorded: Vec<Value> = array(row.model_rounds.as_deref())
        .into_iter()
        .filter(Value::is_object)
        .collect();
    let round_of = |value: &Value| value.get("round").and_then(Value::as_i64);
    let numbers: Vec<Option<i64>> = recorded.iter().map(round_of).collect();
    let mut placed: Vec<Vec<Value>> = vec![Vec::new(); recorded.len().max(1)];
    for call in calls {
        let at = round_of(&call)
            .and_then(|round| numbers.iter().position(|n| *n == Some(round)))
            .unwrap_or(placed.len() - 1);
        placed[at].push(call);
    }
    if recorded.is_empty() {
        let calls = placed.pop().unwrap_or_default();
        if calls.is_empty() && model.is_empty() {
            return Vec::new();
        }
        return vec![ChatRound {
            model: model.to_owned(),
            reasoning: None,
            finish_reason: None,
            latency_ms: None,
            tool_bundles: Vec::new(),
            tools: names(&calls),
            tool_calls: Value::Array(calls),
            response: None,
            route: None,
            clean: false,
        }];
    }
    recorded
        .iter()
        .zip(placed)
        .map(|(round, calls)| {
            let requested: Vec<String> = round
                .get("requested_tools")
                .and_then(Value::as_array)
                .map(|tools| {
                    tools
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                // v4 left `requested_tools` empty on some rounds that placed calls.
                .filter(|tools| !tools.is_empty())
                .unwrap_or_else(|| names(&calls));
            ChatRound {
                model: model.to_owned(),
                reasoning: None,
                finish_reason: None,
                latency_ms: None,
                tool_bundles: Vec::new(),
                tools: requested,
                tool_calls: Value::Array(calls),
                response: round
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                route: None,
                clean: false,
            }
        })
        .collect()
}

/// The calls' tool names, deduplicated in first-call order.
fn names(calls: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for name in calls
        .iter()
        .filter_map(|call| call.get("name").and_then(Value::as_str))
    {
        if !seen.iter().any(|known| known == name) {
            seen.push(name.to_owned());
        }
    }
    seen
}

pub fn chat(row: &V4Chat, at: DateTime<Utc>) -> ChatInteraction {
    let model = row.model.clone().unwrap_or_default();
    ChatInteraction {
        id: log_id(&row.id),
        at,
        channel_id: row.channel_id.clone(),
        message_id: row.message_id.clone(),
        member_id: row.author_id.clone(),
        question: row.question.clone().unwrap_or_default(),
        reply: row.reply.clone().unwrap_or_default(),
        outcome: chat_outcome(row),
        error: row.error.clone().filter(|error| !error.is_empty()),
        clean_retry: false,
        withheld: false,
        guardrail: json!({}),
        request_count: count(row.rounds)
            .and_then(|rounds| u32::try_from(rounds).ok())
            .unwrap_or(0),
        latency_ms: count(row.latency_ms),
        model_ms: count(row.model_ms),
        tools_ms: count(row.tools_ms),
        prompt_tokens: count(row.prompt_tokens),
        completion_tokens: count(row.completion_tokens),
        rounds: rounds(row, &model),
        persona: None,
        profile: None,
        profile_source: None,
        error_code: None,
    }
}

/// Amendments present → `proposed`; none with a JSON response →
/// `no_change`; unreadable ids or a non-JSON response (v4 logged failures
/// as their error text) → `unknown`. The channel is the messages' one
/// channel, and the members their authors, when the snapshot has them.
pub fn extraction(
    row: &V4Extraction,
    at: DateTime<Utc>,
    messages: &BTreeMap<String, &V4Message>,
) -> ExtractionLog {
    let message_ids = id_list(row.message_ids.as_deref());
    let proposal_ids = id_list(row.amendment_ids.as_deref());
    let raw_response = row.raw_response.clone().unwrap_or_default();
    let outcome = match (&message_ids, &proposal_ids) {
        (Some(_), Some(proposals)) if !proposals.is_empty() => ExtractionOutcome::Proposed,
        (Some(_), Some(_)) if serde_json::from_str::<Value>(&raw_response).is_ok() => {
            ExtractionOutcome::NoChange
        }
        _ => ExtractionOutcome::Unknown,
    };
    let message_ids = message_ids.unwrap_or_default();
    let read: Vec<&V4Message> = message_ids
        .iter()
        .filter_map(|id| messages.get(id).copied())
        .collect();
    let channels: BTreeSet<&str> = read
        .iter()
        .filter_map(|message| message.channel_id.as_deref())
        .collect();
    let member_ids: BTreeSet<String> = read
        .iter()
        .filter_map(|message| message.author_id.clone())
        .collect();
    ExtractionLog {
        id: log_id(&row.id),
        at,
        channel_id: (channels.len() == 1)
            .then(|| channels.first().map(|channel| (*channel).to_owned()))
            .flatten(),
        member_ids: member_ids.into_iter().collect(),
        model: row.model.clone().unwrap_or_default(),
        reasoning: None,
        prompt: row.prompt.clone().unwrap_or_default(),
        raw_response,
        latency_ms: count(row.latency_ms),
        request_count: 1,
        outcome,
        error: None,
        guardrail: json!({}),
        message_ids,
        proposal_ids: proposal_ids.unwrap_or_default(),
        refusals: Vec::new(),
    }
}

/// A referenced message, always marked processed (v4's time, else the
/// import's) so the v5 extractor never reads it as new backlog.
pub fn message(row: &V4Message, now: DateTime<Utc>) -> Option<WatchedMessage> {
    Some(WatchedMessage {
        id: row.id.clone(),
        channel_id: row.channel_id.clone().filter(|id| !id.is_empty())?,
        author_id: row.author_id.clone().filter(|id| !id.is_empty())?,
        created_at: instant(row.created_at.as_deref())?,
        edited_at: None,
        content: row.content.clone()?,
        processed_at: Some(instant(row.processed_at.as_deref()).unwrap_or(now)),
    })
}
