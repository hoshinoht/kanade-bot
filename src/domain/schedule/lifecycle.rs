//! Run lifecycle: retiring past runs and pushing weekly-timing edits onto runs.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeDelta, Utc};

use super::draft::Draft;
use super::error::ScheduleError;
use super::policy::utc_instant;
use super::reminders::{ReminderPolicy, refresh_run_reminders};
use super::rsvp::recompute_after_roster_change;
use super::run::{FixedField, RunStatus};
use crate::domain::attendance::AttendanceMode;
use crate::domain::ids::IdGenerator;
use crate::domain::time::{AwareDateTime, ZonedDateTime};
use crate::domain::weeks::slot_in_week;

/// How long after its start a run still counts as happening.
pub const RUN_DONE_AFTER: TimeDelta = TimeDelta::hours(2);

/// True once a run's slot is strictly more than [`RUN_DONE_AFTER`] behind `now`.
pub fn is_past(run_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    run_at + RUN_DONE_AFTER < now
}

/// [`is_past`] for a zoned slot: v4 adds the grace to the wall clock.
///
/// # Errors
/// [`ScheduleError::DateOutOfRange`].
pub fn is_past_slot(slot: &ZonedDateTime, now: DateTime<Utc>) -> Result<bool, ScheduleError> {
    let wall = slot
        .wall()
        .checked_add_signed(RUN_DONE_AFTER)
        .ok_or(crate::domain::time::DateOutOfRange)?;
    let end = ZonedDateTime::new(wall, slot.zone())?.to_fixed();
    Ok(end < now)
}

/// Mark live runs whose slot has passed `done` and drop their unsent pings.
/// Returns the ids that changed, in `(datetime, id)` order.
pub fn mark_done(draft: &mut Draft, now: DateTime<Utc>) -> Vec<String> {
    let mut changed = Vec::new();
    for run in draft.runs(None) {
        if !run.status.is_live() || !is_past(run.datetime, now) {
            continue;
        }
        draft.set_run_status(&run.id, RunStatus::Done);
        draft.delete_unsent_reminders(&run.id, &[]);
        changed.push(run.id);
    }
    changed
}

/// Push the touched fields of an edited weekly timing onto its runs in
/// `week_starts`; done/cancelled runs are left as the record. Only a weekday or
/// time change re-snaps the slot (and rebuilds reminders), so editing a note
/// never undoes an amend. Returns how many runs were touched.
///
/// v5: a participants edit also drops the RSVPs of members it took off each
/// touched run (v4 kept them as orphans) and re-derives its status as swap does.
///
/// # Errors
/// [`ScheduleError::RunMoveConflict`] or [`ScheduleError::DateOutOfRange`].
pub fn apply_fixed_to_runs(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    fixed_id: &str,
    changed: &[FixedField],
    week_starts: &[impl AwareDateTime],
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<usize, ScheduleError> {
    let push = FixedPush {
        fixed_id,
        changed,
        keep_slot: &BTreeSet::new(),
    };
    push.apply(draft, ids, week_starts, policy, now)
}

/// One weekly-timing edit being pushed onto its runs.
pub(super) struct FixedPush<'a> {
    pub fixed_id: &'a str,
    pub changed: &'a [FixedField],
    /// Amended runs whose owner chose to keep this week's slot.
    pub keep_slot: &'a BTreeSet<String>,
}

impl FixedPush<'_> {
    pub(super) fn apply(
        &self,
        draft: &mut Draft,
        ids: &mut impl IdGenerator,
        week_starts: &[impl AwareDateTime],
        policy: &ReminderPolicy,
        now: DateTime<Utc>,
    ) -> Result<usize, ScheduleError> {
        let Some(fixed) = draft.fixed_run(self.fixed_id).cloned() else {
            return Ok(0);
        };
        if self.changed.is_empty() {
            return Ok(0);
        }
        let touched_field = |field| self.changed.contains(&field);
        let reschedule = touched_field(FixedField::Weekday) || touched_field(FixedField::Time);
        let mut touched = 0;
        for week_start in week_starts {
            let start = week_start.astimezone(policy.zone)?;
            let week = utc_instant(&start)?;
            let Some(run) = draft.run_for_fixed(self.fixed_id, week).cloned() else {
                continue;
            };
            if run.status.is_terminal() {
                continue;
            }
            if touched_field(FixedField::Bosses) {
                draft.set_run_bosses(&run.id, fixed.bosses.clone());
            }
            if touched_field(FixedField::Participants) && run.participants != fixed.participants {
                draft.set_run_participants(&run.id, fixed.participants.clone(), now);
                for user in run
                    .participants
                    .iter()
                    .filter(|u| !fixed.participants.contains(u))
                {
                    draft.clear_rsvp(&run.id, user);
                }
                // As swap: a dropped lone "no" must not leave a stale at_risk.
                recompute_after_roster_change(draft, &run.id, now)?;
            }
            if touched_field(FixedField::ChannelId) {
                draft.set_run_channel(&run.id, fixed.channel_id.clone());
            }
            if reschedule && !self.keep_slot.contains(&run.id) {
                let slot = slot_in_week(&start, policy.zone, fixed.weekday, fixed.time)?;
                let at = slot.to_fixed().with_timezone(&Utc);
                draft.set_run_datetime(&run.id, at, week)?;
                // v5: a move ends the pin.
                if draft.attendance().mode == AttendanceMode::V5 && at != run.datetime {
                    draft.set_run_pin(&run.id, None);
                    recompute_after_roster_change(draft, &run.id, now)?;
                }
                refresh_run_reminders(draft, ids, &run.id, policy, now)?;
            }
            touched += 1;
        }
        Ok(touched)
    }
}

/// Delete a weekly timing and cancel its live runs in `week_starts`, rebuilding
/// their reminders to none. Returns how many runs were cancelled.
///
/// # Errors
/// [`ScheduleError::DateOutOfRange`].
pub fn retire_fixed_run(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    fixed_id: &str,
    week_starts: &[impl AwareDateTime],
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<usize, ScheduleError> {
    let mut cancelled = 0;
    for week_start in week_starts {
        let week = utc_instant(&week_start.astimezone(policy.zone)?)?;
        let Some(run) = draft.run_for_fixed(fixed_id, week).cloned() else {
            continue;
        };
        if run.status.is_terminal() {
            continue;
        }
        draft.set_run_status(&run.id, RunStatus::Cancelled);
        refresh_run_reminders(draft, ids, &run.id, policy, now)?;
        cancelled += 1;
    }
    draft.delete_fixed_run(fixed_id);
    Ok(cancelled)
}
