//! Chat and extraction log rows (`chat.json`, `extractions.json`). A
//! withheld chat question is never shown: the admin sees the placeholder the
//! model sees, and the model output and tool arguments and results of that
//! turn are withheld with it, as they may quote the question.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::iso_instant;
use crate::{
    api::state::ChannelEntry,
    chat::context::WITHHELD,
    domain::{
        drafts::{DraftStatus, LoadedDraft},
        ids::short_id,
        members::{Directory, Roster, member_name},
        model_log::{
            ChatInteraction, ChatOutcome, ExtractionLog, LogFacets, MaskedTurn, WatchedMessage,
        },
        proposals::StoredCard,
    },
    extract::pipeline::{HISTORY_UNREADABLE, SCHEDULE_UNREADABLE},
};

/// No model ran (a rate-limited question has no rounds).
const NO_MODEL: &str = "—";

pub struct Names<'a> {
    pub roster: &'a Roster,
    pub channels: &'a BTreeMap<String, ChannelEntry>,
}

impl Names<'_> {
    fn channel(&self, id: Option<&str>) -> Value {
        id.and_then(|id| self.channels.get(id))
            .map_or(Value::Null, |channel| json!(channel.name))
    }

    fn channel_name(&self, id: &str) -> String {
        self.channels
            .get(id)
            .map_or_else(|| id.to_owned(), |channel| channel.name.clone())
    }

    fn member(&self, id: Option<&str>) -> Value {
        match id {
            Some(id) => json!({"id": id, "name": member_name(self.roster, id)}),
            None => json!({"id": "", "name": "unknown"}),
        }
    }

    pub fn facets(&self, facets: &LogFacets) -> Value {
        json!({
            "models": facets.models,
            "tools": facets.tools,
            "outcomes": facets.outcomes,
            "channels": facets
                .channels
                .iter()
                .map(|id| json!({"id": id, "name": self.channel_name(id)}))
                .collect::<Vec<_>>(),
        })
    }
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

fn tools_used(chat: &ChatInteraction) -> Vec<&str> {
    let mut seen = Vec::new();
    for tool in chat.rounds.iter().flat_map(|round| &round.tools) {
        if !seen.contains(&tool.as_str()) {
            seen.push(tool.as_str());
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

pub fn chat_row(names: &Names<'_>, chat: &ChatInteraction) -> Value {
    let models = models(chat);
    json!({
        "id": chat.id,
        "at": iso_instant(chat.at),
        "member": names.member(chat.member_id.as_deref()),
        // The app resolves `user <short id>` placeholders once the roster fills.
        "member_id": chat.member_id,
        "channel": names.channel(chat.channel_id.as_deref()),
        "channel_id": chat.channel_id.as_deref().unwrap_or_default(),
        "model": models.first().copied().unwrap_or(NO_MODEL),
        "models": models,
        "latency_ms": chat.latency_ms.unwrap_or_default(),
        "outcome": chat.outcome.as_str(),
        "asked": asked(chat),
        "tools_used": tools_used(chat),
        // Turn totals as logged; counts only, so shown for withheld turns too.
        "prompt_tokens": chat.prompt_tokens,
        "completion_tokens": chat.completion_tokens,
    })
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
fn model_view(names: &Names<'_>, chat: &ChatInteraction, masked: &MaskedTurn) -> Value {
    if chat.withheld {
        return Value::Null;
    }
    json!({
        "rounds": masked
            .rounds
            .iter()
            .enumerate()
            // The logged position, as `rounds[].round` and `tools[].round`
            // (both lists hold one entry per answered request, in order).
            .map(|(index, round)| json!({
                "round": index + 1,
                "clean": round.clean,
                "request": round.request,
                "reply": round.reply,
                "tool_calls": round.tool_calls,
            }))
            .collect::<Vec<_>>(),
        "reply": masked.reply,
        "mapping": masked
            .mapping
            .iter()
            .map(|name| json!({
                "token": name.token,
                "name": name
                    .display_name
                    .clone()
                    .or_else(|| names.roster.member(&name.user_id).and_then(|m| m.name().map(str::to_owned)))
                    .unwrap_or_else(|| UNNAMED.to_owned()),
            }))
            .collect::<Vec<_>>(),
    })
}

/// A mapped member with no known name (never their id).
const UNNAMED: &str = "someone";

/// The turn: tool calls with their results (v5 `result`, v4 `output`) and
/// wall times, rounds, cards and the round responses.
pub fn chat_turn(
    names: &Names<'_>,
    chat: &ChatInteraction,
    cards: &[StoredCard],
    guild_id: Option<&str>,
    masked: Option<&MaskedTurn>,
) -> Value {
    let mut turn = chat_row(names, chat);
    let tools: Vec<Value> = chat
        .rounds
        .iter()
        .enumerate()
        .filter_map(|(index, round)| Some((index, round.tool_calls.as_array()?)))
        .flat_map(|(index, calls)| calls.iter().map(move |call| (index, call)))
        .map(|(index, call)| {
            json!({
                "round": index + 1,
                "name": text(call.get("name")),
                "arguments": if chat.withheld { WITHHELD.to_owned() } else { text(call.get("arguments")) },
                "result": if chat.withheld {
                    WITHHELD.to_owned()
                } else {
                    text(call.get("result").or_else(|| call.get("output")))
                },
                "took_ms": took_ms(call),
                "outcome": text(call.get("outcome")),
            })
        })
        .collect();
    let rounds: Vec<Value> = chat
        .rounds
        .iter()
        .enumerate()
        .map(|(index, round)| {
            json!({
                "round": index + 1,
                "requested_tools": round.tools,
                "finish": round.finish_reason.as_deref().unwrap_or_default(),
                "model": round.model,
                "effort": round.reasoning,
                "route": round.route,
                "latency_ms": round.latency_ms,
                "prompt_tokens": round.prompt_tokens,
                "completion_tokens": round.completion_tokens,
                "prompt_estimate": round.prompt_estimate,
                "guardrail": {
                    "clean": round.clean,
                    "content_filter": round.finish_reason.as_deref() == Some("content_filter"),
                },
            })
        })
        .collect();
    let cards: Vec<Value> = cards
        .iter()
        .filter_map(|card| {
            let message = card.message_id.as_deref()?;
            Some(json!({
                "kind": "proposal",
                "url": format!(
                    "https://discord.com/channels/{}/{}/{message}",
                    guild_id?,
                    card.channel_id
                ),
            }))
        })
        .collect();
    let raw = if chat.withheld {
        WITHHELD.to_owned()
    } else {
        chat.rounds
            .iter()
            .filter_map(|round| round.response.as_deref())
            .filter(|response| !response.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let object = turn.as_object_mut().expect("row object");
    object.insert("said".into(), json!(chat.reply));
    object.insert("tools".into(), json!(tools));
    object.insert("rounds".into(), json!(rounds));
    object.insert("cards".into(), json!(cards));
    object.insert("raw".into(), json!(raw));
    object.insert("persona".into(), json!(chat.persona));
    object.insert("profile".into(), json!(chat.profile));
    object.insert("profile_source".into(), json!(chat.profile_source));
    object.insert(
        "route".into(),
        json!(
            chat.rounds
                .iter()
                .rev()
                .find_map(|round| round.route.as_deref())
        ),
    );
    object.insert("error".into(), json!(chat.error));
    object.insert("error_code".into(), json!(chat.error_code));
    object.insert("guardrail".into(), chat.guardrail.clone());
    object.insert("masked".into(), json!(masked.is_some()));
    object.insert(
        "model_view".into(),
        masked.map_or(Value::Null, |masked| model_view(names, chat, masked)),
    );
    turn
}

/// Reported token usage over a set of logged requests: sums over the ones
/// that reported a pair (`null` when none did, never 0), how many did, and
/// the median of reported prompt tokens over the local estimate among those
/// that also have a non-zero estimate, rounded to two decimals.
#[derive(Default)]
struct UsageTally {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    reported: usize,
    ratios: Vec<f64>,
}

impl UsageTally {
    fn add(&mut self, prompt: Option<u64>, completion: Option<u64>, estimate: Option<u64>) {
        let (Some(prompt), Some(completion)) = (prompt, completion) else {
            return;
        };
        let sum = |total: &mut Option<u64>, value: u64| {
            *total = Some(total.unwrap_or_default().saturating_add(value));
        };
        sum(&mut self.prompt_tokens, prompt);
        sum(&mut self.completion_tokens, completion);
        self.reported += 1;
        if let Some(estimate) = estimate.filter(|estimate| *estimate > 0) {
            self.ratios.push(prompt as f64 / estimate as f64);
        }
    }

    fn est_ratio(&self) -> Option<f64> {
        let mut ratios = self.ratios.clone();
        ratios.sort_by(f64::total_cmp);
        let middle = ratios.len() / 2;
        let median = match ratios.len() {
            0 => return None,
            n if n % 2 == 1 => ratios[middle],
            _ => f64::midpoint(ratios[middle - 1], ratios[middle]),
        };
        Some((median * 100.0).round() / 100.0)
    }

    fn insert_into(&self, object: &mut serde_json::Map<String, Value>) {
        object.insert("prompt_tokens".into(), json!(self.prompt_tokens));
        object.insert("completion_tokens".into(), json!(self.completion_tokens));
        object.insert("reported".into(), json!(self.reported));
        object.insert("est_ratio".into(), json!(self.est_ratio()));
    }
}

/// Per model over the listed rows (as the mock): `errors` are `error` and
/// `timeout`, `p50_ms` the median latency of answered questions. Token usage
/// comes from the model's own round rows, never the turn totals.
pub fn chat_summary(rows: &[ChatInteraction]) -> Vec<Value> {
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
            let mut summary = json!({
                "model": model,
                "count": mine.len(),
                "answered": count(&[ChatOutcome::Answered]),
                "refused": count(&[ChatOutcome::Refused]),
                "errors": count(&[ChatOutcome::Error, ChatOutcome::Timeout]),
                "p50_ms": latencies.get(latencies.len() / 2).copied().unwrap_or_default(),
                "tool_calls": mine.iter().map(|chat| tool_calls(chat)).sum::<usize>(),
            });
            usage.insert_into(summary.as_object_mut().expect("summary object"));
            summary
        })
        .collect()
}

/// Per model over the listed calls: how many, and their reported usage
/// (each call's pair is summed over its reporting attempts).
pub fn extraction_summary(rows: &[ExtractionLog]) -> Vec<Value> {
    let listed: BTreeSet<&str> = rows.iter().map(|log| log.model.as_str()).collect();
    listed
        .into_iter()
        .map(|model| {
            let mine: Vec<&ExtractionLog> = rows.iter().filter(|log| log.model == model).collect();
            let mut usage = UsageTally::default();
            for log in &mine {
                usage.add(
                    log.prompt_tokens,
                    log.completion_tokens,
                    log.prompt_estimate,
                );
            }
            let mut summary = json!({"model": model, "count": mine.len()});
            usage.insert_into(summary.as_object_mut().expect("summary object"));
            summary
        })
        .collect()
}

/// Shown instead of a logged failure that is not a known typed text.
pub const CALL_FAILED: &str = "The call failed; the server log has the detail.";

/// Fixed texts the extractor logs (its own sentences, governor refusals,
/// session failures, schema and date errors).
fn fixed_failures() -> Vec<String> {
    use crate::infrastructure::llm::governor::Refused;
    let refused = [
        Refused::UnknownRole,
        Refused::Ungrouped,
        Refused::ExternalForbidden,
        Refused::MustNotWait,
        Refused::Busy,
        Refused::Timeout,
        Refused::Unavailable { retry_at: None },
        Refused::RateLimited {
            wait: std::time::Duration::ZERO,
        },
        Refused::RetryBudgetExhausted,
    ];
    [
        SCHEDULE_UNREADABLE,
        HISTORY_UNREADABLE,
        "no answer",
        "the extraction model is not configured",
        "the model returned an empty response",
        "model request cap reached",
        "model session has ended",
        "clean retry unavailable",
        "answer retry unavailable",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(refused.iter().map(ToString::to_string))
    .chain([crate::domain::time::DateOutOfRange.to_string()])
    .collect()
}

/// Typed texts with a variable part that is never store or backend text:
/// redacted provider errors, timeouts, schema validation and identity
/// decoding (model output), and the external-route refusal (config).
fn typed_failure(text: &str) -> bool {
    const PREFIXES: [&str; 5] = [
        "LLM completion failed (",
        "the model did not answer within ",
        "not JSON: ",
        "expected a JSON object, got ",
        "unknown identity token at byte ",
    ];
    let first = text.lines().next().unwrap_or_default();
    let validation = first.split_once(' ').is_some_and(|(count, rest)| {
        !count.is_empty()
            && count.bytes().all(|byte| byte.is_ascii_digit())
            && matches!(
                rest,
                "validation error for Extraction" | "validation errors for Extraction"
            )
    });
    PREFIXES.iter().any(|prefix| text.starts_with(prefix))
        || validation
        || (text.starts_with("role ") && text.ends_with(" but pseudonymization is off"))
}

/// The logged failure when it is a known typed text, else [`CALL_FAILED`]:
/// store and backend text (paths, SQLite messages), including rows written
/// before the extractor stopped logging it, never reaches the portal.
pub fn call_error(error: Option<&str>) -> Option<String> {
    let error = error?;
    Some(
        if typed_failure(error) || fixed_failures().iter().any(|known| known == error) {
            error.to_owned()
        } else {
            CALL_FAILED.to_owned()
        },
    )
}

fn extraction_common(names: &Names<'_>, log: &ExtractionLog) -> Value {
    json!({
        "id": log.id,
        "short_id": short_id(&log.id),
        "at": iso_instant(log.at),
        "model": log.model,
        "latency_ms": log.latency_ms,
        "channel": names.channel(log.channel_id.as_deref()),
        "channel_id": log.channel_id.as_deref().unwrap_or_default(),
        "error": call_error(log.error.as_deref()),
        "outcome": log.outcome.as_str(),
        "prompt_tokens": log.prompt_tokens,
        "completion_tokens": log.completion_tokens,
    })
}

/// The call's resolved context (`guardrail.context`) when it was logged
/// whole, else `null`.
fn call_context(log: &ExtractionLog) -> Value {
    let context = log.guardrail.get("context");
    let field = |name: &str| context.and_then(|context| context.get(name));
    match (
        field("window").and_then(Value::as_u64),
        field("reserve").and_then(Value::as_u64),
        field("source").and_then(Value::as_str),
    ) {
        (Some(window), Some(reserve), Some(source)) => {
            json!({"window": window, "reserve": reserve, "source": source})
        }
        _ => Value::Null,
    }
}

pub fn extraction_row(names: &Names<'_>, log: &ExtractionLog) -> Value {
    let mut row = extraction_common(names, log);
    let object = row.as_object_mut().expect("row object");
    object.insert("messages".into(), json!(log.message_ids.len()));
    object.insert("changes".into(), json!(log.proposal_ids.len()));
    row
}

/// A proposal's state in v4's words where it had one.
fn proposal_status(loaded: Option<&LoadedDraft>) -> &'static str {
    let Some(loaded) = loaded else {
        return "missing";
    };
    match loaded.draft.status {
        DraftStatus::Submitted | DraftStatus::Open => "proposed",
        DraftStatus::Merged => "confirmed",
        DraftStatus::Rejected => "rejected",
        DraftStatus::Expired => "expired",
        DraftStatus::Withdrawn => "withdrawn",
        DraftStatus::Discarded
            if loaded.draft.close_reason.as_deref() == Some(crate::domain::drafts::SUPERSEDED) =>
        {
            "superseded"
        }
        DraftStatus::Discarded => "discarded",
    }
}

pub struct Proposed<'a> {
    pub card: Option<&'a StoredCard>,
    pub draft: Option<&'a LoadedDraft>,
}

fn amendment(zone: chrono_tz::Tz, proposed: &Proposed<'_>) -> Value {
    let status = proposal_status(proposed.draft);
    let Some(card) = proposed.card else {
        let kind = proposed
            .draft
            .and_then(|loaded| loaded.draft.subject.as_deref())
            .and_then(crate::domain::proposals::ProposalSubject::parse)
            .map_or_else(
                || "change".to_owned(),
                |subject| subject.kind.as_str().to_owned(),
            );
        return json!({"kind": kind, "bosses": "", "when": "", "confidence": 0.0, "status": status});
    };
    let details = &card.details;
    let when = match details.new_datetime {
        Some(at) => super::when(at, zone),
        None => [details.day_ref.as_deref(), details.time_ref.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    };
    json!({
        "kind": details.kind.as_str(),
        "bosses": details.bosses.join(", "),
        "when": when,
        "confidence": details.confidence,
        "status": status,
    })
}

/// The call: prompt, raw response, what it proposed, the messages it read
/// (in the log's order; pruned ones are left out) and the changes refused
/// up front.
pub fn extraction(
    names: &Names<'_>,
    zone: chrono_tz::Tz,
    log: &ExtractionLog,
    proposed: &[Proposed<'_>],
    messages: &[WatchedMessage],
) -> Value {
    let mut detail = extraction_common(names, log);
    let read: Vec<Value> = log
        .message_ids
        .iter()
        .filter_map(|id| messages.iter().find(|message| &message.id == id))
        .map(|message| {
            json!({
                "id": message.id,
                "author": member_name(names.roster, &message.author_id),
                "author_id": message.author_id,
                "at": iso_instant(message.created_at),
                "content": message.content,
            })
        })
        .collect();
    let object = detail.as_object_mut().expect("detail object");
    object.insert("prompt".into(), json!(log.prompt));
    object.insert("raw_response".into(), json!(log.raw_response));
    object.insert("prompt_estimate".into(), json!(log.prompt_estimate));
    object.insert("context".into(), call_context(log));
    object.insert(
        "amendments".into(),
        json!(
            proposed
                .iter()
                .map(|proposed| amendment(zone, proposed))
                .collect::<Vec<_>>()
        ),
    );
    object.insert("messages".into(), json!(read));
    object.insert(
        "refusals".into(),
        json!(
            log.refusals
                .iter()
                .map(|refusal| refusal.to_json())
                .collect::<Vec<_>>()
        ),
    );
    detail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scheduler::StoreError;

    #[test]
    fn only_known_typed_failures_are_shown() {
        for shown in [
            "no answer",
            "model unavailable",
            SCHEDULE_UNREADABLE,
            "date value out of range",
            "LLM completion failed (InvalidOutput, digest=00000000000000ff)",
            "the model did not answer within 60s",
            "2 validation errors for Extraction\namendments.0.kind\n  Input should be ...",
            "role extraction routes to external model \"x\" but pseudonymization is off",
        ] {
            assert_eq!(call_error(Some(shown)).as_deref(), Some(shown), "{shown}");
        }
        for hidden in [
            StoreError::Backend("/private/var/db/kanade.sqlite3: disk I/O error".into())
                .to_string(),
            "schedule constraint violated: runs.id".into(),
            "v4: Traceback (most recent call last)".into(),
            "1 validation error for Extraction/private".into(),
        ] {
            assert_eq!(
                call_error(Some(&hidden)).as_deref(),
                Some(CALL_FAILED),
                "{hidden}"
            );
        }
        assert_eq!(call_error(None), None);
    }
}
