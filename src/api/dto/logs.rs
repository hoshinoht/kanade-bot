//! Chat and extraction log rows (`chat.json`, `extractions.json`). A
//! withheld chat question is never shown: the admin sees the placeholder the
//! model sees, and the model output and tool arguments of that turn are
//! withheld with it, as they may quote the question.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::iso_instant;
use crate::{
    api::state::ChannelEntry,
    chat::context::WITHHELD,
    domain::{
        drafts::{DraftStatus, LoadedDraft},
        ids::short_id,
        members::{Roster, member_name},
        model_log::{ChatInteraction, ChatOutcome, ExtractionLog, LogFacets, WatchedMessage},
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

/// The turn: tool calls (the log keeps no results or per-call timings, so
/// `result` is empty and `took_ms` 0), rounds, cards and the round responses.
pub fn chat_turn(
    names: &Names<'_>,
    chat: &ChatInteraction,
    cards: &[StoredCard],
    guild_id: Option<&str>,
) -> Value {
    let mut turn = chat_row(names, chat);
    let tools: Vec<Value> = chat
        .rounds
        .iter()
        .filter_map(|round| round.tool_calls.as_array())
        .flatten()
        .map(|call| {
            json!({
                "name": text(call.get("name")),
                "arguments": if chat.withheld { WITHHELD.to_owned() } else { text(call.get("arguments")) },
                "result": "",
                "took_ms": 0,
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
    turn
}

/// Per model over the listed rows (as the mock): `errors` are `error` and
/// `timeout`, `p50_ms` the median latency of answered questions.
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
            json!({
                "model": model,
                "count": mine.len(),
                "answered": count(&[ChatOutcome::Answered]),
                "refused": count(&[ChatOutcome::Refused]),
                "errors": count(&[ChatOutcome::Error, ChatOutcome::Timeout]),
                "p50_ms": latencies.get(latencies.len() / 2).copied().unwrap_or_default(),
                "tool_calls": mine.iter().map(|chat| tool_calls(chat)).sum::<usize>(),
            })
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
    })
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
