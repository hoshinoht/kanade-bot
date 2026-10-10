//! `reminders.json`: every card the bot will post or has posted.

use serde::Serialize;

use super::{
    Boss, iso_instant,
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
    /// The guild wall-clock label ("Tue 29 Sep 21:00").
    pub at: String,
    /// The exact fire instant (ISO, UTC), for "In" on the server clock.
    pub fire_at: String,
    pub bosses: Vec<Boss>,
    pub party: Vec<String>,
    pub url: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Reminders {
    pub upcoming: Vec<ReminderRow>,
    pub sent: Vec<ReminderRow>,
    /// The server's now when this was read: relative times count from it,
    /// never from the browser's clock.
    pub generated_at: String,
}

/// One embed field of a previewed card.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CardField {
    pub name: String,
    pub value: String,
    /// Shown side by side with the neighbouring inline fields.
    pub inline: bool,
}

/// A reminder card as the bot posts it: the message text (mentions as
/// `<@id>`), its first embed and any further ones (the redesigned day-of
/// card has one per run). Art is a same-origin `/art/` URL.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CardPreview {
    pub content: String,
    /// `#rrggbb`, the embed's colour bar.
    pub color: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<CardField>,
    pub footer: Option<String>,
    pub thumbnail: Option<String>,
    pub image: Option<String>,
    /// The embeds after the first, in order.
    pub more_embeds: Vec<EmbedPreview>,
    /// The heading line stored for this card (posted or prepared to post);
    /// `false` while it is the seed line the bot may reword when it posts.
    pub heading_final: bool,
}

/// A further embed of a previewed card.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EmbedPreview {
    /// `#rrggbb`.
    pub color: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<CardField>,
    pub footer: Option<String>,
    pub thumbnail: Option<String>,
    pub image: Option<String>,
}

/// `GET /api/admin/reminders/{id}/preview`: the row and its card; `card` is
/// null for a reminder that posts none (cancelled run, unknown kind), one
/// retired without posting, and a card posted before records were kept.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReminderPreview {
    pub reminder: ReminderRow,
    pub card: Option<CardPreview>,
    /// The run has started (for a posted card, every run it names): it is no
    /// longer edited, so Discord keeps its last edit from before then.
    pub run_started: bool,
    pub generated_at: String,
}

/// Upcoming (queued, or due and not yet posted) soonest first; sent and stale newest first.
pub fn reminders(ctx: &Context<'_>, snapshot: &ScheduleSnapshot) -> Reminders {
    let mut upcoming = Vec::new();
    let mut sent = Vec::new();
    for (reminder, run, kind, state) in classified(ctx, snapshot) {
        let row = row(ctx, reminder, run, kind, state);
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
        generated_at: iso_instant(ctx.now),
    }
}

pub fn row(
    ctx: &Context<'_>,
    reminder: &Reminder,
    run: &Run,
    kind: CardKind,
    state: ReminderState,
) -> ReminderRow {
    ReminderRow {
        id: reminder.id.clone(),
        run_id: run.id.clone(),
        run_short_id: short_id(&run.id),
        kind,
        state,
        at: when(reminder.fire_at, ctx.zone),
        fire_at: iso_instant(reminder.fire_at),
        bosses: ctx.bosses(&run.bosses),
        party: run.participants.iter().map(|id| ctx.name(id)).collect(),
        url: (state == ReminderState::Sent)
            .then(|| message_url(ctx, run, reminder))
            .flatten(),
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
pub fn classified<'s>(
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
