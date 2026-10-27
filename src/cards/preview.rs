//! A reminder's card for the admin preview, built by the delivery code with
//! no store write, model call, art read or Discord call:
//! - a posted card as the refresh worker edits it: its record's kind and
//!   heading over the runs it was posted for ([`posted`]);
//! - an unsent one as the tick would post it: the tick's own dispatch plan
//!   (`plan_dispatch`, at the reminder's fire time) with the stored record's
//!   heading when one exists, else the seed line ([`reminder_intent`]).
//!
//! Both end in `cards::build`, as `render` and the refresh edit do. The
//! estimate chart a new post attaches comes from the tick's own
//! [`super::estimates::chart_for`] ([`reminder_chart`]): roster and estimates-memory
//! reads and CPU only.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::cards::chart::Chart;
use crate::cards::estimates::{PartyEstimates, chart_for, shows_files};
use crate::cards::{self, Card, CardContext};
use kanade_domain::attendance::{countdown_mentions, morning_mentions};
use kanade_domain::members::Directory;
use kanade_domain::notify::{
    ChannelDirectory, DAY_OF_KIND, DedupeKey, DeliverySettings, DeliveryTarget, DispatchInput,
    IntentContent, NotificationIntent, PingKind, PostedCard, countdown_minutes, everyone_on,
    plan_dispatch, resolve_mentions,
};
use kanade_domain::schedule::ScheduleSnapshot;

/// Every channel reachable: the preview shows the card, not where it lands.
struct AnyChannel;

impl ChannelDirectory for AnyChannel {
    fn is_reachable(&self, _: &str) -> bool {
        true
    }
}

/// The post channel the preview plans with when none is configured, so a
/// run without a home channel still yields its card.
const PREVIEW_CHANNEL: &str = "preview";

/// The intent the tick plans for the unsent `reminder_id` when it falls due:
/// the reminder and the other unsent rows firing at the same instant (the
/// tick posts them together, so a day-of card groups them). `None` when the
/// row is unknown or already sent, or would be retired without a message
/// (cancelled run, unknown kind).
pub fn reminder_intent(
    schedule: &ScheduleSnapshot,
    reminder_id: &str,
    members: &dyn Directory,
    settings: DeliverySettings<'_>,
) -> Option<NotificationIntent> {
    let reminder = schedule
        .reminders
        .iter()
        .find(|row| row.id == reminder_id && row.sent_at.is_none())?;
    let at: DateTime<Utc> = reminder.fire_at;
    let mut due = schedule.clone();
    due.reminders = schedule
        .reminders
        .iter()
        .filter(|row| row.fire_at == at && row.sent_at.is_none())
        .cloned()
        .collect();
    let settings = DeliverySettings {
        post_channel_id: settings.post_channel_id.or(Some(PREVIEW_CHANNEL)),
        ..settings
    };
    let plan = plan_dispatch(&DispatchInput {
        now: at,
        schedule: &due,
        members,
        channels: &AnyChannel,
        journal: &BTreeSet::<DeliveryTarget>::new(),
        settings,
    });
    let target = DeliveryTarget::Reminder(reminder_id.to_owned());
    plan.sends
        .into_iter()
        .map(|send| send.intent)
        .find(|intent| intent.targets.contains(&target))
}

/// The card record key the tick stores the intent's heading under.
pub fn record_key(intent: &NotificationIntent) -> Option<String> {
    DedupeKey::native(&intent.targets)
        .ok()
        .map(|key| key.as_str().to_owned())
}

/// The content and mentions the refresh worker edits `card` with (test
/// cards are not reminder rows and are not previewed).
pub fn posted(card: &PostedCard, ctx: &CardContext<'_>) -> Option<(IntentContent, Vec<String>)> {
    let content = posted_content(card)?;
    let mentions = posted_mentions(ctx, &content);
    Some((content, mentions))
}

/// The card exactly as [`super::render::render`] and the refresh edit build it
/// before attaching art.
pub fn reminder_card(
    content: &IntentContent,
    mentions: &[String],
    ctx: &CardContext<'_>,
    heading: Option<&str>,
) -> Option<Card> {
    cards::build(content, ctx, heading, mentions)
}

/// The chart a new post of `card` (built from `content`) attaches, as the
/// tick decides it (`attach_chart`): `None` for a card with nowhere to
/// show a file and wherever [`chart_for`] draws none.
pub async fn reminder_chart(
    card: &Card,
    content: &IntentContent,
    ctx: &CardContext<'_>,
    port: &dyn PartyEstimates,
    now: DateTime<Utc>,
) -> Option<Chart> {
    if !shows_files(Some(&card.content), &card.components) {
        return None;
    }
    chart_for(content, ctx, port, now).await
}

/// What a posted card shows: its record's kind over the runs it was posted
/// for (its card→run rows), never the runs' current grouping.
pub fn posted_content(posted: &PostedCard) -> Option<IntentContent> {
    if posted.record.kind == DAY_OF_KIND {
        return Some(IntentContent::DayOf {
            run_ids: posted.run_ids.clone(),
        });
    }
    Some(IntentContent::Countdown {
        run_id: posted.run_ids.first()?.clone(),
        minutes: countdown_minutes(&posted.record.kind)?,
    })
}

/// Who the card names as a mention, planned as dispatch plans it, so an
/// edit reads like a fresh post (it notifies nobody either way).
pub fn posted_mentions(ctx: &CardContext<'_>, content: &IntentContent) -> Vec<String> {
    if ctx.quiet {
        return Vec::new();
    }
    let members: &dyn Directory = ctx.members;
    match content {
        IntentContent::DayOf { run_ids } => {
            let runs = cards::card_runs(ctx, run_ids);
            let unknown: Vec<Vec<String>> = runs
                .iter()
                .map(|run| morning_mentions(&ctx.states(run)))
                .collect();
            let candidates = everyone_on(unknown.iter().map(Vec::as_slice));
            resolve_mentions(members, &candidates, &PingKind::DayOf)
        }
        IntentContent::Countdown { run_id, .. } => match ctx.run(run_id) {
            Some(run) => resolve_mentions(
                members,
                &countdown_mentions(&ctx.states(run)),
                &PingKind::Countdown,
            ),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}
