//! What one attempt leaves behind, read back for the record: the chat-log
//! row, extraction logs, what members saw, proposals with their cards, and
//! answer and schedule changes.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use kanade::{
    bot::transport::{Call, FakeDiscord, Outcome},
    domain::{
        drafts::ProposalStore,
        model_log::{ChatFilter, ChatInteraction, ExtractionFilter, ModelLogStore},
        proposals::ProposalCardStore,
        scheduler::{ScheduleStore, Scope},
    },
    infrastructure::store::SqliteStore,
};
use serde_json::{Value, json};
use twilight_model::channel::message::Embed;

use crate::world::{self, C1};

/// `Tue 13 Oct 22:00` in the guild zone.
pub fn label(at: DateTime<Utc>) -> String {
    let local = at.with_timezone(&world::ZONE);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{:?} {:02} {} {:02}:{:02}",
        local.weekday(),
        local.day(),
        MONTHS[local.month0() as usize],
        local.hour(),
        local.minute()
    )
}

pub fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

pub fn member(id: &str) -> String {
    id.parse()
        .map(world::name_of)
        .map_or_else(|_| id.to_owned(), str::to_owned)
}

pub async fn chat_row(store: &SqliteStore, message: u64) -> Option<ChatInteraction> {
    let page = store
        .list_chats(&ChatFilter {
            limit: 50,
            ..ChatFilter::default()
        })
        .await
        .ok()?;
    page.items
        .into_iter()
        .find(|row| row.message_id.as_deref() == Some(message.to_string().as_str()))
}

pub fn chat_json(row: &ChatInteraction) -> Value {
    json!({
        "id": row.id,
        "outcome": row.outcome.as_str(),
        "error": row.error,
        "error_code": row.error_code,
        "question": row.question,
        "reply": row.reply,
        "clean_retry": row.clean_retry,
        "withheld": row.withheld,
        "guardrail": row.guardrail,
        "request_count": row.request_count,
        "latency_ms": row.latency_ms,
        "persona": row.persona,
        "profile_source": row.profile_source,
        "rounds": row.rounds.iter().map(|round| json!({
            "alias": round.model,
            "reasoning": round.reasoning,
            "finish_reason": round.finish_reason,
            "route": round.route,
            "clean": round.clean,
            "tool_bundles": round.tool_bundles,
            "tools": round.tools,
            "tool_calls": round.tool_calls,
            "response": round.response,
            "latency_ms": round.latency_ms,
            "prompt_tokens": round.prompt_tokens,
            "completion_tokens": round.completion_tokens,
        })).collect::<Vec<_>>(),
    })
}

/// Every tool call in order, arguments parsed when they came as text.
pub fn tool_calls(row: &ChatInteraction) -> Vec<Value> {
    let mut out = Vec::new();
    for (index, round) in row.rounds.iter().enumerate() {
        for call in round.tool_calls.as_array().into_iter().flatten() {
            let arguments = match &call["arguments"] {
                Value::String(text) => serde_json::from_str(text).unwrap_or(json!(text)),
                other => other.clone(),
            };
            out.push(json!({
                "round": index,
                "name": call["name"],
                "arguments": arguments,
                "outcome": call["outcome"],
                "result": call["result"],
                "posted": call["posted"],
            }));
        }
    }
    out
}

pub async fn extraction_logs(store: &SqliteStore) -> Vec<Value> {
    let Ok(page) = store
        .list_extractions(&ExtractionFilter {
            limit: 50,
            ..ExtractionFilter::default()
        })
        .await
    else {
        return Vec::new();
    };
    let mut rows: Vec<Value> = page
        .items
        .iter()
        .map(|log| {
            json!({
                "id": log.id,
                "at": label(log.at),
                "channel": log.channel_id,
                "alias": log.model,
                "reasoning": log.reasoning,
                "outcome": log.outcome.as_str(),
                "error": log.error,
                "request_count": log.request_count,
                "message_ids": log.message_ids,
                "proposal_ids": log.proposal_ids,
                "refusals": log.refusals.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
                "guardrail": log.guardrail,
                "latency_ms": log.latency_ms,
                "prompt_tokens": log.prompt_tokens,
                "completion_tokens": log.completion_tokens,
                "raw_response": log.raw_response,
                "parsed": serde_json::from_str::<Value>(&log.raw_response).unwrap_or(Value::Null),
                "prompt": log.prompt,
            })
        })
        .collect();
    rows.reverse();
    rows
}

fn render(embed: &Embed) -> String {
    let mut lines = Vec::new();
    if let Some(author) = &embed.author {
        lines.push(author.name.clone());
    }
    if let Some(title) = &embed.title {
        lines.push(title.clone());
    }
    if let Some(description) = &embed.description {
        lines.push(description.clone());
    }
    for field in &embed.fields {
        lines.push(field.name.clone());
        lines.push(field.value.clone());
    }
    if let Some(footer) = &embed.footer {
        lines.push(footer.text.clone());
    }
    lines.join("\n")
}

fn channel_name(channel: u64) -> String {
    match channel {
        C1 => "C1".to_owned(),
        world::C2 => "C2".to_owned(),
        other => other.to_string(),
    }
}

/// Every card create after the baseline, kept apart from the final visible
/// state: a card later deleted still counts as posted.
pub fn card_history(fake: &FakeDiscord, baseline: usize) -> Vec<Value> {
    let calls = fake.calls();
    let deleted: Vec<u64> = calls
        .iter()
        .skip(baseline)
        .filter_map(|call| match call {
            Call::Delete {
                message, outcome, ..
            } if outcome.is_delivered() => Some(message.get()),
            _ => None,
        })
        .collect();
    calls
        .into_iter()
        .skip(baseline)
        .filter_map(|call| match call {
            Call::Create {
                channel,
                message,
                outcome,
            } if !message.embeds.is_empty() => {
                let (posted, id, result) = match outcome {
                    Outcome::Delivered(id) => (true, Some(id.get()), "delivered".to_owned()),
                    // An ambiguous create may have posted: count it.
                    Outcome::Ambiguous(kind) => (true, None, format!("ambiguous {kind:?}")),
                    Outcome::DefinitelyRejected(kind) => {
                        (false, None, format!("rejected {kind:?}"))
                    }
                };
                let text = message
                    .embeds
                    .iter()
                    .map(render)
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(json!({
                    "channel": channel_name(channel.get()),
                    "posted": posted,
                    "outcome": result,
                    "deleted": id.is_some_and(|id| deleted.contains(&id)),
                    "text": text,
                }))
            }
            _ => None,
        })
        .collect()
}

/// What members see after the baseline: each delivered message's latest
/// content and embeds (placeholders are edited into answers, cards into
/// their refreshed form), in posting order.
pub fn messages(fake: &FakeDiscord, baseline: usize) -> Vec<Value> {
    struct Shown {
        id: u64,
        channel: u64,
        content: String,
        embeds: Vec<Embed>,
        reply_to: Option<u64>,
        edits: usize,
    }
    let mut shown: Vec<Shown> = Vec::new();
    for call in fake.calls().into_iter().skip(baseline) {
        match call {
            Call::Create {
                channel,
                message,
                outcome: Outcome::Delivered(id),
            } => shown.push(Shown {
                id: id.get(),
                channel: channel.get(),
                content: message.content.unwrap_or_default(),
                embeds: message.embeds,
                reply_to: message.reply_to.map(|id| id.get()),
                edits: 0,
            }),
            Call::Edit {
                message,
                edit,
                outcome: Outcome::Delivered(()),
                ..
            } => {
                if let Some(entry) = shown.iter_mut().find(|entry| entry.id == message.get()) {
                    if let Some(content) = edit.content {
                        entry.content = content;
                    }
                    if let Some(embeds) = edit.embeds {
                        entry.embeds = embeds;
                    }
                    entry.edits += 1;
                }
            }
            Call::Delete {
                message,
                outcome: Outcome::Delivered(()),
                ..
            } => shown.retain(|entry| entry.id != message.get()),
            _ => {}
        }
    }
    shown
        .into_iter()
        .map(|entry| {
            let card = !entry.embeds.is_empty();
            let mut text = entry.content.clone();
            for embed in &entry.embeds {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&render(embed));
            }
            json!({
                "channel": channel_name(entry.channel),
                "kind": if card { "card" } else { "text" },
                "reply_to_input": entry.reply_to.is_some(),
                "edits": entry.edits,
                "text": text,
                "embeds": serde_json::to_value(&entry.embeds).unwrap_or(Value::Null),
            })
        })
        .collect()
}

pub async fn proposals(store: &SqliteStore) -> Result<Vec<Value>, String> {
    let rows = store
        .list_proposals(false)
        .await
        .map_err(|e| format!("proposals: {e}"))?;
    let ids: Vec<String> = rows.iter().map(|row| row.draft.id.clone()).collect();
    let cards = store
        .load_cards(&ids)
        .await
        .map_err(|e| format!("cards: {e}"))?;
    Ok(rows
        .iter()
        .map(|row| {
            let card = cards.iter().find(|card| card.proposal_id == row.draft.id);
            let details = card.map(|card| &card.details);
            json!({
                "id": row.draft.id,
                "status": row.draft.status.as_str(),
                "source": row.info.source.as_str(),
                "source_id": row.info.source_id,
                "kind": details.map(|d| d.kind.as_str()),
                "run": details.and_then(|d| d.run_id.as_deref().map(short)),
                "when": details.and_then(|d| d.new_datetime.map(label)),
                "bosses": details.map(|d| d.bosses.clone()),
                "participants": details.map(|d| d.participants.iter().map(|p| member(p)).collect::<Vec<_>>()),
                "remove": details.map(|d| d.payload.remove.iter().map(|p| member(p)).collect::<Vec<_>>()),
                "add": details.map(|d| d.payload.add.iter().map(|p| member(p)).collect::<Vec<_>>()),
                "is_question": details.map(|d| d.is_question),
                "card_channel": card.map(|card| card.channel_id.clone()),
                "card_posted": card.is_some_and(|card| card.message_id.is_some()),
                "details": details.map(|d| d.to_json()),
            })
        })
        .collect())
}

pub type Answers = BTreeMap<(String, String), String>;

pub async fn rsvps(store: &SqliteStore) -> Result<Answers, String> {
    let snapshot = store
        .load(&Scope::All)
        .await
        .map_err(|e| format!("schedule: {e}"))?;
    Ok(snapshot
        .rsvps
        .iter()
        .map(|rsvp| {
            (
                (short(&rsvp.run_id), member(&rsvp.user_id)),
                rsvp.state.as_str().to_owned(),
            )
        })
        .collect())
}

pub fn rsvp_changes(before: &Answers, after: &Answers) -> Vec<Value> {
    let mut keys: Vec<&(String, String)> = before.keys().chain(after.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .map(|(run, who)| {
            json!({
                "run": run, "member": who,
                "before": before.get(&(run.clone(), who.clone())),
                "after": after.get(&(run.clone(), who.clone())),
            })
        })
        .collect()
}

pub async fn runs(store: &SqliteStore) -> Result<Vec<String>, String> {
    let snapshot = store
        .load(&Scope::All)
        .await
        .map_err(|e| format!("schedule: {e}"))?;
    let mut rows: Vec<String> = snapshot
        .runs
        .iter()
        // Status is left out: an answer may re-derive it (planned → confirmed).
        .map(|run| {
            format!(
                "{} {} {:?} {:?}",
                short(&run.id),
                label(run.datetime),
                run.bosses,
                run.participants
            )
        })
        .collect();
    rows.sort();
    Ok(rows)
}
