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
        ProposalError::EditNotApplicable => "edit_not_applicable",
        ProposalError::EditInPast => "edit_in_past",
        ProposalError::Draft(_) => "store",
    }
}

/// Shown by the admin portal, so a store failure's own text (paths, SQLite
/// messages) is replaced by a generic line.
pub const STORE_REFUSAL: &str = "the change could not be staged";

pub fn refusal(change: &str, error: &ProposalError) -> ExtractionRefusal {
    let message = match error {
        ProposalError::Draft(_) => STORE_REFUSAL.to_owned(),
        other => other.to_string(),
    };
    ExtractionRefusal {
        change: change.to_owned(),
        code: refusal_code(error).to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scheduler::{DraftError, StoreError};

    #[test]
    fn a_store_failure_is_logged_without_its_backend_text() {
        let error = ProposalError::Draft(DraftError::Store(StoreError::Backend(
            "/private/var/db.sqlite3: disk I/O error".into(),
        )));
        let logged = refusal("move", &error);
        assert_eq!(logged.code, "store");
        assert_eq!(logged.message, STORE_REFUSAL);
    }
}
