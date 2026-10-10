//! Due-reminder dispatch as a pure plan (v4 `BossBot.dispatch_reminders`).
//!
//! Due rows are retired without a message when their run is gone or cancelled,
//! when they are too late to help, or when their kind is not a known ping.
//! Countdowns post one card each; day-of rows post one card per home channel.
//!
//! v5 deviation: a home channel that is unset or unreachable falls back to the
//! post channel and the delivery binds there, with a
//! [`DeliveryWarning::HomeChannelUnavailable`]; v4 refused the binding and the
//! ping expired silently.

use chrono::{DateTime, Utc};

use super::audience::{everyone_on, resolve_mentions};
use super::intent::{
    ChannelChoice, ChannelDirectory, DeliveryTarget, DeliveryWarning, EffectKind, IntentContent,
    JournalView, NotificationIntent, PlannedSend, canonical_allow_list, choose_channel,
};
use super::policy::{PingKind, allowed_mentions};
use crate::domain::attendance::{
    AnswerState, AttendanceMode, AttendancePolicy, countdown_mentions, morning_mentions,
    snapshot_states,
};
use crate::domain::members::Directory;
use crate::domain::pytext;
use crate::domain::schedule::{
    COUNTDOWN_PREFIX, DAY_OF, Reminder, Run, RunStatus, ScheduleSnapshot, is_stale,
};

/// Why a due row is retired without a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SuppressReason {
    /// The run is gone or cancelled.
    Cancelled,
    /// Past its kind's grace ([`is_stale`]).
    Stale,
    /// Neither `day_of` nor a readable `countdown_M`.
    UnknownKind,
}

/// What one due reminder becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReminderClass {
    Suppress(SuppressReason),
    DayOf,
    Countdown { minutes: i64 },
}

/// The minutes of a `countdown_M` kind, read as Python `int()` reads `M`.
pub fn countdown_minutes(kind: &str) -> Option<i64> {
    let text = pytext::strip(kind.strip_prefix(COUNTDOWN_PREFIX)?);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    // Underscores may only separate digits.
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') {
        return None;
    }
    let mut value: i64 = 0;
    let mut previous_underscore = false;
    for c in digits.chars() {
        if c == '_' {
            if previous_underscore {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        previous_underscore = false;
        let digit = i64::from(pytext::decimal(c)?);
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(if negative { -value } else { value })
}

/// Classify one due row against its run (v4 check order: run, staleness, kind).
pub fn classify(reminder: &Reminder, run: Option<&Run>, now: DateTime<Utc>) -> ReminderClass {
    if run.is_none_or(|run| run.status == RunStatus::Cancelled) {
        return ReminderClass::Suppress(SuppressReason::Cancelled);
    }
    if is_stale(&reminder.kind, reminder.fire_at, now) {
        return ReminderClass::Suppress(SuppressReason::Stale);
    }
    if reminder.kind == DAY_OF {
        return ReminderClass::DayOf;
    }
    match countdown_minutes(&reminder.kind) {
        Some(minutes) => ReminderClass::Countdown { minutes },
        None => ReminderClass::Suppress(SuppressReason::UnknownKind),
    }
}

/// Unsent rows whose time has come, by `(fire_at, id)`.
pub fn due_reminders(reminders: &[Reminder], now: DateTime<Utc>) -> Vec<&Reminder> {
    let mut due: Vec<&Reminder> = reminders
        .iter()
        .filter(|row| row.sent_at.is_none() && row.fire_at <= now)
        .collect();
    due.sort_by(|a, b| (a.fire_at, &a.id).cmp(&(b.fire_at, &b.id)));
    due
}

/// Guild settings dispatch reads.
#[derive(Clone, Copy, Debug)]
pub struct DeliverySettings<'a> {
    pub post_channel_id: Option<&'a str>,
    pub quiet_mode: bool,
    /// Who the morning ping names (v5: unknown answers only) and how cards
    /// show answers; `SchedulePolicy.attendance`.
    pub attendance: AttendancePolicy,
}

/// Everything one dispatch pass reads.
#[derive(Clone, Copy)]
pub struct DispatchInput<'a> {
    pub now: DateTime<Utc>,
    /// Runs with all their reminders and RSVPs.
    pub schedule: &'a ScheduleSnapshot,
    pub members: &'a dyn Directory,
    pub channels: &'a dyn ChannelDirectory,
    pub journal: &'a dyn JournalView,
    pub settings: DeliverySettings<'a>,
}

/// A due row retired without a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retirement {
    pub reminder_id: String,
    pub reason: SuppressReason,
}

/// Due rows left queued because there is nowhere to post them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub reminder_ids: Vec<String>,
    pub home_channel_id: Option<String>,
}

/// One dispatch pass: retirements, sends in v4 order (countdowns as met, then
/// day-of cards per home channel in first-seen order), and rows left queued.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchPlan {
    pub retire: Vec<Retirement>,
    pub sends: Vec<PlannedSend>,
    pub queued: Vec<Queued>,
}

type DayOfEntry<'a> = (&'a Reminder, &'a Run);
/// Day-of rows sharing one home channel key (`None` = no home channel).
type DayOfGroup<'a> = (Option<&'a str>, Vec<DayOfEntry<'a>>);

struct Destination {
    channel_id: String,
    warnings: Vec<DeliveryWarning>,
}

fn destination(
    input: &DispatchInput<'_>,
    home: Option<&str>,
    runs: &[&Run],
) -> Option<Destination> {
    match choose_channel(home, input.settings.post_channel_id, input.channels) {
        ChannelChoice::Requested(channel_id) => Some(Destination {
            channel_id,
            warnings: Vec::new(),
        }),
        ChannelChoice::Fallback {
            channel_id,
            requested,
        } => Some(Destination {
            channel_id,
            warnings: vec![DeliveryWarning::HomeChannelUnavailable {
                home_channel_id: requested,
                run_ids: runs.iter().map(|run| run.id.clone()).collect(),
            }],
        }),
        ChannelChoice::Unavailable => None,
    }
}

fn states(input: &DispatchInput<'_>, run: &Run) -> Vec<(String, AnswerState)> {
    snapshot_states(input.schedule, run, input.settings.attendance.mode)
}

fn gate(input: &DispatchInput<'_>, card_mentions: &[String]) -> Vec<String> {
    canonical_allow_list(allowed_mentions(card_mentions, None, input.settings.quiet_mode).users())
}

fn countdown(
    input: &DispatchInput<'_>,
    plan: &mut DispatchPlan,
    reminder: &Reminder,
    run: &Run,
    minutes: i64,
) {
    let Some(to) = destination(input, run.channel_id.as_deref(), &[run]) else {
        plan.queued.push(Queued {
            reminder_ids: vec![reminder.id.clone()],
            home_channel_id: run.channel_id.clone(),
        });
        return;
    };
    // v4: everyone not declined (in v4-compat mode exactly `not_declined`).
    let candidates = countdown_mentions(&states(input, run));
    let mentions = resolve_mentions(input.members, &candidates, &PingKind::Countdown);
    let intent = NotificationIntent {
        effect: EffectKind::Reminder,
        effect_context: Vec::new(),
        channel_id: to.channel_id,
        targets: vec![DeliveryTarget::Reminder(reminder.id.clone())],
        mentions: gate(input, &mentions),
        content: IntentContent::Countdown {
            run_id: run.id.clone(),
            minutes,
        },
        warnings: to.warnings,
    };
    plan.sends.push(PlannedSend::new(intent, input.journal));
}

fn day_of(
    input: &DispatchInput<'_>,
    plan: &mut DispatchPlan,
    home: Option<&str>,
    mut entries: Vec<DayOfEntry<'_>>,
) {
    // Stable, so equal start times keep due order.
    entries.sort_by_key(|(_, run)| run.datetime);
    let runs: Vec<&Run> = entries.iter().map(|(_, run)| *run).collect();
    let Some(to) = destination(input, home, &runs) else {
        plan.queued.push(Queued {
            reminder_ids: entries.iter().map(|(row, _)| row.id.clone()).collect(),
            home_channel_id: home.map(str::to_owned),
        });
        return;
    };
    let people = everyone_on(runs.iter().map(|run| run.participants.as_slice()));
    let mentions = match input.settings.attendance.mode {
        AttendanceMode::V4Compat => resolve_mentions(input.members, &people, &PingKind::DayOf),
        // v5: the morning ping names only members whose answer is unknown.
        AttendanceMode::V5 => {
            let unknown: Vec<Vec<String>> = runs
                .iter()
                .map(|run| morning_mentions(&states(input, run)))
                .collect();
            let candidates = everyone_on(unknown.iter().map(Vec::as_slice));
            resolve_mentions(input.members, &candidates, &PingKind::DayOf)
        }
    };
    let intent = NotificationIntent {
        effect: EffectKind::Reminder,
        effect_context: Vec::new(),
        channel_id: to.channel_id,
        targets: entries
            .iter()
            .map(|(row, _)| DeliveryTarget::Reminder(row.id.clone()))
            .collect(),
        mentions: gate(input, &mentions),
        content: IntentContent::DayOf {
            run_ids: runs.iter().map(|run| run.id.clone()).collect(),
        },
        warnings: to.warnings,
    };
    plan.sends.push(PlannedSend::new(intent, input.journal));
}

/// Plan one dispatch pass over every due reminder in `input.schedule`.
pub fn plan_dispatch(input: &DispatchInput<'_>) -> DispatchPlan {
    let schedule = input.schedule;
    let run = |id: &str| schedule.runs.iter().find(|run| run.id == id);
    let mut plan = DispatchPlan::default();
    let mut groups: Vec<DayOfGroup<'_>> = Vec::new();
    for reminder in due_reminders(&schedule.reminders, input.now) {
        let owner = run(&reminder.run_id);
        match (classify(reminder, owner, input.now), owner) {
            (ReminderClass::Suppress(reason), _) => plan.retire.push(Retirement {
                reminder_id: reminder.id.clone(),
                reason,
            }),
            (ReminderClass::DayOf, Some(owner)) => {
                let home = owner.channel_id.as_deref();
                match groups.iter_mut().find(|(key, _)| *key == home) {
                    Some((_, entries)) => entries.push((reminder, owner)),
                    None => groups.push((home, vec![(reminder, owner)])),
                }
            }
            (ReminderClass::Countdown { minutes }, Some(owner)) => {
                countdown(input, &mut plan, reminder, owner, minutes);
            }
            // Unreachable: `classify` already suppresses rows without a run.
            (_, None) => plan.retire.push(Retirement {
                reminder_id: reminder.id.clone(),
                reason: SuppressReason::Cancelled,
            }),
        }
    }
    for (home, entries) in groups {
        day_of(input, &mut plan, home, entries);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::countdown_minutes;

    #[test]
    fn countdown_minutes_reads_the_suffix_as_python_int() {
        assert_eq!(countdown_minutes("countdown_15"), Some(15));
        assert_eq!(countdown_minutes("countdown_ +1_5 "), Some(15));
        assert_eq!(countdown_minutes("countdown_-5"), Some(-5));
        assert_eq!(countdown_minutes("countdown_\u{ff19}"), Some(9));
        for kind in [
            "day_of",
            "day_off",
            "countdown_",
            "countdown_1__5",
            "countdown__5",
            "countdown_5_",
            "countdown_x",
            "countdown_99999999999999999999",
        ] {
            assert_eq!(countdown_minutes(kind), None, "{kind}");
        }
    }
}
