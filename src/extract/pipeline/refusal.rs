//! The structured extraction-log entry for a change refused up front.

use crate::domain::model_log::ExtractionRefusal;
use crate::domain::proposals::Refusal;
use crate::domain::scheduler::ProposalError;

/// A stable snake_case code for filtering and the admin portal; the message
/// keeps v4's words.
pub fn refusal_code(error: &ProposalError) -> &'static str {
    match error {
        ProposalError::Refused(refusal) => match refusal {
            Refusal::RunGone => "run_gone",
            Refusal::NoNewTime => "no_new_time",
            Refusal::WeeklyHoldsWeek => "weekly_holds_week",
            Refusal::NoDayAndTime => "no_day_and_time",
            Refusal::NoBosses => "no_bosses",
            Refusal::NobodyToSwap => "nobody_to_swap",
            Refusal::RunEmptied => "run_emptied",
            Refusal::NoBossesFromRun => "no_bosses_from_run",
            Refusal::NoAnswer => "no_answer",
            Refusal::NobodyNamed => "nobody_named",
            Refusal::AnswerForOutsider => "answer_for_outsider",
            Refusal::NoRecurringSlot => "no_recurring_slot",
            Refusal::NoTimingNamed => "no_timing_named",
            Refusal::TimingGone => "timing_gone",
            Refusal::TimingAlreadyGone => "timing_already_gone",
            Refusal::NothingLeftToChange => "nothing_left_to_change",
            Refusal::Rule(_) => "rule",
        },
        ProposalError::NoEffect => "no_effect",
        ProposalError::Expired => "expired",
        ProposalError::Unauthorised => "unauthorised",
        ProposalError::NotAProposal => "not_a_proposal",
        ProposalError::Draft(_) => "store",
    }
}

pub fn refusal(change: &str, error: &ProposalError) -> ExtractionRefusal {
    ExtractionRefusal {
        change: change.to_owned(),
        code: refusal_code(error).to_owned(),
        message: error.to_string(),
    }
}
