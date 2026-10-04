//! `reminders.json`: every card the bot will post or has posted.

use serde::Serialize;

use super::{
    Boss,
    week::{CardKind, CardState, Context, card_label, card_state, message_url},
    when,
};
use crate::domain::{
    ids::short_id,
    schedule::{Reminder, Run, ScheduleSnapshot},
};

/// A reminder row's state: `due` is past its fire time but not yet posted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ReminderState {
    Queued,
    Due,
    Sent,
    Stale,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReminderRow {
    pub id: String,
    pub run_id: String,
    pub run_short_id: String,
    pub kind: CardKind,
    pub state: ReminderState,
    pub at: String,
    pub bosses: Vec<Boss>,
    pub party: Vec<String>,
    pub url: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Reminders {
    pub upcoming: Vec<ReminderRow>,
    pub sent: Vec<ReminderRow>,
}

/// Upcoming (queued, or due and not yet posted) soonest first; sent and stale newest first.
pub fn reminders(ctx: &Context<'_>, snapshot: &ScheduleSnapshot) -> Reminders {
    let mut upcoming = Vec::new();
    let mut sent = Vec::new();
    for (reminder, run, kind, state) in classified(ctx, snapshot) {
        let row = ReminderRow {
            id: reminder.id.clone(),
            run_id: run.id.clone(),
            run_short_id: short_id(&run.id),
            kind,
            state,
            at: when(reminder.fire_at, ctx.zone),
            bosses: ctx.bosses(&run.bosses),
            party: run.participants.iter().map(|id| ctx.name(id)).collect(),
            url: (state == ReminderState::Sent)
                .then(|| message_url(ctx, run, reminder))
                .flatten(),
        };
        let key = (reminder.fire_at, reminder.id.clone());
        if is_upcoming(&state) {
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

/// How many rows [`reminders`] lists as `upcoming`: the Reminders page's queued count.
pub fn upcoming(ctx: &Context<'_>, snapshot: &ScheduleSnapshot) -> usize {
    classified(ctx, snapshot)
        .filter(|(.., state)| is_upcoming(state))
        .count()
}

pub(super) fn is_upcoming(state: &ReminderState) -> bool {
    matches!(state, ReminderState::Queued | ReminderState::Due)
}

/// Each listable reminder with its run, card label and row state.
pub(super) fn classified<'s>(
    ctx: &Context<'_>,
    snapshot: &'s ScheduleSnapshot,
) -> impl Iterator<Item = (&'s Reminder, &'s Run, CardKind, ReminderState)> {
    let now = ctx.now;
    snapshot.reminders.iter().filter_map(move |reminder| {
        let kind = card_label(&reminder.kind)?;
        let run = snapshot.runs.iter().find(|run| run.id == reminder.run_id)?;
        let state = match card_state(reminder, snapshot, now) {
            CardState::Posted => ReminderState::Sent,
            CardState::Skipped => ReminderState::Stale,
            CardState::Queued if reminder.fire_at <= now => ReminderState::Due,
            CardState::Queued => ReminderState::Queued,
        };
        Some((reminder, run, kind, state))
    })
}
