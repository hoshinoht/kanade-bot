//! v5 attendance writes on a weekly timing: members' standing answers and
//! the timing's default for unanswered members (`domain::attendance`).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::draft::Draft;
use super::error::ScheduleError;
use super::notice::Outcome;
use super::roster::{RunState, run_state};
use super::rsvp::{answer_states, derive_run_status};
use super::run::FixedRun;
use crate::domain::attendance::{
    AttendanceActor, AttendanceDefault, AttendanceMode, AttendanceRecord, StandingAnswer,
    check_attendance, recorded_or_prefill,
};

fn timing(draft: &Draft, fixed_id: &str) -> Result<FixedRun, ScheduleError> {
    draft
        .fixed_run(fixed_id)
        .cloned()
        .ok_or_else(|| ScheduleError::UnknownFixedRun(fixed_id.to_owned()))
}

/// Set (`on`) or clear a member's standing answer ("always in") on a weekly
/// timing. Only party members may have one; setting it again keeps the
/// original, clearing an absent one does nothing.
///
/// # Errors
/// [`ScheduleError::UnknownFixedRun`] or [`ScheduleError::NotInParty`].
pub fn set_standing_answer(
    draft: &mut Draft,
    fixed_id: &str,
    user_id: &str,
    on: bool,
    set_by: &str,
    now: DateTime<Utc>,
) -> Result<Outcome<FixedRun>, ScheduleError> {
    let row = timing(draft, fixed_id)?;
    let held = row.standing.iter().any(|answer| answer.user_id == user_id);
    if on {
        if !row.participants.iter().any(|user| user == user_id) {
            return Err(ScheduleError::NotInParty {
                user_id: user_id.to_owned(),
            });
        }
        if !held {
            draft.set_standing(
                fixed_id,
                user_id,
                Some(StandingAnswer {
                    user_id: user_id.to_owned(),
                    set_by: set_by.to_owned(),
                    at: now,
                }),
            );
            rederive(draft, Some(fixed_id), now);
        }
    } else if held {
        draft.set_standing(fixed_id, user_id, None);
        rederive(draft, Some(fixed_id), now);
    }
    Ok(Outcome::quiet(timing(draft, fixed_id)?))
}

/// v5: re-derive the status of every live, not-yet-started run (of one
/// timing, or all) at `now`; returns the runs that changed. Nothing in
/// v4-compat mode.
fn rederive(draft: &mut Draft, fixed_id: Option<&str>, now: DateTime<Utc>) -> Vec<String> {
    if draft.attendance().mode == AttendanceMode::V4Compat {
        return Vec::new();
    }
    let mut changed = Vec::new();
    for run in draft.runs(None) {
        let of_timing = fixed_id.is_none_or(|id| run.fixed_run_id.as_deref() == Some(id));
        if !of_timing || !run.status.is_live() || run.datetime <= now {
            continue;
        }
        let status = derive_run_status(draft, &run, run.status, now).status;
        if status != run.status {
            draft.set_run_status(&run.id, status);
            changed.push(run.id);
        }
    }
    changed
}

/// Record who attended a done run (`attended`: the whole attended set as
/// the caller sees it). Every participant without an entry stands at the
/// prefill (everyone not declined), so a member may confirm their own
/// entry from the prefill. An administrator writes an entry for every
/// participant; a member writes only their own. Entries that already say
/// the same are kept as recorded (who and when).
///
/// # Errors
/// [`ScheduleError::UnknownRun`] or [`ScheduleError::Attendance`]
/// ([`check_attendance`]).
pub fn record_attendance(
    draft: &mut Draft,
    run_id: &str,
    actor: &AttendanceActor,
    attended: &BTreeSet<String>,
    recorded_by: &str,
    now: DateTime<Utc>,
) -> Result<Outcome<RunState>, ScheduleError> {
    let run = draft.require_run(run_id)?;
    let states = answer_states(draft, &run);
    let recorded = recorded_or_prefill(&states, &run.attendance);
    check_attendance(actor, run.status, &run.participants, &recorded, attended)
        .map_err(ScheduleError::Attendance)?;
    let writes: Vec<&String> = match actor {
        AttendanceActor::Admin => run.participants.iter().collect(),
        AttendanceActor::Member(me) => run.participants.iter().filter(|user| *user == me).collect(),
    };
    let mut records = run.attendance.clone();
    for user in writes {
        let value = attended.contains(user);
        let current = records.iter().position(|record| &record.user_id == user);
        if current.is_some_and(|index| records[index].attended == value) {
            continue;
        }
        let record = AttendanceRecord {
            user_id: user.clone(),
            attended: value,
            recorded_by: recorded_by.to_owned(),
            at: now,
        };
        match current {
            Some(index) => records[index] = record,
            None => records.push(record),
        }
    }
    if records != run.attendance {
        draft.set_run_attendance(run_id, records);
    }
    Ok(Outcome::quiet(run_state(draft, run_id)?))
}

/// v5: re-derive every live run's status under the draft's attendance rules
/// at `now` (the tick runs this so unknown answers make a run at risk once
/// its window opens, and assumed ones confirm it). Returns the runs whose
/// status changed. A no-op in v4-compat mode.
pub fn recount_attendance(draft: &mut Draft, now: DateTime<Utc>) -> Vec<String> {
    rederive(draft, None, now)
}

/// Set a weekly timing's attendance default; in v5 its live, not-yet-started
/// runs are re-derived at `now` in the same change.
///
/// # Errors
/// [`ScheduleError::UnknownFixedRun`].
pub fn set_attendance_default(
    draft: &mut Draft,
    fixed_id: &str,
    default: AttendanceDefault,
    now: DateTime<Utc>,
) -> Result<Outcome<FixedRun>, ScheduleError> {
    if timing(draft, fixed_id)?.attendance_default == default {
        return Ok(Outcome::quiet(timing(draft, fixed_id)?));
    }
    draft.set_attendance_default(fixed_id, default);
    rederive(draft, Some(fixed_id), now);
    Ok(Outcome::quiet(timing(draft, fixed_id)?))
}
