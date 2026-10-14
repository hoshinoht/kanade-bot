//! Run mutations behind every surface: status, move, swap and reset.

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

use super::draft::Draft;
use super::error::ScheduleError;
use super::notice::{Notice, NoticeChange, Outcome};
use super::policy::{SchedulePolicy, utc_instant};
use super::reminders::{ReminderPolicy, refresh_run_reminders};
use super::roster::{RunState, named, run_state, validate_participants};
use super::rsvp::recompute_after_roster_change;
use super::run::{Run, RunStatus};
use crate::domain::attendance::{AttendanceMode, StatusPin};
use crate::domain::ids::{IdGenerator, short_id};
use crate::domain::members::Directory;
use crate::domain::time::{AwareDateTime, DateOutOfRange};
use crate::domain::weeks::slot_in_week;

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// v4 `formatting.local_day`, e.g. `Thu 03 Sep`.
fn local_day(at: &impl AwareDateTime, zone: Tz) -> Result<String, DateOutOfRange> {
    let wall = at.astimezone(zone)?.wall();
    Ok(format!(
        "{} {:02} {}",
        WEEKDAY_NAMES[wall.weekday().num_days_from_monday() as usize],
        wall.day(),
        MONTH_NAMES[wall.month0() as usize]
    ))
}

/// v4 `formatting.local_time`, e.g. `21:30`.
fn local_time(at: &impl AwareDateTime, zone: Tz) -> Result<String, DateOutOfRange> {
    let wall = at.astimezone(zone)?.wall();
    Ok(format!("{:02}:{:02}", wall.hour(), wall.minute()))
}

/// v4 `SETTABLE_STATUSES` gate: `at_risk` is derived, never set by hand.
///
/// # Errors
/// [`ScheduleError::NotSettable`] for `at_risk` or unknown text.
pub fn settable_status(text: &str) -> Result<RunStatus, ScheduleError> {
    match RunStatus::parse(text) {
        Ok(status) if status != RunStatus::AtRisk => Ok(status),
        _ => Err(ScheduleError::NotSettable(text.to_owned())),
    }
}

fn notice(run: &Run, change: NoticeChange, listed: Vec<String>, via_portal: bool) -> Notice {
    Notice {
        change,
        channel_id: run.channel_id.clone(),
        listed,
        via_portal,
    }
}

/// A requested status change and how it is announced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusChange {
    pub status: RunStatus,
    /// Ask for a channel notice at all (v4 `announce`).
    pub announce: bool,
    /// Made outside the channel (v4 `mark`, the `(via portal)` suffix).
    pub via_portal: bool,
}

/// Move a run to an explicitly chosen status (v4 `service.set_status`).
///
/// Reminders are rebuilt from the new status. Coming back to `planned` from
/// `cancelled`/`otot`/`done` clears the participants' answers. An unchanged
/// status does nothing and asks for no notice.
///
/// # Errors
/// [`ScheduleError::NotSettable`], [`ScheduleError::UnknownRun`] or
/// [`ScheduleError::DateOutOfRange`].
pub fn set_status(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    change: StatusChange,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<Outcome<RunState>, ScheduleError> {
    let StatusChange {
        status,
        announce,
        via_portal,
    } = change;
    if status == RunStatus::AtRisk {
        return Err(ScheduleError::NotSettable(status.as_str().into()));
    }
    let run = draft.require_run(run_id)?;
    let previous = run.status;
    // v5: a hand-set planned or confirmed is pinned (kept by derivation);
    // setting it again re-pins, other statuses clear the pin.
    let pin = (draft.attendance().mode == AttendanceMode::V5
        && matches!(status, RunStatus::Planned | RunStatus::Confirmed))
    .then_some(StatusPin { status, at: now });
    if previous == status {
        if draft.attendance().mode == AttendanceMode::V5 && run.status_pin != pin {
            draft.set_run_pin(run_id, pin);
        }
        return Ok(Outcome::quiet(run_state(draft, run_id)?));
    }
    if status == RunStatus::Planned
        && matches!(
            previous,
            RunStatus::Cancelled | RunStatus::Otot | RunStatus::Done
        )
    {
        for user in &run.participants {
            draft.clear_rsvp(run_id, user);
        }
    }
    draft.set_run_status(run_id, status);
    if draft.attendance().mode == AttendanceMode::V5 {
        draft.set_run_pin(run_id, pin);
    }
    refresh_run_reminders(draft, ids, run_id, policy, now)?;
    let state = run_state(draft, run_id)?;
    let notices = if announce {
        vec![notice(
            &state.run,
            NoticeChange::RunStatus {
                run_id: run_id.to_owned(),
                from: previous,
                to: status,
            },
            state.run.participants.clone(),
            via_portal,
        )]
    } else {
        Vec::new()
    };
    Ok(Outcome {
        value: state,
        notices,
    })
}

/// Move a run to `to` (v4 `service.amend_run`), into the boss week of `to`.
///
/// A weekly's run cannot move into a week that weekly already has a run in.
/// Answers given about the old slot no longer settle it, so `confirmed` and
/// `at_risk` go back to `planned`; reminders are rebuilt.
///
/// # Errors
/// [`ScheduleError::MoveConflict`], [`ScheduleError::UnknownRun`],
/// [`ScheduleError::RunEnded`] for a live run past its end, or
/// [`ScheduleError::DateOutOfRange`].
pub fn amend_run(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    to: DateTime<Utc>,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Outcome<RunState>, ScheduleError> {
    let run = draft.require_run(run_id)?;
    draft.refuse_ended(run_id, now)?;
    let zone = policy.zone();
    let week_start = policy.week_of(&to)?;
    let week = utc_instant(&week_start)?;
    let conflict = run
        .fixed_run_id
        .as_deref()
        .and_then(|fixed| draft.run_for_fixed(fixed, week))
        .filter(|other| other.id != run.id);
    if let Some(existing) = conflict {
        return Err(ScheduleError::MoveConflict {
            week_day: local_day(&week_start, zone)?,
            existing_short_id: short_id(&existing.id),
            existing_day: local_day(&existing.datetime, zone)?,
            existing_time: local_time(&existing.datetime, zone)?,
        });
    }
    draft.set_run_datetime(run_id, to, week)?;
    settle_after_move(draft, ids, run_id, &policy.reminders, now)?;
    let state = run_state(draft, run_id)?;
    let intent = notice(
        &state.run,
        NoticeChange::RunMoved {
            run_id: run_id.to_owned(),
            from: run.datetime,
            to: state.run.datetime,
        },
        state.run.participants.clone(),
        true,
    );
    Ok(Outcome {
        value: state,
        notices: vec![intent],
    })
}

/// Exchange two runs' local day and clock in one draft. Own-time runs retain
/// their clock, so they exchange only the day. Both runs must remain in their
/// existing boss week; this is a planner operation, not two independent moves.
pub fn swap_run_slots(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    with_id: &str,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Outcome<Vec<RunState>>, ScheduleError> {
    if run_id == with_id {
        return Err(ScheduleError::SameRunSwap);
    }
    let first = draft.require_run(run_id)?;
    let second = draft.require_run(with_id)?;
    if first.week_start != second.week_start {
        return Err(ScheduleError::DifferentSwapWeek);
    }
    if first.status.is_terminal() {
        return Err(ScheduleError::RunNotLive {
            run_id: first.id.clone(),
            status: first.status.as_str().to_owned(),
        });
    }
    if second.status.is_terminal() {
        return Err(ScheduleError::RunNotLive {
            run_id: second.id.clone(),
            status: second.status.as_str().to_owned(),
        });
    }
    draft.refuse_ended(run_id, now)?;
    draft.refuse_ended(with_id, now)?;

    let zone = policy.zone();
    let first_local = zone.from_utc_datetime(&first.datetime.naive_utc());
    let second_local = zone.from_utc_datetime(&second.datetime.naive_utc());
    let first_time = if first.status == RunStatus::Otot || second.status == RunStatus::Otot {
        first_local.time()
    } else {
        second_local.time()
    };
    let second_time = if first.status == RunStatus::Otot || second.status == RunStatus::Otot {
        second_local.time()
    } else {
        first_local.time()
    };
    let first_to = zone
        .from_local_datetime(&second_local.date_naive().and_time(first_time))
        .earliest()
        .ok_or(DateOutOfRange)?
        .with_timezone(&Utc);
    let second_to = zone
        .from_local_datetime(&first_local.date_naive().and_time(second_time))
        .earliest()
        .ok_or(DateOutOfRange)?
        .with_timezone(&Utc);
    let week = first.week_start;
    // Day zero begins at the configured reset time, so swapping an earlier
    // clock onto it can otherwise silently assign the previous boss week.
    if policy.week_of(&first_to)?.to_fixed() != week
        || policy.week_of(&second_to)?.to_fixed() != week
    {
        return Err(ScheduleError::SwapLeavesWeek);
    }
    draft.set_run_datetime(run_id, first_to, week)?;
    draft.set_run_datetime(with_id, second_to, week)?;
    settle_after_move(draft, ids, run_id, &policy.reminders, now)?;
    settle_after_move(draft, ids, with_id, &policy.reminders, now)?;
    let first_state = run_state(draft, run_id)?;
    let second_state = run_state(draft, with_id)?;
    let notices = vec![
        notice(
            &first_state.run,
            NoticeChange::RunMoved {
                run_id: run_id.to_owned(),
                from: first.datetime,
                to: first_state.run.datetime,
            },
            first_state.run.participants.clone(),
            true,
        ),
        notice(
            &second_state.run,
            NoticeChange::RunMoved {
                run_id: with_id.to_owned(),
                from: second.datetime,
                to: second_state.run.datetime,
            },
            second_state.run.participants.clone(),
            true,
        ),
    ];
    Ok(Outcome {
        value: vec![first_state, second_state],
        notices,
    })
}

/// The amend status rule on the run's current status, plus a reminder rebuild.
/// v5: a move ends the pin and re-derives.
fn settle_after_move(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    policy: &ReminderPolicy,
    now: DateTime<Utc>,
) -> Result<(), ScheduleError> {
    let status = draft.require_run(run_id)?.status;
    if matches!(status, RunStatus::Confirmed | RunStatus::AtRisk) {
        draft.set_run_status(run_id, RunStatus::Planned);
    }
    if draft.attendance().mode == AttendanceMode::V5 {
        draft.set_run_pin(run_id, None);
        if status.is_live() {
            recompute_after_roster_change(draft, run_id, now)?;
        }
    }
    refresh_run_reminders(draft, ids, run_id, policy, now)
}

/// Change who is on a run for this week only (v4 `service.swap_participants`).
///
/// Leavers' answers are cleared and the status is re-derived; a run may not be
/// emptied, and a swap that changes nothing asks for no notice.
///
/// # Errors
/// Participant validation errors, [`ScheduleError::NotOnRun`],
/// [`ScheduleError::RunEmptied`] or [`ScheduleError::UnknownRun`].
pub fn swap_participants(
    draft: &mut Draft,
    directory: &impl Directory,
    run_id: &str,
    remove: &[String],
    add: &[String],
    via_portal: bool,
    now: DateTime<Utc>,
) -> Result<Outcome<RunState>, ScheduleError> {
    let run = draft.require_run(run_id)?;
    draft.refuse_ended(run_id, now)?;
    let leaving: Vec<String> = remove.to_vec();
    let joining = if add.is_empty() {
        Vec::new()
    } else {
        validate_participants(directory, add)?
    };
    let mut people: Vec<String> = run
        .participants
        .iter()
        .filter(|uid| !leaving.contains(uid))
        .cloned()
        .collect();
    let unknown: Vec<String> = leaving
        .iter()
        .filter(|uid| !run.participants.contains(uid))
        .map(|uid| named(directory, uid))
        .collect();
    if !unknown.is_empty() {
        return Err(ScheduleError::NotOnRun(unknown));
    }
    for uid in &joining {
        if !people.contains(uid) {
            people.push(uid.clone());
        }
    }
    if people.is_empty() {
        return Err(ScheduleError::RunEmptied);
    }
    if people == run.participants {
        return Ok(Outcome::quiet(run_state(draft, run_id)?));
    }
    draft.set_run_participants(run_id, people, now);
    for uid in &leaving {
        draft.clear_rsvp(run_id, uid);
    }
    recompute_after_roster_change(draft, run_id, now)?;
    let state = run_state(draft, run_id)?;
    let listed = state
        .run
        .participants
        .iter()
        .chain(&leaving)
        .cloned()
        .collect();
    let intent = notice(
        &state.run,
        NoticeChange::RunSwapped {
            run_id: run_id.to_owned(),
            participants: state.run.participants.clone(),
            leaving,
            joining,
        },
        listed,
        via_portal,
    );
    Ok(Outcome {
        value: state,
        notices: vec![intent],
    })
}

/// v5: put an amended run back on its weekly timing for its own boss week.
///
/// Slot, bosses, participants and home channel come from the weekly timing.
/// Members taken off lose their answers and the status is re-derived as swap
/// does; then the amend rules apply (`confirmed`/`at_risk` back to `planned`,
/// reminders rebuilt). A run that already matches is left untouched and asks
/// for no notice. A restored slot at or before `now` is refused: it would
/// retire a live run into the past.
///
/// The run stays in its own boss week, whose weekly already owns it, so no
/// move conflict can arise.
///
/// # Errors
/// [`ScheduleError::UnknownRun`], [`ScheduleError::NotAFixedRun`],
/// [`ScheduleError::FixedRunRetired`], [`ScheduleError::RunNotLive`],
/// [`ScheduleError::RunEnded`], [`ScheduleError::ResetSlotPassed`] or
/// [`ScheduleError::DateOutOfRange`].
pub fn reset_to_fixed(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Outcome<RunState>, ScheduleError> {
    let run = draft.require_run(run_id)?;
    let Some(fixed_id) = run.fixed_run_id.clone() else {
        return Err(ScheduleError::NotAFixedRun(run_id.to_owned()));
    };
    let Some(fixed) = draft.fixed_run(&fixed_id).cloned() else {
        return Err(ScheduleError::FixedRunRetired(fixed_id));
    };
    if run.status.is_terminal() {
        return Err(ScheduleError::RunNotLive {
            run_id: run_id.to_owned(),
            status: run.status.as_str().to_owned(),
        });
    }
    draft.refuse_ended(run_id, now)?;
    // Re-derive the zoned week so a reset inside a DST gap keeps its wall clock.
    let week_start = policy.week_of(&run.week_start)?;
    let slot = slot_in_week(&week_start, policy.zone(), fixed.weekday, fixed.time)?;
    let slot = slot.to_fixed().with_timezone(&Utc);
    if slot <= now {
        return Err(ScheduleError::ResetSlotPassed {
            run_id: run_id.to_owned(),
            slot_day: local_day(&slot, policy.zone())?,
            slot_time: local_time(&slot, policy.zone())?,
        });
    }
    if run.datetime == slot
        && run.bosses == fixed.bosses
        && run.participants == fixed.participants
        && run.channel_id == fixed.channel_id
    {
        return Ok(Outcome::quiet(run_state(draft, run_id)?));
    }
    draft.set_run_datetime(run_id, slot, run.week_start)?;
    draft.set_run_bosses(run_id, fixed.bosses.clone());
    draft.set_run_channel(run_id, fixed.channel_id.clone());
    if run.participants != fixed.participants {
        draft.set_run_participants(run_id, fixed.participants.clone(), now);
        for uid in run
            .participants
            .iter()
            .filter(|u| !fixed.participants.contains(u))
        {
            draft.clear_rsvp(run_id, uid);
        }
        recompute_after_roster_change(draft, run_id, now)?;
    }
    settle_after_move(draft, ids, run_id, &policy.reminders, now)?;
    let state = run_state(draft, run_id)?;
    let intent = notice(
        &state.run,
        NoticeChange::RunReset {
            run_id: run_id.to_owned(),
            from: run.datetime,
            to: state.run.datetime,
        },
        state.run.participants.clone(),
        true,
    );
    Ok(Outcome {
        value: state,
        notices: vec![intent],
    })
}
