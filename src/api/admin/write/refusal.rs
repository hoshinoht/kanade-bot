//! Mutation refusals as `ApiError` bodies. Rule refusals keep the scheduler's
//! v4 wording (written for members); store and backend text never leaves.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{
    api::error::ApiError,
    domain::{
        history::PreconditionError,
        schedule::{MemberRunRefusal, ScheduleError},
        scheduler::{SchedulerError, StoreError},
    },
};

#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: StatusCode,
    pub error: &'static str,
    pub message: String,
}

impl Refusal {
    pub fn new(status: StatusCode, error: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            error,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", message)
    }

    pub fn stale() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "stale",
            "The week changed since it was loaded.",
        )
    }
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Self {
        Self::new(error.status, error.error, error.message)
    }
}

#[derive(Serialize)]
struct Body<'a> {
    error: &'a str,
    message: &'a str,
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(Body {
                error: self.error,
                message: &self.message,
            }),
        )
            .into_response()
    }
}

/// The status/code table for scheduler errors (`AlreadyApplied` is handled
/// by the caller: it answers the current state as the first result).
pub fn scheduler(error: SchedulerError) -> Refusal {
    use StatusCode as S;
    match error {
        SchedulerError::AlreadyApplied { .. } => Refusal::new(
            S::CONFLICT,
            "already_applied",
            "That request was already applied.",
        ),
        SchedulerError::IdempotencyMismatch { .. } => Refusal::new(
            S::UNPROCESSABLE_ENTITY,
            "idempotency_mismatch",
            "That Idempotency-Key was already used for a different request.",
        ),
        SchedulerError::StaleEdit { .. } => Refusal::stale(),
        SchedulerError::Precondition(error) => precondition(&error),
        SchedulerError::Forbidden(reason) => Refusal::new(S::FORBIDDEN, "forbidden", reason),
        SchedulerError::History(refusal) => Refusal::invalid(refusal.to_string()),
        SchedulerError::Schedule(error) => schedule(error),
        SchedulerError::Store(error) => store(&error),
    }
}

fn precondition(error: &PreconditionError) -> Refusal {
    let code = match error {
        PreconditionError::UnknownField { .. } => "unknown_field",
        PreconditionError::DuplicateField { .. } => "duplicate_field",
        PreconditionError::OverrideNotSeen { .. } => "override_not_seen",
        PreconditionError::OverrideUnchanged { .. } => "override_unchanged",
        PreconditionError::UnknownOverride { .. } => "unknown_override",
        PreconditionError::UnknownTarget { .. } => {
            return Refusal::new(StatusCode::NOT_FOUND, "unknown_target", error.to_string());
        }
        PreconditionError::OverrideNotAdmin => {
            return Refusal::new(
                StatusCode::FORBIDDEN,
                "override_forbidden",
                error.to_string(),
            );
        }
    };
    Refusal::new(StatusCode::UNPROCESSABLE_ENTITY, code, error.to_string())
}

fn schedule(error: ScheduleError) -> Refusal {
    use StatusCode as S;
    let message = error.to_string();
    match error {
        ScheduleError::UnknownRun(_) => {
            Refusal::new(S::NOT_FOUND, "not_found", "That run no longer exists.")
        }
        ScheduleError::UnknownFixedRun(_) => Refusal::new(
            S::NOT_FOUND,
            "not_found",
            "That weekly timing no longer exists.",
        ),
        ScheduleError::RunMoveConflict | ScheduleError::MoveConflict { .. } => {
            Refusal::new(S::CONFLICT, "move_conflict", message)
        }
        ScheduleError::MissingChoice(_) => {
            Refusal::new(S::UNPROCESSABLE_ENTITY, "choices_required", message)
        }
        ScheduleError::UnexpectedChoice(_) => {
            Refusal::new(S::UNPROCESSABLE_ENTITY, "choices_not_applicable", message)
        }
        ScheduleError::NothingToChange => {
            Refusal::new(S::UNPROCESSABLE_ENTITY, "nothing_to_change", message)
        }
        ScheduleError::NotOnRun(_) => Refusal::new(S::UNPROCESSABLE_ENTITY, "not_on_run", message),
        // A live run past its end is frozen until it is settled.
        ScheduleError::RunEnded { .. } => Refusal::new(S::CONFLICT, "run_ended", message),
        // Only the public origin's own-run writes are refused this way.
        ScheduleError::MemberRun(refusal) => {
            let (status, code) = match refusal {
                MemberRunRefusal::NotInRun => (S::FORBIDDEN, "not_in_run"),
                MemberRunRefusal::Closed => (S::CONFLICT, "run_closed"),
                MemberRunRefusal::Ended => (S::CONFLICT, "run_ended"),
                MemberRunRefusal::WeekOver => (S::CONFLICT, "week_over"),
                MemberRunRefusal::Started => (S::CONFLICT, "run_started"),
                MemberRunRefusal::InThePast => (S::UNPROCESSABLE_ENTITY, "in_the_past"),
            };
            Refusal::new(status, code, message)
        }
        // Configuration, not the request: never the client's to fix.
        ScheduleError::AttendanceMismatch { .. } => ApiError::UNAVAILABLE.into(),
        _ => Refusal::invalid(message),
    }
}

fn store(error: &StoreError) -> Refusal {
    match error {
        // Revision races outlasted the retries: the same edit may simply be retried.
        StoreError::Conflict { .. } => Refusal::new(
            StatusCode::CONFLICT,
            "busy",
            "Another change landed at the same moment; try again.",
        ),
        StoreError::StaleEdit(_) => Refusal::stale(),
        StoreError::Precondition(error) => precondition(error),
        StoreError::IdempotencyMismatch { .. } => {
            scheduler(SchedulerError::IdempotencyMismatch { seq: 0 })
        }
        _ => ApiError::UNAVAILABLE.into(),
    }
}

#[cfg(test)]
mod atomic_fixed_patch_proof;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::history::BlameTarget;

    #[test]
    fn every_scheduler_error_has_a_fixed_status_and_code() {
        let run = || BlameTarget::Run("r".into());
        let table: Vec<(SchedulerError, u16, &str)> = vec![
            (
                SchedulerError::IdempotencyMismatch { seq: 3 },
                422,
                "idempotency_mismatch",
            ),
            (
                SchedulerError::StaleEdit {
                    conflicts: Vec::new(),
                },
                409,
                "stale",
            ),
            (SchedulerError::Forbidden("no".into()), 403, "forbidden"),
            (
                SchedulerError::Precondition(PreconditionError::OverrideNotAdmin),
                403,
                "override_forbidden",
            ),
            (
                SchedulerError::Precondition(PreconditionError::UnknownField {
                    target: run(),
                    field: "x".into(),
                }),
                422,
                "unknown_field",
            ),
            (
                SchedulerError::Precondition(PreconditionError::UnknownTarget { target: run() }),
                404,
                "unknown_target",
            ),
            (
                ScheduleError::UnknownRun("r".into()).into(),
                404,
                "not_found",
            ),
            (
                ScheduleError::UnknownFixedRun("f".into()).into(),
                404,
                "not_found",
            ),
            (ScheduleError::RunMoveConflict.into(), 409, "move_conflict"),
            (
                ScheduleError::MissingChoice("r".into()).into(),
                422,
                "choices_required",
            ),
            (
                ScheduleError::UnexpectedChoice("r".into()).into(),
                422,
                "choices_not_applicable",
            ),
            (
                ScheduleError::NothingToChange.into(),
                422,
                "nothing_to_change",
            ),
            (
                ScheduleError::NotOnRun(vec!["1".into()]).into(),
                422,
                "not_on_run",
            ),
            (
                ScheduleError::RunEnded { run_id: "r".into() }.into(),
                409,
                "run_ended",
            ),
            (
                ScheduleError::MemberRun(MemberRunRefusal::Ended).into(),
                409,
                "run_ended",
            ),
            (ScheduleError::NoParticipants.into(), 422, "invalid"),
            (
                ScheduleError::NotSettable("at_risk".into()).into(),
                422,
                "invalid",
            ),
            (
                StoreError::Conflict {
                    expected: 1,
                    found: 2,
                }
                .into(),
                409,
                "busy",
            ),
            (
                StoreError::Backend("disk /secret/path".into()).into(),
                503,
                "unavailable",
            ),
            (
                StoreError::Constraint("UNIQUE members".into()).into(),
                503,
                "unavailable",
            ),
        ];
        for (error, status, code) in table {
            let refusal = scheduler(error);
            assert_eq!((refusal.status.as_u16(), refusal.error), (status, code));
            assert!(!refusal.message.contains("/secret/path"));
        }
    }
}
