//! `chat.json`: the Chat log list, its per-model summary and the turn.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use super::{LogFacets, Names, UsageSummary, UsageTally};
use crate::{
    api::dto::{Named, iso_instant},
    chat::context::WITHHELD,
    domain::{
        members::Directory,
        model_log::{ChatInteraction, ChatOutcome, MaskedTurn},
        proposals::StoredCard,
    },
};

/// No model ran (a rate-limited question has no rounds).
const NO_MODEL: &str = "—";

/// A mapped member with no known name (never their id).
const UNNAMED: &str = "someone";

/// Query params of `GET /api/admin/chat`, all optional and combinable:
/// `model`, `from`/`to` (guild-local YYYY-MM-DD), `outcome` (comma-separated,
/// any of), `channel`, `member`, `q`, `tool` and `min_ms`. Unknown outcomes or
/// malformed dates: 422 invalid_filter.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Chat {
    /// Per model, over the filtered rows.
    pub summary: Vec<ChatSummary>,
    pub rows: Vec<ChatRow>,
    pub total: u64,
    pub facets: LogFacets,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChatRow {
    pub id: String,
    pub at: String,
    pub member: Named,
    /// The full Discord id, even when `member.name` is a placeholder.
    #[cfg_attr(test, ts(optional = nullable))]
    pub member_id: Option<String>,
    pub channel: Option<String>,
    pub channel_id: String,
    /// The first round's alias ("—" when no model was called).
    pub model: String,
    /// Model alias per request round.
    pub models: Vec<String>,
    pub latency_ms: u64,
    #[cfg_attr(test, ts(type = "ChatOutcome"))]
    pub outcome: &'static str,
    pub asked: String,
    pub tools_used: Vec<String>,
    /// Turn totals as logged (v4 imports may carry one); null = not reported.
    #[cfg_attr(test, ts(optional = nullable))]
    pub prompt_tokens: Option<u64>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub completion_tokens: Option<u64>,
    /// Sum of the rounds' reported reasoning counts; null = unknown.
    #[cfg_attr(test, ts(optional = nullable))]
    pub reasoning_tokens: Option<u64>,
}

/// Per model; usage fields come from this model's round rows, never the
/// turn totals.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChatSummary {
    pub model: String,
    pub count: usize,
    pub answered: usize,
    pub refused: usize,
    pub errors: usize,
    pub p50_ms: u64,
    pub tool_calls: usize,
    #[serde(flatten)]
    pub usage: UsageSummary,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChatToolCall {
    /// The request round (1-based index into `rounds`) whose reply asked for it.
    pub round: usize,
    pub name: String,
    pub arguments: String,
    pub result: String,
    /// Wall time; null when unknown (0 is a real 0 ms).
    pub took_ms: Option<u64>,
    pub outcome: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RoundGuardrail {
    pub clean: bool,
    pub content_filter: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChatRoundFacts {
    pub round: usize,
    pub requested_tools: Vec<String>,
    pub finish: String,
    /// Alias the request named, as sent.
    pub model: String,
    /// Reasoning effort as sent (after capability shaping); null when none went out.
    pub effort: Option<String>,
    #[cfg_attr(test, ts(type = "ChatRoute | null"))]
    pub route: Option<String>,
    /// null when unknown.
    pub latency_ms: Option<u64>,
    /// Provider-reported usage for this request (both or neither); null = not reported.
    #[cfg_attr(test, ts(optional = nullable))]
    pub prompt_tokens: Option<u64>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub completion_tokens: Option<u64>,
    /// The context budget's estimate, completion reserve excluded.
    #[cfg_attr(test, ts(optional = nullable))]
    pub prompt_estimate: Option<u64>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub reasoning_content: Option<String>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub reasoning_tokens: Option<u64>,
    pub guardrail: RoundGuardrail,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChatCard {
    pub kind: String,
    pub url: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MaskedRoundView {
    pub round: usize,
    pub clean: bool,
    /// The request messages exactly as sent (masked).
    #[cfg_attr(
        test,
        ts(type = "{ role: 'system' | 'user' | 'assistant' | 'tool'; [key: string]: unknown }[]")
    )]
    pub request: Value,
    /// The model's reply before names were restored.
    pub reply: Option<String>,
    /// Tool-call arguments before names were restored.
    #[cfg_attr(test, ts(type = "{ name: string; arguments: string }[]"))]
    pub tool_calls: Value,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct TokenName {
    pub token: String,
    pub name: String,
}

/// A pseudonymized turn as the model saw it (admin only).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ModelView {
    pub rounds: Vec<MaskedRoundView>,
    /// The decoded, finished reply members saw.
    pub reply: String,
    /// Fake name → member display name; never user ids.
    pub mapping: Vec<TokenName>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChatTurn {
    #[serde(flatten)]
    pub row: ChatRow,
    pub said: String,
    pub tools: Vec<ChatToolCall>,
    pub rounds: Vec<ChatRoundFacts>,
    pub cards: Vec<ChatCard>,
    pub raw: String,
    /// Persona bundle id; null when none answered (rate limited, imported).
    pub persona: Option<String>,
    /// Reply profile id; null for the bundle default voice.
    pub profile: Option<String>,
    #[cfg_attr(test, ts(type = "'saved' | 'role' | 'default' | null"))]
    pub profile_source: Option<String>,
    /// The turn's route (its last round's); null when no model ran.
    #[cfg_attr(test, ts(type = "ChatRoute | null"))]
    pub route: Option<String>,
    pub error: Option<String>,
    /// Stable code: timeout, malformed, content_blocked, identity_leak_blocked, rate_limited, …
    pub error_code: Option<String>,
    /// content_filter, external_unmasked, pseudonymized, identity_leak_blocked {role, kinds, count}, …
    #[cfg_attr(test, ts(type = "Record<string, unknown>"))]
    pub guardrail: Value,
    /// Pseudonymized, with a stored Model view.
    pub masked: bool,
    /// Null for passthrough and withheld turns.
    pub model_view: Option<ModelView>,
}

/// Round aliases in first-use order.
fn models(chat: &ChatInteraction) -> Vec<&str> {
    let mut seen = Vec::new();
    for round in &chat.rounds {
        if !seen.contains(&round.model.as_str()) {
            seen.push(round.model.as_str());
        }
    }
    seen
}

fn tools_used(chat: &ChatInteraction) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for tool in chat.rounds.iter().flat_map(|round| &round.tools) {
        if !seen.contains(tool) {
            seen.push(tool.clone());
        }
    }
    seen
}

fn tool_calls(chat: &ChatInteraction) -> usize {
    chat.rounds.iter().map(|round| round.tools.len()).sum()
}

pub fn asked(chat: &ChatInteraction) -> &str {
    if chat.withheld {
        WITHHELD
    } else {
        &chat.question
    }
}

pub fn chat_row(names: &Names<'_>, chat: &ChatInteraction) -> ChatRow {
    let models = models(chat);
    ChatRow {
        id: chat.id.clone(),
        at: iso_instant(chat.at),
        member: names.member(chat.member_id.as_deref()),
        // The app resolves `user <short id>` placeholders once the roster fills.
        member_id: chat.member_id.clone(),
        channel: names.channel(chat.channel_id.as_deref()),
        channel_id: chat.channel_id.clone().unwrap_or_default(),
        model: models.first().copied().unwrap_or(NO_MODEL).to_owned(),
        models: models.into_iter().map(str::to_owned).collect(),
        latency_ms: chat.latency_ms.unwrap_or_default(),
        outcome: chat.outcome.as_str(),
        asked: asked(chat).to_owned(),
        tools_used: tools_used(chat),
        // Turn totals as logged; counts only, so shown for withheld turns too.
        prompt_tokens: chat.prompt_tokens,
        completion_tokens: chat.completion_tokens,
        reasoning_tokens: chat
            .rounds
            .iter()
            .filter_map(|round| round.reasoning_tokens)
            .fold(None::<u64>, |sum, count| {
                Some(sum.unwrap_or_default().saturating_add(count))
            }),
    }
}

/// Proposal ids a turn's tool calls created (their cards link back).
pub fn created_proposals(chat: &ChatInteraction) -> Vec<String> {
    let mut ids = Vec::new();
    for call in chat
        .rounds
        .iter()
        .filter_map(|round| round.tool_calls.as_array())
        .flatten()
    {
        for id in call
            .get("created")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_owned());
            }
        }
    }
    ids
}

fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// A call's wall time: v5 `took_ms`, else v4's `ms`; `null` when unknown
/// (absent or not a non-negative integer), so 0 ms stays distinguishable.
fn took_ms(call: &Value) -> Option<u64> {
    call.get("took_ms")
        .or_else(|| call.get("ms"))
        .and_then(Value::as_u64)
}

/// The masked turn's Model view: each round's request exactly as the model
/// received it, its raw reply and tool-call arguments before decoding, the
/// decoded final reply, and token → display name (never a user id). `None`
/// for a withheld turn (its requests quote the question).
fn model_view(names: &Names<'_>, chat: &ChatInteraction, masked: &MaskedTurn) -> Option<ModelView> {
    if chat.withheld {
        return None;
    }
    Some(ModelView {
        rounds: masked
            .rounds
            .iter()
            .enumerate()
            // The logged position, as `rounds[].round` and `tools[].round`
            // (both lists hold one entry per answered request, in order).
            .map(|(index, round)| MaskedRoundView {
                round: index + 1,
                clean: round.clean,
                request: round.request.clone(),
                reply: round.reply.clone(),
                tool_calls: round.tool_calls.clone(),
            })
            .collect(),
        reply: masked.reply.clone(),
        mapping: masked
            .mapping
            .iter()
            .map(|name| TokenName {
                token: name.token.clone(),
                name: name
                    .display_name
                    .clone()
                    .or_else(|| {
                        names
                            .roster
                            .member(&name.user_id)
                            .and_then(|m| m.name().map(str::to_owned))
                    })
                    .unwrap_or_else(|| UNNAMED.to_owned()),
            })
            .collect(),
    })
}

fn turn(
    names: &Names<'_>,
    chat: &ChatInteraction,
    cards: &[StoredCard],
    guild_id: Option<&str>,
    masked: Option<&MaskedTurn>,
) -> ChatTurn {
    let withheld = |shown: String| {
        if chat.withheld {
            WITHHELD.to_owned()
        } else {
            shown
        }
    };
    let tools = chat
        .rounds
        .iter()
        .enumerate()
        .filter_map(|(index, round)| Some((index, round.tool_calls.as_array()?)))
        .flat_map(|(index, calls)| calls.iter().map(move |call| (index, call)))
        .map(|(index, call)| ChatToolCall {
            round: index + 1,
            name: text(call.get("name")),
            arguments: withheld(text(call.get("arguments"))),
            result: withheld(text(call.get("result").or_else(|| call.get("output")))),
            took_ms: took_ms(call),
            outcome: text(call.get("outcome")),
        })
        .collect();
    let rounds = chat
        .rounds
        .iter()
        .enumerate()
        .map(|(index, round)| ChatRoundFacts {
            round: index + 1,
            requested_tools: round.tools.clone(),
            finish: round.finish_reason.clone().unwrap_or_default(),
            model: round.model.clone(),
            effort: round.reasoning.clone(),
            route: round.route.clone(),
            latency_ms: round.latency_ms,
            prompt_tokens: round.prompt_tokens,
            completion_tokens: round.completion_tokens,
            prompt_estimate: round.prompt_estimate,
            reasoning_content: round
                .reasoning_content
                .as_ref()
                .map(|content| withheld(content.clone())),
            reasoning_tokens: round.reasoning_tokens,
            guardrail: RoundGuardrail {
                clean: round.clean,
                content_filter: round.finish_reason.as_deref() == Some("content_filter"),
            },
        })
        .collect();
    let cards = cards
        .iter()
        .filter_map(|card| {
            let message = card.message_id.as_deref()?;
            Some(ChatCard {
                kind: "proposal".to_owned(),
                url: format!(
                    "https://discord.com/channels/{}/{}/{message}",
                    guild_id?, card.channel_id
                ),
            })
        })
        .collect();
    let raw = withheld(
        chat.rounds
            .iter()
            .filter_map(|round| round.response.as_deref())
            .filter(|response| !response.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    ChatTurn {
        row: chat_row(names, chat),
        said: chat.reply.clone(),
        tools,
        rounds,
        cards,
        raw,
        persona: chat.persona.clone(),
        profile: chat.profile.clone(),
        profile_source: chat.profile_source.clone(),
        route: chat
            .rounds
            .iter()
            .rev()
            .find_map(|round| round.route.clone()),
        error: chat.error.clone(),
        error_code: chat.error_code.clone(),
        guardrail: chat.guardrail.clone(),
        masked: masked.is_some(),
        model_view: masked.and_then(|masked| model_view(names, chat, masked)),
    }
}

/// The turn: tool calls with their results (v5 `result`, v4 `output`) and
/// wall times, rounds, cards and the round responses, as [`ChatTurn`] JSON
/// (the shape the chat suites read).
pub fn chat_turn(
    names: &Names<'_>,
    chat: &ChatInteraction,
    cards: &[StoredCard],
    guild_id: Option<&str>,
    masked: Option<&MaskedTurn>,
) -> Value {
    serde_json::to_value(turn(names, chat, cards, guild_id, masked)).expect("ChatTurn is JSON")
}

/// Per model over the listed rows (as the mock): `errors` are `error` and
/// `timeout`, `p50_ms` the median latency of answered questions. Token usage
/// comes from the model's own round rows, never the turn totals.
pub fn chat_summary(rows: &[ChatInteraction]) -> Vec<ChatSummary> {
    let listed: BTreeSet<&str> = rows.iter().flat_map(models).collect();
    listed
        .into_iter()
        .map(|model| {
            let mine: Vec<&ChatInteraction> = rows
                .iter()
                .filter(|chat| models(chat).contains(&model))
                .collect();
            let count = |outcomes: &[ChatOutcome]| {
                mine.iter()
                    .filter(|chat| outcomes.contains(&chat.outcome))
                    .count()
            };
            let mut latencies: Vec<u64> = mine
                .iter()
                .filter(|chat| chat.outcome == ChatOutcome::Answered)
                .map(|chat| chat.latency_ms.unwrap_or_default())
                .collect();
            latencies.sort_unstable();
            let mut usage = UsageTally::default();
            for round in mine
                .iter()
                .flat_map(|chat| &chat.rounds)
                .filter(|round| round.model == model)
            {
                usage.add(
                    round.prompt_tokens,
                    round.completion_tokens,
                    round.prompt_estimate,
                );
            }
            ChatSummary {
                model: model.to_owned(),
                count: mine.len(),
                answered: count(&[ChatOutcome::Answered]),
                refused: count(&[ChatOutcome::Refused]),
                errors: count(&[ChatOutcome::Error, ChatOutcome::Timeout]),
                p50_ms: latencies
                    .get(latencies.len() / 2)
                    .copied()
                    .unwrap_or_default(),
                tool_calls: mine.iter().map(|chat| tool_calls(chat)).sum(),
                usage: usage.summary(),
            }
        })
        .collect()
}
