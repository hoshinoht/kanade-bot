//! Why a proposal cannot apply, in v4 `commit.py`'s words (they reach the
//! channel verbatim).

use super::change::ChangeKind;
use crate::domain::drafts::ReplayError;
use crate::domain::schedule::ScheduleError;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("that run has gone")]
    RunGone,
    #[error("no new time was agreed - use `/amend` to set one")]
    NoNewTime,
    #[error(
        "that weekly already has a run in that boss week - edit that existing run \
         instead or keep this move within its current boss week"
    )]
    WeeklyHoldsWeek,
    #[error("no day and time were agreed - use `/amend` or `/fixed add`")]
    NoDayAndTime,
    #[error("no bosses were named")]
    NoBosses,
    #[error("nobody to swap in or out - use `/swap` or `/fixed edit`")]
    NobodyToSwap,
    #[error("that would leave the run with nobody on it - cancel it instead")]
    RunEmptied,
    #[error("no bosses from that run were named")]
    NoBossesFromRun,
    #[error("no answer was given")]
    NoAnswer,
    #[error("nobody was named")]
    NobodyNamed,
    #[error("that answer is for somebody who is no longer on the run")]
    AnswerForOutsider,
    #[error("no recurring day and time were agreed - use `/fixed add`")]
    NoRecurringSlot,
    #[error("no weekly timing was named")]
    NoTimingNamed,
    /// An edit's timing was retired.
    #[error("that weekly timing has gone")]
    TimingGone,
    /// A removal's timing was already retired.
    #[error("that weekly timing has already gone")]
    TimingAlreadyGone,
    #[error("nothing was left to change on that weekly timing")]
    NothingLeftToChange,
    /// Any other scheduling rule, in its own words.
    #[error("{0}")]
    Rule(String),
}

impl Refusal {
    /// A scheduling refusal of a `kind` proposal, in v4's words where v4
    /// `commit` had its own.
    pub fn from_schedule(kind: ChangeKind, removing: bool, error: &ScheduleError) -> Self {
        match error {
            ScheduleError::UnknownRun(_) => Self::RunGone,
            ScheduleError::UnknownFixedRun(_) if removing => Self::TimingAlreadyGone,
            ScheduleError::UnknownFixedRun(_) if kind == ChangeKind::Fix => Self::TimingGone,
            ScheduleError::RunEmptied => Self::RunEmptied,
            ScheduleError::MoveConflict { .. } | ScheduleError::RunMoveConflict => {
                Self::WeeklyHoldsWeek
            }
            ScheduleError::NothingToChange => Self::NothingLeftToChange,
            other => Self::Rule(other.to_string()),
        }
    }

    pub fn from_replay(kind: ChangeKind, removing: bool, error: &ReplayError) -> Self {
        match error {
            ReplayError::Schedule(error) => Self::from_schedule(kind, removing, error),
            other => Self::Rule(other.to_string()),
        }
    }
}
