//! Turning weekly timings into concrete runs for one boss week.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::draft::Draft;
use super::error::ScheduleError;
use super::lifecycle::{is_past, is_past_slot};
use super::policy::{SchedulePolicy, utc_instant};
use super::reminders::{ReminderPolicy, ensure_reminders, reconcile_day_of, refresh_run_reminders};
use super::rsvp::recompute_after_roster_change;
use super::run::{FixedRun, NewRun, Run, RunSource, RunStatus};
use crate::domain::ids::IdGenerator;
use crate::domain::time::{AwareDateTime, ZonedDateTime};
use crate::domain::weeks::{slot_in_week, week_end};

/// Create a run for every weekly timing in `week_start`'s boss week, then make
/// sure every run of that week has its reminders. Idempotent.
///
/// Pass the zoned week start ([`SchedulePolicy::materialised_weeks`]) so a
/// reset inside a DST gap keeps its wall clock, as v4 does; slots are placed
/// from that wall clock.
///
/// A slot already past is not created. A timing whose week already holds
/// exactly one matching standalone run adopts it in place instead; adopted
/// runs are not in the returned ids of newly created runs.
///
/// # Errors
/// [`ScheduleError::RunMoveConflict`] or [`ScheduleError::DateOutOfRange`].
pub fn materialise_week(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    week_start: impl AwareDateTime,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<Vec<String>, ScheduleError> {
    let start = week_start.astimezone(policy.zone)?;
    let week = utc_instant(&start)?;
    let end = week_end(&start, policy.zone)?;
    let mut created = Vec::new();
    for fixed in draft.fixed_runs() {
        if draft.run_for_fixed(&fixed.id, week).is_some() {
            continue;
        }
        let slot = slot_in_week(&start, policy.zone, fixed.weekday, fixed.time)?;
        // Unreachable by slot_in_week's contract; kept as v4's guard. Same-zone
        // values compare by wall clock, as Python does, so a gap reset (02:30
        // wall, 07:30Z) still owns a 03:00 slot (07:00Z).
        if slot.wall() < start.wall() || slot.wall() >= end.wall() {
            continue;
        }
        if is_past_slot(&slot, now)? {
            continue;
        }
        let run_at = slot.to_fixed().with_timezone(&Utc);
        if let Some(run) = adoptable_run(draft, &fixed, week, now) {
            adopt_run(draft, ids, &run, &fixed, &slot, policy, now)?;
            continue;
        }
        let id = draft.create_run(
            ids,
            NewRun {
                fixed_run_id: Some(fixed.id.clone()),
                channel_id: fixed.channel_id.clone(),
                week_start: week,
                datetime: run_at,
                bosses: fixed.bosses.clone(),
                participants: fixed.participants.clone(),
                status: RunStatus::Planned,
                source: RunSource::Fixed,
            },
        )?;
        created.push(id);
    }
    for run in draft.runs(Some(week)) {
        ensure_reminders(draft, ids, &run.id, policy, now, false)?;
    }
    Ok(created)
}

/// The one standalone run in that week `fixed` should take over, if exactly
/// one fits: same bosses (as a set), same home channel, not done/cancelled,
/// not past and not already tied to a timing.
pub fn adoptable_run(
    draft: &Draft,
    fixed: &FixedRun,
    week_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<Run> {
    let wanted: BTreeSet<&String> = fixed.bosses.iter().collect();
    let mut found: Vec<Run> = draft
        .runs(Some(week_start))
        .into_iter()
        .filter(|run| {
            run.fixed_run_id.is_none()
                && !run.status.is_terminal()
                && run.bosses.iter().collect::<BTreeSet<_>>() == wanted
                && run.channel_id == fixed.channel_id
                && !is_past(run.datetime, now)
        })
        .collect();
    if found.len() == 1 { found.pop() } else { None }
}

/// Make a standalone run that week's run for `fixed`, keeping its id, answers
/// and posted pings; only slot, party and timing link are pushed onto it. The
/// run is already in the slot's boss week, so its `week_start` is kept.
fn adopt_run(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run: &Run,
    fixed: &FixedRun,
    slot: &ZonedDateTime,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<(), ScheduleError> {
    draft.set_run_fixed(&run.id, Some(fixed.id.clone()))?;
    draft.set_run_datetime(&run.id, slot.to_fixed().with_timezone(&Utc), run.week_start)?;
    if run.participants != fixed.participants {
        draft.set_run_participants(&run.id, fixed.participants.clone(), now);
        recompute_after_roster_change(draft, &run.id, now)?;
    }
    refresh_run_reminders(draft, ids, &run.id, policy, now)
}

/// v4 `BossBot.materialise_weeks`: materialise the current and next two boss
/// weeks from one clock read, then reconcile day-of pings to the ping time.
/// Returns every newly created run id.
///
/// # Errors
/// [`ScheduleError::RunMoveConflict`] or [`ScheduleError::DateOutOfRange`].
pub fn materialise_weeks(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Vec<String>, ScheduleError> {
    let mut created = Vec::new();
    for week in policy.materialised_weeks(now)? {
        created.extend(materialise_week(draft, ids, week, &policy.reminders, now)?);
    }
    reconcile_day_of(draft, &policy.reminders, now)?;
    Ok(created)
}
