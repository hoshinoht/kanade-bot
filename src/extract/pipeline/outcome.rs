//! The extraction log outcome of one call (`docs/notes/admin-api.md` filters).

use super::call::Failure;
use crate::domain::model_log::ExtractionOutcome;

/// A failed call is turned away / content-blocked / failed; an answered one
/// is proposed when any proposal was created, a self-service link when the
/// redirect took every change instead, else no change (nothing kept, all
/// refused up front, or only chat answers).
pub fn extraction_outcome(
    failure: Option<Failure>,
    proposals: usize,
    redirected: usize,
) -> ExtractionOutcome {
    match failure {
        Some(Failure::TurnedAway { .. }) => ExtractionOutcome::TurnedAway,
        Some(Failure::ContentBlocked) => ExtractionOutcome::ContentBlocked,
        Some(Failure::Failed) => ExtractionOutcome::Failed,
        None if proposals > 0 => ExtractionOutcome::Proposed,
        None if redirected > 0 => ExtractionOutcome::SelfServiceLink,
        None => ExtractionOutcome::NoChange,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_follow_the_failure_then_what_was_created() {
        let away = Failure::TurnedAway { retry_at: None };
        assert_eq!(
            extraction_outcome(Some(away), 1, 0),
            ExtractionOutcome::TurnedAway
        );
        assert_eq!(
            extraction_outcome(Some(Failure::ContentBlocked), 0, 0),
            ExtractionOutcome::ContentBlocked
        );
        assert_eq!(
            extraction_outcome(Some(Failure::Failed), 0, 0),
            ExtractionOutcome::Failed
        );
        assert_eq!(extraction_outcome(None, 1, 1), ExtractionOutcome::Proposed);
        assert_eq!(
            extraction_outcome(None, 0, 1),
            ExtractionOutcome::SelfServiceLink
        );
        assert_eq!(extraction_outcome(None, 0, 0), ExtractionOutcome::NoChange);
    }
}
