//! Before a reminder card is claimed: its record (kind and, for day-of, the
//! heading line), reused if a retry or restart already wrote one, so the
//! rewrite runs once per card and edits render what was posted.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::cards::local_day;
use super::cards::{CardContext, CardKit, CardRecord, DAY_OF_KIND, ReminderCardStore, card_runs};
use crate::domain::notify::{DedupeKey, IntentContent, NotificationIntent};
use crate::runtime::logging;

/// The record kind for `content`; `None` for sends that are not reminders.
fn record_kind(content: &IntentContent) -> Option<String> {
    match content {
        IntentContent::DayOf { .. } => Some(DAY_OF_KIND.to_owned()),
        IntentContent::Countdown { minutes, .. } => Some(format!("countdown_{minutes}")),
        _ => None,
    }
}

/// The stored record for `intent`, else a new one saved now (the day-of
/// heading chosen within the rewrite deadline). A store failure keeps the
/// fresh record for this send only.
pub async fn prepare<S: ReminderCardStore>(
    store: &S,
    kit: &CardKit,
    ctx: &CardContext<'_>,
    intent: &NotificationIntent,
    now: DateTime<Utc>,
) -> Option<CardRecord> {
    let kind = record_kind(&intent.content)?;
    let key = DedupeKey::native(&intent.targets).ok()?;
    if let Ok(Some(record)) = store.card_record(key.as_str()).await {
        return Some(record);
    }
    let heading = match &intent.content {
        IntentContent::DayOf { run_ids } => {
            let first = card_runs(ctx, run_ids).first()?.datetime;
            Some(kit.heading.choose(&local_day(first, ctx.zone)).await.0)
        }
        _ => None,
    };
    let record = CardRecord { kind, heading };
    match store.save_card_record(key.as_str(), &record, now).await {
        Ok(stored) => Some(stored),
        Err(_) => {
            // Store text can carry paths; the event is enough.
            logging::event("WARN", "card_record_failed", json!({}));
            Some(record)
        }
    }
}
