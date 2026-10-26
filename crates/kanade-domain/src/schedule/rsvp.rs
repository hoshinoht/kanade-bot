//! The RSVP state machine: reactions, tallies and derived run status.

use chrono::{DateTime, Utc};

use super::draft::Draft;
use super::error::ScheduleError;
use super::run::{RsvpSource, RsvpState, Run, RunStatus};
use crate::attendance::{AnswerState, DerivedStatus, derive_status, states_of};

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

/// Every participant's answer state for `run`.
pub fn answer_states(draft: &Draft, run: &Run) -> Vec<(String, AnswerState)> {
    let timing = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| draft.fixed_run(id));
    states_of(run, &draft.rsvps(&run.id), timing)
}

/// A run's status is frozen once it has started (`now >= start`): only a
/// hand-set status changes it then.
pub fn is_frozen(run: &Run, now: DateTime<Utc>) -> bool {
    now >= run.datetime
}

/// A run's status derived from `current` and its answers
/// ([`derive_status`]). A started live run keeps its status (frozen) and a
/// pinned one keeps its hand-set status (the unknown window does not
/// override it).
pub fn derive_run_status(
    draft: &Draft,
    run: &Run,
    current: RunStatus,
    now: DateTime<Utc>,
) -> DerivedStatus {
    if run.status.is_live() {
        let kept = if is_frozen(run, now) {
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
    pub new_status: RunStatus,
}

impl ReactionResult {
    pub fn declined(&self) -> bool {
        self.applied && self.state == Some(RsvpState::No)
    }
}

/// Record or undo one `✅`/`❌` reaction and recompute the run's status.
///
/// Non-participants and other emoji are ignored; removing a reaction clears
/// the RSVP only when it is the answer currently recorded.
///
/// # Errors
/// [`ScheduleError::UnknownRun`], or [`ScheduleError::RunEnded`] for a
/// participant's answer on a live run past its end (frozen).
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
        new_status: run.status,
    };
    let Some(state) = state_for_emoji(emoji) else {
        return Ok(result);
    };
    if !run.participants.iter().any(|user| user == user_id) {
        return Ok(result);
    }
    draft.refuse_ended(run_id, now)?;
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
    // An explicit answer (or taking one back) ends a hand-set status, unless
    // the run has started (frozen).
    if run.status_pin.is_some() && !is_frozen(&run, now) {
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
