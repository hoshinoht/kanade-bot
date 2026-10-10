//! A reminder's card for the admin preview, built by the delivery code with
//! no store write, model call, art read or Discord call:
//! - a posted card as the refresh worker edits it: its record's kind and
//!   heading over the runs it was posted for ([`posted`]);
//! - an unsent one as the tick would post it: the tick's own dispatch plan
//!   (`plan_dispatch`, at the reminder's fire time) with the stored record's
//!   heading when one exists, else the seed line ([`reminder_intent`]).
//!
//! Both end in `cards::build`, as `render` and the refresh edit do.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::cards::{self, Card, CardContext, PostedCard};
use super::refresh::{posted_content, posted_mentions};
use crate::domain::members::Directory;
use crate::domain::notify::{
    ChannelDirectory, DedupeKey, DeliverySettings, DeliveryTarget, DispatchInput, IntentContent,
    NotificationIntent, plan_dispatch,
};
use crate::domain::schedule::ScheduleSnapshot;

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

/// The card exactly as [`super::render`] and the refresh edit build it
/// before attaching art.
pub fn reminder_card(
    content: &IntentContent,
    mentions: &[String],
    ctx: &CardContext<'_>,
    heading: Option<&str>,
) -> Option<Card> {
    cards::build(content, ctx, heading, mentions)
}
