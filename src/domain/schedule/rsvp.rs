//! The RSVP state machine: reactions, tallies and derived run status.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::draft::Draft;
use super::error::ScheduleError;
use super::run::{RsvpSource, RsvpState, Run, RunStatus};
use crate::domain::attendance::{
    AnswerState, AttendanceMode, DerivedStatus, derive_status, states_of,
};

pub const EMOJI_YES: &str = "\u{2705}";
pub const EMOJI_NO: &str = "\u{274c}";

/// `✅` is yes, `❌` is no; any other emoji is not an answer.
pub fn state_for_emoji(emoji: &str) -> Option<RsvpState> {
    match emoji {
        EMOJI_YES => Some(RsvpState::Yes),
        EMOJI_NO => Some(RsvpState::No),
        _ => None,
    }
}

/// Statuses a tally never overwrites: they were set deliberately.
fn is_sticky(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Cancelled | RunStatus::Otot | RunStatus::Done
    )
}

/// Derive a run's status from its tally.
///
/// A participant's "no" makes it `at_risk`; every participant saying yes makes
/// it `confirmed`; an incomplete tally keeps `confirmed` (silence does not undo
/// a decision) and is otherwise `planned`. Sticky statuses are unchanged and
/// answers from non-participants are ignored.
pub fn compute_status(
    current: RunStatus,
    participants: &[String],
    rsvps: &BTreeMap<String, RsvpState>,
) -> RunStatus {
    if is_sticky(current) {
        return current;
    }
    let answer = |user: &String| rsvps.get(user).copied();
    if participants
        .iter()
        .any(|user| answer(user) == Some(RsvpState::No))
    {
        return RunStatus::AtRisk;
    }
    if !participants.is_empty()
        && participants
            .iter()
            .all(|user| answer(user) == Some(RsvpState::Yes))
    {
        return RunStatus::Confirmed;
    }
    if current == RunStatus::Confirmed {
        RunStatus::Confirmed
    } else {
        RunStatus::Planned
    }
}

/// Every participant's answer state for `run` under the draft's attendance
/// rules (explicit answers only in v4-compat mode).
pub fn answer_states(draft: &Draft, run: &Run) -> Vec<(String, AnswerState)> {
    let timing = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| draft.fixed_run(id));
    states_of(run, &draft.rsvps(&run.id), timing, draft.attendance().mode)
}

/// v5: a run's status is frozen once it has started (`now >= start`):
/// only a hand-set status changes it then.
pub fn is_frozen(draft: &Draft, run: &Run, now: DateTime<Utc>) -> bool {
    draft.attendance().mode == AttendanceMode::V5 && now >= run.datetime
}

/// A run's status derived from `current` and its answers
/// ([`derive_status`]); in v4-compat mode exactly [`compute_status`]. In v5
/// a started run keeps its status (frozen) and a pinned run keeps its
/// hand-set status (the unknown window does not override it).
pub fn derive_run_status(
    draft: &Draft,
    run: &Run,
    current: RunStatus,
    now: DateTime<Utc>,
) -> DerivedStatus {
    if draft.attendance().mode == AttendanceMode::V5 && run.status.is_live() {
        let kept = if is_frozen(draft, run, now) {
            Some(run.status)
        } else {
            run.status_pin.map(|pin| pin.status)
        };
        if let Some(status) = kept {
            return DerivedStatus {
                status,
                expected: false,
            };
        }
    }
    let states: Vec<AnswerState> = answer_states(draft, run)
        .into_iter()
        .map(|(_, state)| state)
        .collect();
    derive_status(current, &states, run.datetime, now, draft.attendance())
}

/// Re-derive status after the line-up changed: a newcomer never agreed, so a
/// `confirmed` run goes back to whatever the tally says.
///
/// # Errors
/// [`ScheduleError::UnknownRun`].
pub fn recompute_after_roster_change(
    draft: &mut Draft,
    run_id: &str,
    now: DateTime<Utc>,
) -> Result<RunStatus, ScheduleError> {
    let run = draft.require_run(run_id)?;
    let derive_from = if run.status == RunStatus::Confirmed {
        RunStatus::Planned
    } else {
        run.status
    };
    let status = derive_run_status(draft, &run, derive_from, now).status;
    if status != run.status {
        draft.set_run_status(run_id, status);
    }
    Ok(status)
}

/// What one reaction did, so the caller knows whether to post anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionResult {
    pub run_id: String,
    pub applied: bool,
    pub state: Option<RsvpState>,
    pub old_status: RunStatus,
    pub new_status: RunStatus,
}

impl ReactionResult {
    pub fn declined(&self) -> bool {
        self.applied && self.state == Some(RsvpState::No)
    }

    pub fn status_changed(&self) -> bool {
        self.applied && self.old_status != self.new_status
    }
}

/// Record or undo one `✅`/`❌` reaction and recompute the run's status.
///
/// Non-participants and other emoji are ignored; removing a reaction clears
/// the RSVP only when it is the answer currently recorded.
///
/// # Errors
/// [`ScheduleError::UnknownRun`].
pub fn apply_reaction(
    draft: &mut Draft,
    run_id: &str,
    user_id: &str,
    emoji: &str,
    added: bool,
    now: DateTime<Utc>,
) -> Result<ReactionResult, ScheduleError> {
    let run = draft.require_run(run_id)?;
    let mut result = ReactionResult {
        run_id: run.id.clone(),
        applied: false,
        state: None,
        old_status: run.status,
        new_status: run.status,
    };
    let Some(state) = state_for_emoji(emoji) else {
        return Ok(result);
    };
    if !run.participants.iter().any(|user| user == user_id) {
        return Ok(result);
    }
    if added {
        draft.set_rsvp(run_id, user_id, state, RsvpSource::Reaction, now);
    } else {
        if draft.rsvps(run_id).get(user_id) != Some(&state) {
            return Ok(result);
        }
        draft.clear_rsvp(run_id, user_id);
    }
    result.applied = true;
    result.state = added.then_some(state);
    // v5: an explicit answer (or taking one back) ends a hand-set status,
    // unless the run has started (frozen).
    if run.status_pin.is_some() && !is_frozen(draft, &run, now) {
        draft.set_run_pin(run_id, None);
    }
    let run = draft.require_run(run_id)?;
    let new_status = derive_run_status(draft, &run, run.status, now).status;
    if new_status != run.status {
        draft.set_run_status(run_id, new_status);
    }
    result.new_status = new_status;
    Ok(result)
}
