//! Why a proposal cannot apply, in v4 `commit.py`'s words (they reach the
//! channel verbatim).

use std::fmt;

use super::change::ChangeKind;
use crate::domain::drafts::ReplayError;
use crate::domain::schedule::ScheduleError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    RunGone,
    NoNewTime,
    WeeklyHoldsWeek,
    NoDayAndTime,
    NoBosses,
    NobodyToSwap,
    RunEmptied,
    NoBossesFromRun,
    NoAnswer,
    NobodyNamed,
    AnswerForOutsider,
    NoRecurringSlot,
    NoTimingNamed,
    /// An edit's timing was retired.
    TimingGone,
    /// A removal's timing was already retired.
    TimingAlreadyGone,
    NothingLeftToChange,
    /// Any other scheduling rule, in its own words.
    Rule(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RunGone => "that run has gone",
            Self::NoNewTime => "no new time was agreed - use `/amend` to set one",
            Self::WeeklyHoldsWeek => {
                "that weekly already has a run in that boss week - edit that existing run \
                 instead or keep this move within its current boss week"
            }
            Self::NoDayAndTime => "no day and time were agreed - use `/amend` or `/fixed add`",
            Self::NoBosses => "no bosses were named",
            Self::NobodyToSwap => "nobody to swap in or out - use `/swap` or `/fixed edit`",
            Self::RunEmptied => "that would leave the run with nobody on it - cancel it instead",
            Self::NoBossesFromRun => "no bosses from that run were named",
            Self::NoAnswer => "no answer was given",
            Self::NobodyNamed => "nobody was named",
            Self::AnswerForOutsider => "that answer is for somebody who is no longer on the run",
            Self::NoRecurringSlot => "no recurring day and time were agreed - use `/fixed add`",
            Self::NoTimingNamed => "no weekly timing was named",
            Self::TimingGone => "that weekly timing has gone",
            Self::TimingAlreadyGone => "that weekly timing has already gone",
            Self::NothingLeftToChange => "nothing was left to change on that weekly timing",
            Self::Rule(text) => text,
        })
    }
}

impl std::error::Error for Refusal {}

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
