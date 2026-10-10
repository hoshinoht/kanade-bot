//! Reminder rows: which pings a run gets, writing them, and stale suppression.
//!
//! Reminder rows are the scheduler's source of truth. A row whose `fire_at` is
//! already behind the clock when written is stored as sent, so a restart or a
//! late edit never produces a due ping for a slot that has passed.

use chrono::{DateTime, FixedOffset, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;

use super::draft::Draft;
use super::error::ScheduleError;
use super::run::RunStatus;
use crate::domain::ids::IdGenerator;
use crate::domain::time::{AwareDateTime, ZonedDateTime};

pub const DAY_OF: &str = "day_of";
pub const COUNTDOWN_PREFIX: &str = "countdown_";

/// How late a day-of ping may still post.
pub const DAY_OF_GRACE: TimeDelta = TimeDelta::hours(12);
/// How late a countdown (or any other kind) may still post.
pub const COUNTDOWN_GRACE: TimeDelta = TimeDelta::minutes(30);

/// v5 deviation from v4: how far before the start a day-of ping is clamped
/// when its resolved instant would be at or after the run start. One second is
/// the finest step v4 inputs carry (`ping_time` is `HH:MM:SS`).
pub const DAY_OF_CLAMP: TimeDelta = TimeDelta::seconds(1);

/// The guild settings reminders are derived from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderPolicy {
    pub zone: Tz,
    pub ping_time: NaiveTime,
    /// Countdown offsets in minutes; duplicates and order do not matter.
    pub countdowns: Vec<u32>,
}

/// One reminder a run should have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderSpec {
    pub kind: String,
    /// `day_of` in the guild zone; countdowns in the run instant's offset.
    pub fire_at: DateTime<FixedOffset>,
}

pub fn countdown_kind(minutes: u32) -> String {
    format!("{COUNTDOWN_PREFIX}{minutes}")
}

/// True when `fire_at` is so far behind `now` that posting would mislead
/// (strictly more than the kind's grace; unknown kinds use the countdown grace).
pub fn is_stale(kind: &str, fire_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let grace = if kind == DAY_OF {
        DAY_OF_GRACE
    } else {
        COUNTDOWN_GRACE
    };
    now - fire_at > grace
}

/// The reminders a run should have, as pure data.
///
/// * `day_of` - `ping_time` on the run's local date, or the previous day when
///   that wall clock is not before the run's (late-night runs).
/// * `countdown_M` - `M` minutes before the run, largest first.
///
/// `otot` keeps only `day_of`; `cancelled` and `done` get nothing.
///
/// v5 deviation: a ping wall clock inside a spring-forward gap resolves with
/// the pre-transition offset (as v4), which can land at or after the start.
/// Such a `day_of` is clamped to `start - DAY_OF_CLAMP`; nothing else moves.
///
/// # Errors
/// [`ScheduleError::DateOutOfRange`] outside v4's representable years.
pub fn reminder_specs(
    run_at: DateTime<FixedOffset>,
    status: RunStatus,
    policy: &ReminderPolicy,
) -> Result<Vec<ReminderSpec>, ScheduleError> {
    if matches!(status, RunStatus::Cancelled | RunStatus::Done) {
        return Ok(Vec::new());
    }
    let local = run_at.astimezone(policy.zone)?;
    let mut day_of =
        ZonedDateTime::new(local.wall().date().and_time(policy.ping_time), policy.zone)?;
    // Wall-clock comparison, as Python compares datetimes sharing a zone.
    if day_of.wall() >= local.wall() {
        let previous = day_of
            .wall()
            .checked_sub_signed(TimeDelta::days(1))
            .ok_or(crate::domain::time::DateOutOfRange)?;
        day_of = ZonedDateTime::new(previous, policy.zone)?;
    }
    let mut day_of_at = day_of.to_fixed();
    if day_of_at >= run_at {
        day_of_at = ZonedDateTime::from_instant(&(run_at - DAY_OF_CLAMP), policy.zone)?.to_fixed();
    }
    let mut specs = vec![ReminderSpec {
        kind: DAY_OF.to_owned(),
        fire_at: day_of_at,
    }];
    if status != RunStatus::Otot {
        let mut minutes = policy.countdowns.clone();
        minutes.sort_unstable_by(|a, b| b.cmp(a));
        minutes.dedup();
        specs.extend(minutes.into_iter().map(|m| ReminderSpec {
            kind: countdown_kind(m),
            fire_at: run_at - TimeDelta::minutes(i64::from(m)),
        }));
    }
    Ok(specs)
}

/// Create the reminder rows a run should have; idempotent. Returns created kinds.
///
/// Without `rebuild`, unsent rows of kinds the run should no longer have are
/// dropped. `rebuild` drops every row, sent ones included, so a moved run gets
/// fresh pings despite `(run_id, kind)` uniqueness. Slots at or before `now`
/// are written as sent.
///
/// # Errors
/// [`ScheduleError::UnknownRun`] or [`ScheduleError::DateOutOfRange`].
pub fn ensure_reminders(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
    rebuild: bool,
) -> Result<Vec<String>, ScheduleError> {
    let run = draft.require_run(run_id)?;
    let specs = reminder_specs(run.datetime.fixed_offset(), run.status, policy)?;
    if rebuild {
        draft.delete_reminders(run_id);
    } else {
        let wanted: Vec<&str> = specs.iter().map(|spec| spec.kind.as_str()).collect();
        draft.delete_unsent_reminders(run_id, &wanted);
    }
    let mut created = Vec::new();
    for spec in specs {
        let fire_at = spec.fire_at.with_timezone(&Utc);
        let sent_at = (fire_at <= now).then_some(now);
        if draft
            .add_reminder(ids, run_id, &spec.kind, fire_at, sent_at)
            .is_some()
        {
            created.push(spec.kind);
        }
    }
    Ok(created)
}

/// Rebuild one run's reminders after it changed; an absent run is ignored.
///
/// # Errors
/// [`ScheduleError::DateOutOfRange`].
pub fn refresh_run_reminders(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<(), ScheduleError> {
    if draft.run(run_id).is_some() {
        ensure_reminders(draft, ids, run_id, policy, now, true)?;
    }
    Ok(())
}

/// Move unposted `day_of` rows to the policy's ping time; returns rows changed.
///
/// Posted cards stay as they reached Discord; a queued row moved into the past
/// is retired rather than becoming due; a skipped row reopens if its new time
/// is ahead.
///
/// # Errors
/// [`ScheduleError::DateOutOfRange`].
pub fn reconcile_day_of(
    draft: &mut Draft,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<usize, ScheduleError> {
    let day_policy = ReminderPolicy {
        countdowns: Vec::new(),
        ..policy.clone()
    };
    let mut changed = 0;
    for run in draft.runs(None) {
        let Some(reminder) = draft
            .reminders(&run.id)
            .into_iter()
            .find(|row| row.kind == DAY_OF)
        else {
            continue;
        };
        let spec = reminder_specs(run.datetime.fixed_offset(), run.status, &day_policy)?
            .into_iter()
            .find(|spec| spec.kind == DAY_OF);
        if let Some(spec) = spec
            && draft.reschedule_unposted_reminder(
                &reminder.id,
                spec.fire_at.with_timezone(&Utc),
                now,
            )
        {
            changed += 1;
        }
    }
    Ok(changed)
}
