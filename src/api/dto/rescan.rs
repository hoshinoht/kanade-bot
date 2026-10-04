//! `RescanJob` from a job and its per-channel results
//! (`docs/v5/extraction-orchestration.md` "Rescan jobs"). Result errors can
//! carry store or Discord text, so each channel's `errors` are fixed
//! sentences naming what went wrong, never the recorded text.
//!
//! Progress is counted in gated messages: a channel's `messages` once read,
//! the job's `messages` their sum, and `messages_total` that sum plus what
//! each unread channel was expected to find when the job started. A final
//! job's total is what it read, so a done job's two counts are equal.

use serde_json::{Value, json};

use super::iso_instant;
use crate::{api::rescan::RescanView, domain::model_log::RescanStatus};

pub const CHANNEL_FAILED: &str = "This channel could not be read.";
pub const BACKFILL_FAILED: &str =
    "Discord history could not be fetched; only cached messages were read.";
pub const OTHER_FAILED: &str = "Some messages or changes could not be saved or proposed.";

fn unread_error(unread: u64) -> String {
    format!("{unread} message(s) not read: the model kept turning the rescan away.")
}

/// The API spelling of a stored window (v4's `2weeks` is `two_weeks`).
pub fn window(stored: &str) -> &str {
    match stored {
        "2weeks" => "two_weeks",
        other => other,
    }
}

fn count(result: &Value, key: &str) -> u64 {
    result.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn errors(result: &Value, unread: u64) -> Vec<String> {
    let recorded: Vec<&str> = result
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let backfill = recorded
        .iter()
        .filter(|error| error.starts_with("backfill:"))
        .count();
    // The reader records one error for the unread messages.
    let other = recorded
        .len()
        .saturating_sub(backfill + usize::from(unread > 0));
    let mut out = Vec::new();
    if unread > 0 {
        out.push(unread_error(unread));
    }
    if backfill > 0 {
        out.push(BACKFILL_FAILED.to_owned());
    }
    if other > 0 {
        out.push(OTHER_FAILED.to_owned());
    }
    out
}

/// `name` names a channel the results do not (not reached yet, or failed).
pub fn job(view: &RescanView, name: impl Fn(&str) -> String) -> Value {
    let job = &view.job;
    let results: Vec<&Value> = job.results.as_array().into_iter().flatten().collect();
    let result_of = |channel: &str| {
        results
            .iter()
            .copied()
            .find(|result| result.get("channel_id").and_then(Value::as_str) == Some(channel))
    };
    let reached = |index: usize| {
        job.channels.iter().skip(index + 1).any(|later| {
            result_of(later).is_some() || view.current.as_deref() == Some(later.as_str())
        })
    };
    let ended = matches!(job.status, RescanStatus::Done | RescanStatus::Failed);
    let mut proposals = 0;
    let mut unread_total = 0;
    let mut read = 0;
    // `None` once a channel still to be read has no count.
    let mut total = Some(0);
    let channels: Vec<Value> = job
        .channels
        .iter()
        .enumerate()
        .map(|(index, channel)| {
            let (state, messages, unread, errors, named) = match result_of(channel) {
                Some(result) => {
                    let unread = count(result, "unread");
                    proposals += count(result, "proposals");
                    unread_total += unread;
                    let named = result
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned);
                    (
                        "done",
                        count(result, "gated"),
                        unread,
                        errors(result, unread),
                        named,
                    )
                }
                None if view.current.as_deref() == Some(channel.as_str()) => {
                    ("reading", 0, 0, Vec::new(), None)
                }
                None if ended || reached(index) => {
                    ("done", 0, 0, vec![CHANNEL_FAILED.to_owned()], None)
                }
                None => ("queued", 0, 0, Vec::new(), None),
            };
            read += messages;
            let still = if state == "done" {
                Some(messages)
            } else {
                view.expected.get(channel).map(|&count| count as u64)
            };
            total = total.zip(still).map(|(sum, more)| sum + more);
            json!({
                "id": channel,
                "name": named.unwrap_or_else(|| name(channel)),
                "state": state,
                "messages": messages,
                "unread": unread,
                "errors": errors,
            })
        })
        .collect();
    let state = match job.status {
        _ if view.stopping => "cancelled",
        RescanStatus::Queued | RescanStatus::Running => "running",
        RescanStatus::Done | RescanStatus::Failed => "done",
        RescanStatus::Cancelled => "cancelled",
    };
    let total = if job.status.is_final() {
        Some(read)
    } else {
        total
    };
    json!({
        "id": job.id,
        "state": state,
        "window": window(&job.window),
        "started_at": job.started_at.map(iso_instant),
        "channels": channels,
        "messages": read,
        "messages_total": total,
        "proposals": proposals,
        "unread": unread_total,
    })
}
