//! `reminders.json`: every card the bot will post or has posted.

use serde::Serialize;

use super::{
    Boss,
    week::{Context, card_label, card_state, message_url},
    when,
};
use crate::domain::{ids::short_id, schedule::ScheduleSnapshot};

#[derive(Serialize)]
pub struct ReminderRow {
    pub id: String,
    pub run_id: String,
    pub run_short_id: String,
    pub kind: &'static str,
    pub state: &'static str,
    pub at: String,
    pub bosses: Vec<Boss>,
    pub party: Vec<String>,
    pub url: Option<String>,
}

#[derive(Serialize)]
pub struct Reminders {
    pub upcoming: Vec<ReminderRow>,
    pub sent: Vec<ReminderRow>,
}

/// Upcoming (queued, or due and not yet posted) soonest first; sent and stale newest first.
pub fn reminders(ctx: &Context<'_>, snapshot: &ScheduleSnapshot) -> Reminders {
    let mut upcoming = Vec::new();
    let mut sent = Vec::new();
    for reminder in &snapshot.reminders {
        let Some(kind) = card_label(&reminder.kind) else {
            continue;
        };
        let Some(run) = snapshot.runs.iter().find(|run| run.id == reminder.run_id) else {
            continue;
        };
        let state = match card_state(reminder, snapshot, ctx.now) {
            "posted" => "sent",
            "skipped" => "stale",
            _ if reminder.fire_at <= ctx.now => "due",
            _ => "queued",
        };
        let row = ReminderRow {
            id: reminder.id.clone(),
            run_id: run.id.clone(),
            run_short_id: short_id(&run.id),
            kind,
            state,
            at: when(reminder.fire_at, ctx.zone),
            bosses: ctx.bosses(&run.bosses),
            party: run.participants.iter().map(|id| ctx.name(id)).collect(),
            url: (state == "sent")
                .then(|| message_url(ctx, run, reminder))
                .flatten(),
        };
        let key = (reminder.fire_at, reminder.id.clone());
        if matches!(state, "queued" | "due") {
            upcoming.push((key, row));
        } else {
            sent.push((key, row));
        }
    }
    upcoming.sort_by(|a, b| a.0.cmp(&b.0));
    sent.sort_by(|a, b| b.0.cmp(&a.0));
    Reminders {
        upcoming: upcoming.into_iter().map(|(_, row)| row).collect(),
        sent: sent.into_iter().map(|(_, row)| row).collect(),
    }
}
