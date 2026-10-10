//! Who may be on a run, and a run's roster view.

use std::collections::BTreeMap;

use super::draft::Draft;
use super::error::ScheduleError;
use super::run::{RsvpState, Run};
use crate::domain::members::{Directory, member_name};
use crate::domain::pytext::{is_digit, strip};

/// `Name (id)` for error messages.
pub fn named(directory: &impl Directory, user_id: &str) -> String {
    format!("{} ({user_id})", member_name(directory, user_id))
}

/// v4 `validate_participants`: rostered humans only, no duplicates, not empty.
///
/// # Errors
/// [`ScheduleError::NotAUserId`], [`ScheduleError::NoParticipants`],
/// [`ScheduleError::NotInRole`] or [`ScheduleError::BotParticipants`].
pub fn validate_participants(
    directory: &impl Directory,
    ids: &[String],
) -> Result<Vec<String>, ScheduleError> {
    let mut out: Vec<String> = Vec::new();
    for raw in ids {
        let uid = strip(raw);
        if uid.is_empty() {
            continue;
        }
        if !uid.chars().all(is_digit) {
            return Err(ScheduleError::NotAUserId(uid.to_owned()));
        }
        if !out.iter().any(|seen| seen == uid) {
            out.push(uid.to_owned());
        }
    }
    if out.is_empty() {
        return Err(ScheduleError::NoParticipants);
    }
    let member = |uid: &String| directory.member(uid).unwrap_or_default();
    let outsiders: Vec<String> = out
        .iter()
        .filter(|uid| !member(uid).has_role)
        .map(|uid| named(directory, uid))
        .collect();
    if !outsiders.is_empty() {
        return Err(ScheduleError::NotInRole(outsiders));
    }
    let bots: Vec<String> = out
        .iter()
        .filter(|uid| member(uid).is_bot)
        .cloned()
        .collect();
    if !bots.is_empty() {
        return Err(ScheduleError::BotParticipants(bots));
    }
    Ok(out)
}

/// v4 `validate_channel`: a home channel must be watched.
///
/// # Errors
/// [`ScheduleError::ChannelNotWatched`].
pub fn validate_channel(
    directory: &impl Directory,
    channel_id: &str,
) -> Result<String, ScheduleError> {
    if directory.is_watched(channel_id) {
        Ok(channel_id.to_owned())
    } else {
        Err(ScheduleError::ChannelNotWatched(channel_id.to_owned()))
    }
}

/// This week's line-up against its weekly baseline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterChange {
    /// Baseline members not on this week's run.
    pub out: Vec<String>,
    /// Members on this week's run who are not in the baseline.
    pub joined: Vec<String>,
}

impl RosterChange {
    pub fn changed(&self) -> bool {
        !self.out.is_empty() || !self.joined.is_empty()
    }
}

/// A run with its answers and roster delta, as mutations report it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunState {
    pub run: Run,
    pub rsvps: BTreeMap<String, RsvpState>,
    pub roster_change: RosterChange,
}

/// The current state of one run.
///
/// # Errors
/// [`ScheduleError::UnknownRun`].
pub fn run_state(draft: &Draft, run_id: &str) -> Result<RunState, ScheduleError> {
    let run = draft.require_run(run_id)?;
    let baseline = run
        .fixed_run_id
        .as_deref()
        .and_then(|fixed| draft.fixed_run(fixed));
    let roster_change = baseline.map_or_else(RosterChange::default, |fixed| RosterChange {
        out: fixed
            .participants
            .iter()
            .filter(|uid| !run.participants.contains(uid))
            .cloned()
            .collect(),
        joined: run
            .participants
            .iter()
            .filter(|uid| !fixed.participants.contains(uid))
            .cloned()
            .collect(),
    });
    Ok(RunState {
        rsvps: draft.rsvps(run_id),
        run,
        roster_change,
    })
}
