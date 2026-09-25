//! Editing a weekly timing and pushing the edit onto its materialised runs.
//!
//! v4 re-snaps every live run of the timing when its day or time changes, which
//! silently undoes a move someone made for this week. v5 lists those amended
//! runs first and takes an explicit choice for each; [`FixedEditChoices::UpdateAll`]
//! keeps v4's behaviour for callers that do not ask.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveTime, Utc, Weekday};

use super::draft::Draft;
use super::error::ScheduleError;
use super::lifecycle::FixedPush;
use super::materialise::materialise_weeks;
use super::notice::{Notice, NoticeChange, Outcome};
use super::policy::{SchedulePolicy, utc_instant};
use super::roster::{validate_channel, validate_participants};
use super::rsvp::recompute_after_roster_change;
use super::run::{FixedField, FixedRun, FixedRunPatch};
use crate::domain::ids::IdGenerator;
use crate::domain::members::Directory;
use crate::domain::weeks::slot_in_week;

/// Requested weekly-timing fields; `None` leaves a field alone. Bosses, day
/// and time arrive parsed; participants and channel are validated here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixedEdit {
    pub bosses: Option<Vec<String>>,
    pub weekday: Option<Weekday>,
    pub time: Option<NaiveTime>,
    pub participants: Option<Vec<String>>,
    pub channel_id: Option<String>,
    pub note: Option<String>,
}

/// A live run of the timing that was moved off its weekly slot this week and
/// that a day/time edit would move again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmendedRun {
    pub run_id: String,
    pub week_start: DateTime<Utc>,
    /// Where the run is now (its amended slot).
    pub datetime: DateTime<Utc>,
    /// The weekly slot before the edit.
    pub fixed_slot: DateTime<Utc>,
    /// Where [`AmendedRunChoice::UpdateToFixed`] would put it.
    pub new_slot: DateTime<Utc>,
}

/// What happens to one amended run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmendedRunChoice {
    /// Re-snap to the edited weekly slot (v4).
    UpdateToFixed,
    /// Keep this week's amended slot; later weeks follow the edit.
    KeepForThisWeek,
}

/// Choices for the runs [`preview_fixed_edit`] listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FixedEditChoices {
    /// v4 default: every affected run follows the edit.
    #[default]
    UpdateAll,
    /// Exactly one choice per affected run, keyed by run id.
    PerRun(BTreeMap<String, AmendedRunChoice>),
}

/// The amended runs a day/time edit would move, in materialised-week order.
///
/// # Errors
/// [`ScheduleError::UnknownFixedRun`] or [`ScheduleError::DateOutOfRange`].
pub fn preview_fixed_edit(
    draft: &Draft,
    fixed_id: &str,
    edit: &FixedEdit,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Vec<AmendedRun>, ScheduleError> {
    let fixed = draft
        .fixed_run(fixed_id)
        .ok_or_else(|| ScheduleError::UnknownFixedRun(fixed_id.to_owned()))?;
    if edit.weekday.is_none() && edit.time.is_none() {
        return Ok(Vec::new());
    }
    let weekday = edit.weekday.unwrap_or(fixed.weekday);
    let time = edit.time.unwrap_or(fixed.time);
    let zone = policy.zone();
    let mut affected = Vec::new();
    for week in policy.materialised_weeks(now)? {
        let week_start = utc_instant(&week)?;
        let Some(run) = draft.run_for_fixed(fixed_id, week_start) else {
            continue;
        };
        if run.status.is_terminal() {
            continue;
        }
        let slot = |weekday, time| {
            slot_in_week(&week, zone, weekday, time).map(|at| at.to_fixed().with_timezone(&Utc))
        };
        let fixed_slot = slot(fixed.weekday, fixed.time)?;
        let new_slot = slot(weekday, time)?;
        if run.datetime != fixed_slot && run.datetime != new_slot {
            affected.push(AmendedRun {
                run_id: run.id.clone(),
                week_start,
                datetime: run.datetime,
                fixed_slot,
                new_slot,
            });
        }
    }
    Ok(affected)
}

/// The runs whose amended slot is kept, after checking the choices match.
fn kept_runs(
    affected: &[AmendedRun],
    choices: &FixedEditChoices,
) -> Result<BTreeSet<String>, ScheduleError> {
    let FixedEditChoices::PerRun(choices) = choices else {
        return Ok(BTreeSet::new());
    };
    if let Some(stray) = choices
        .keys()
        .find(|id| !affected.iter().any(|run| &run.run_id == *id))
    {
        return Err(ScheduleError::UnexpectedChoice(stray.clone()));
    }
    if let Some(missing) = affected
        .iter()
        .find(|run| !choices.contains_key(&run.run_id))
    {
        return Err(ScheduleError::MissingChoice(missing.run_id.clone()));
    }
    Ok(choices
        .iter()
        .filter(|(_, choice)| **choice == AmendedRunChoice::KeepForThisWeek)
        .map(|(id, _)| id.clone())
        .collect())
}

/// One weekly-timing edit with its per-run choices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedEditRequest {
    pub fixed_id: String,
    pub edit: FixedEdit,
    pub choices: FixedEditChoices,
}

/// Apply a weekly-timing edit (v4 `service.update_fixed`): push only touched
/// fields onto the live runs of the materialised weeks, then re-materialise.
///
/// # Errors
/// [`ScheduleError::UnknownFixedRun`], participant/channel validation errors,
/// [`ScheduleError::NothingToChange`], [`ScheduleError::UnexpectedChoice`],
/// [`ScheduleError::MissingChoice`] or [`ScheduleError::DateOutOfRange`].
pub fn apply_fixed_edit(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    directory: &impl Directory,
    request: &FixedEditRequest,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Outcome<FixedRun>, ScheduleError> {
    let fixed_id = request.fixed_id.as_str();
    let edit = &request.edit;
    if draft.fixed_run(fixed_id).is_none() {
        return Err(ScheduleError::UnknownFixedRun(fixed_id.to_owned()));
    }
    let mut patch = FixedRunPatch::default();
    let mut fields = Vec::new();
    if let Some(bosses) = &edit.bosses {
        patch.bosses = Some(bosses.clone());
        fields.push(FixedField::Bosses);
    }
    if let Some(weekday) = edit.weekday {
        patch.weekday = Some(weekday);
        fields.push(FixedField::Weekday);
    }
    if let Some(time) = edit.time {
        patch.time = Some(time);
        fields.push(FixedField::Time);
    }
    if let Some(participants) = &edit.participants {
        patch.participants = Some(validate_participants(directory, participants)?);
        fields.push(FixedField::Participants);
    }
    if let Some(channel) = &edit.channel_id {
        patch.channel_id = Some(validate_channel(directory, channel)?);
        fields.push(FixedField::ChannelId);
    }
    if let Some(note) = &edit.note {
        patch.note = Some(note.clone());
        fields.push(FixedField::Note);
    }
    if fields.is_empty() {
        return Err(ScheduleError::NothingToChange);
    }
    fields.sort_by_key(|field| field.as_str());
    let affected = preview_fixed_edit(draft, fixed_id, edit, policy, now)?;
    let keep_slot = kept_runs(&affected, &request.choices)?;

    draft.update_fixed_run(fixed_id, patch);
    let push = FixedPush {
        fixed_id,
        changed: &fields,
        keep_slot: &keep_slot,
    };
    push.apply(
        draft,
        ids,
        &policy.materialised_weeks(now)?,
        &policy.reminders,
        now,
    )?;
    materialise_weeks(draft, ids, policy, now)?;

    let updated = draft
        .fixed_run(fixed_id)
        .cloned()
        .ok_or_else(|| ScheduleError::UnknownFixedRun(fixed_id.to_owned()))?;
    let intent = Notice {
        change: NoticeChange::FixedChanged {
            fixed_id: fixed_id.to_owned(),
            fields,
            weekday: updated.weekday,
            time: updated.time,
            participants: updated.participants.clone(),
        },
        channel_id: updated.channel_id.clone(),
        listed: updated.participants.clone(),
        via_portal: true,
    };
    Ok(Outcome {
        value: updated,
        notices: vec![intent],
    })
}

/// `party` without `remove`, then with each of `add` not already on it,
/// order kept.
pub fn party_delta(party: &[String], add: &[String], remove: &[String]) -> Vec<String> {
    let mut next: Vec<String> = party
        .iter()
        .filter(|user| !remove.contains(user))
        .cloned()
        .collect();
    for user in add {
        if !next.contains(user) {
            next.push(user.clone());
        }
    }
    next
}

/// What [`apply_party_delta`] did: the timing, and the live runs it left
/// unchanged because the delta would have emptied them (reported to
/// administrators; never an error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartyDelta {
    pub fixed: FixedRun,
    pub emptied: Vec<String>,
}

/// v5 only: change a weekly timing's party by delta (a member request's
/// join, leave or swap). Unlike [`apply_fixed_edit`], which pushes the
/// timing's whole party onto its live runs, the same delta is applied to
/// each live run of the materialised weeks, so a one-off substitution on a
/// run and its RSVPs survive; removed members' RSVPs on those runs are
/// dropped (as a swap does). A run the delta would leave with nobody is
/// skipped and reported in [`PartyDelta::emptied`]. Missing weeks are then
/// materialised.
///
/// # Errors
/// [`ScheduleError::UnknownFixedRun`], participant validation errors or
/// [`ScheduleError::DateOutOfRange`].
#[allow(clippy::too_many_arguments)]
pub fn apply_party_delta(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    directory: &impl Directory,
    fixed_id: &str,
    add: &[String],
    remove: &[String],
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Outcome<PartyDelta>, ScheduleError> {
    let fixed = draft
        .fixed_run(fixed_id)
        .cloned()
        .ok_or_else(|| ScheduleError::UnknownFixedRun(fixed_id.to_owned()))?;
    let mut emptied = Vec::new();
    let party = validate_participants(directory, &party_delta(&fixed.participants, add, remove))?;
    draft.update_fixed_run(
        fixed_id,
        FixedRunPatch {
            participants: Some(party),
            ..FixedRunPatch::default()
        },
    );
    for week in policy.materialised_weeks(now)? {
        let week = utc_instant(&week)?;
        let Some(run) = draft.run_for_fixed(fixed_id, week).cloned() else {
            continue;
        };
        if run.status.is_terminal() {
            continue;
        }
        let next = party_delta(&run.participants, add, remove);
        if next == run.participants {
            continue;
        }
        if next.is_empty() {
            emptied.push(run.id.clone());
            continue;
        }
        draft.set_run_participants(&run.id, next);
        for user in run.participants.iter().filter(|user| remove.contains(user)) {
            draft.clear_rsvp(&run.id, user);
        }
        // As swap: a dropped lone "no" must not leave a stale at_risk.
        recompute_after_roster_change(draft, &run.id, now)?;
    }
    materialise_weeks(draft, ids, policy, now)?;
    let updated = draft
        .fixed_run(fixed_id)
        .cloned()
        .ok_or_else(|| ScheduleError::UnknownFixedRun(fixed_id.to_owned()))?;
    let intent = Notice {
        change: NoticeChange::FixedChanged {
            fixed_id: fixed_id.to_owned(),
            fields: vec![FixedField::Participants],
            weekday: updated.weekday,
            time: updated.time,
            participants: updated.participants.clone(),
        },
        channel_id: updated.channel_id.clone(),
        listed: updated.participants.clone(),
        via_portal: true,
    };
    Ok(Outcome {
        value: PartyDelta {
            fixed: updated,
            emptied,
        },
        notices: vec![intent],
    })
}
