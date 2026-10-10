//! Request and proposal refusals as `ApiError` bodies ("Inbox (A6)"
//! refusal table). Store and backend text never leaves.

use axum::http::StatusCode as S;

use super::super::write::{Refusal, scheduler};
use crate::{
    api::{error::ApiError, ownership::OwnershipError},
    domain::{
        drafts::{MergeConflict, ReplayError},
        requests::RequestRefusal,
        schedule::ScheduleError,
        scheduler::{DraftError, ProposalError, RequestError, SchedulerError},
    },
};

pub fn not_found() -> Refusal {
    Refusal::new(
        S::NOT_FOUND,
        "not_found",
        "Nothing in the inbox has that id.",
    )
}

pub fn stale() -> Refusal {
    Refusal::new(
        S::CONFLICT,
        "stale",
        "It changed or was decided since you opened it; review it again.",
    )
}

pub fn conflicts(message: impl Into<String>) -> Refusal {
    Refusal::new(S::CONFLICT, "conflicts", message)
}

pub fn expired() -> Refusal {
    Refusal::new(
        S::GONE,
        "expired",
        "It expired and is now closed; nothing was applied.",
    )
}

pub fn no_effect() -> Refusal {
    Refusal::new(
        S::CONFLICT,
        "no_effect",
        "It is already like that; nothing would change. Reject it instead.",
    )
}

pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal::new(S::UNPROCESSABLE_ENTITY, code, message)
}

pub fn discord_session_required() -> Refusal {
    Refusal::new(
        S::FORBIDDEN,
        "discord_session_required",
        "Sign in with Discord to approve or reject Kanade's proposals.",
    )
}

/// A per-run choice problem, when that is why an operation would not apply.
fn choice(error: &ReplayError) -> Option<Refusal> {
    match error {
        ReplayError::Schedule(error @ ScheduleError::MissingChoice(_)) => {
            Some(unprocessable("choices_required", error.to_string()))
        }
        ReplayError::Schedule(error @ ScheduleError::UnexpectedChoice(_)) => {
            Some(unprocessable("choices_not_applicable", error.to_string()))
        }
        _ => None,
    }
}

pub fn draft(error: DraftError) -> Refusal {
    match error {
        DraftError::Schedule(error) => scheduler(SchedulerError::Schedule(error)),
        DraftError::Store(error) => scheduler(SchedulerError::Store(error)),
        DraftError::UnknownDraft(_) => not_found(),
        DraftError::Stale { .. } | DraftError::AlreadyMerged { .. } => stale(),
        // Callers answer a replay with the first result before this.
        DraftError::AlreadyApplied { .. } => stale(),
        DraftError::IdempotencyMismatch { .. } => unprocessable(
            "idempotency_mismatch",
            "That was already decided with different details.",
        ),
        DraftError::Conflicts(conflicts) => conflicts
            .iter()
            .find_map(|conflict| match conflict {
                MergeConflict::OpRejected { error, .. } => choice(error),
                _ => None,
            })
            .unwrap_or_else(|| {
                self::conflicts("The schedule changed since it was proposed; review it again.")
            }),
        DraftError::ReplayFailed { error, .. } => {
            choice(&error).unwrap_or_else(|| conflicts(error.to_string()))
        }
        DraftError::Expired => expired(),
        DraftError::NoEffect => no_effect(),
        DraftError::RequesterUnauthorised => requester_unauthorised(),
        DraftError::HistoryGap(_)
        | DraftError::EditRefused(_)
        | DraftError::Empty
        | DraftError::InvalidTitle
        | DraftError::RequestMismatch { .. }
        | DraftError::RequestDraft
        | DraftError::ProposalOnlyOp(_) => ApiError::UNAVAILABLE.into(),
    }
}

fn requester_unauthorised() -> Refusal {
    Refusal::new(
        S::CONFLICT,
        "requester_unauthorised",
        "The member may no longer have this approved; reject it.",
    )
}

pub fn request(error: RequestError) -> Refusal {
    match error {
        RequestError::Refused(refusal) => match refusal {
            RequestRefusal::NotAdmin => {
                Refusal::new(S::FORBIDDEN, "forbidden", refusal.to_string())
            }
            RequestRefusal::RequesterUnauthorised => requester_unauthorised(),
            RequestRefusal::UnknownSubject => conflicts("That run or weekly run no longer exists."),
            RequestRefusal::ChoicesRequired => {
                unprocessable("choices_required", refusal.to_string())
            }
            RequestRefusal::ChoicesNotApplicable => {
                unprocessable("choices_not_applicable", refusal.to_string())
            }
            RequestRefusal::ReasonRequired => unprocessable("reason_required", refusal.to_string()),
            RequestRefusal::ReasonInvalid => unprocessable("reason_invalid", refusal.to_string()),
            RequestRefusal::Frozen
            | RequestRefusal::FieldNotAllowed
            | RequestRefusal::AlreadyInParty => Refusal::invalid(refusal.to_string()),
        },
        RequestError::Draft(error) => draft(error),
        RequestError::Limited(_) => {
            Refusal::new(S::TOO_MANY_REQUESTS, "request_limit", "Too many requests.")
        }
        RequestError::AdminDraft => not_found(),
        RequestError::Expired(_) => expired(),
        RequestError::NoEffect => no_effect(),
    }
}

pub fn proposal(error: ProposalError) -> Refusal {
    match error {
        // v4's words, written for members.
        ProposalError::Refused(refusal) => conflicts(refusal.to_string()),
        ProposalError::NoEffect => no_effect(),
        ProposalError::Unauthorised => Refusal::new(
            S::FORBIDDEN,
            "forbidden",
            "That proposal is not yours to answer.",
        ),
        ProposalError::NotAProposal => not_found(),
        ProposalError::Expired => expired(),
        ProposalError::EditNotApplicable => edit_not_applicable(),
        ProposalError::EditInPast => Refusal::invalid(error.to_string()),
        ProposalError::Draft(error) => draft(error),
    }
}

/// An ownership decision's refusal: the member-facing rule text as a 409.
pub fn ownership(error: OwnershipError) -> Refusal {
    match error {
        OwnershipError::UnknownRequest => not_found(),
        OwnershipError::UnknownTiming => conflicts("That weekly timing no longer exists."),
        refused @ (OwnershipError::Refused(_) | OwnershipError::AlreadyAsked) => {
            conflicts(refused.to_string())
        }
        OwnershipError::Scheduler(error) => scheduler(error),
        OwnershipError::Store(error) => scheduler(SchedulerError::Store(error)),
    }
}

pub fn edit_not_applicable() -> Refusal {
    unprocessable(
        "edit_not_applicable",
        "Only a change with a time (a move or a new run) can be edited before approving.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{drafts::DraftStatus, proposals::Refusal as V4, scheduler::StoreError};

    #[test]
    fn every_inbox_error_has_a_fixed_status_and_code() {
        let table: Vec<(Refusal, u16, &str)> = vec![
            (request(RequestRefusal::NotAdmin.into()), 403, "forbidden"),
            (
                request(RequestRefusal::RequesterUnauthorised.into()),
                409,
                "requester_unauthorised",
            ),
            (
                request(RequestRefusal::UnknownSubject.into()),
                409,
                "conflicts",
            ),
            (
                request(RequestRefusal::ChoicesRequired.into()),
                422,
                "choices_required",
            ),
            (
                request(RequestRefusal::ChoicesNotApplicable.into()),
                422,
                "choices_not_applicable",
            ),
            (
                request(RequestRefusal::ReasonRequired.into()),
                422,
                "reason_required",
            ),
            (
                request(RequestRefusal::ReasonInvalid.into()),
                422,
                "reason_invalid",
            ),
            (request(RequestError::NoEffect), 409, "no_effect"),
            (request(RequestError::AdminDraft), 404, "not_found"),
            (
                request(
                    DraftError::Stale {
                        status: DraftStatus::Merged,
                        version: 1,
                        merged_seq: None,
                    }
                    .into(),
                ),
                409,
                "stale",
            ),
            (
                request(DraftError::IdempotencyMismatch { seq: 2 }.into()),
                422,
                "idempotency_mismatch",
            ),
            (
                request(DraftError::Conflicts(Vec::new()).into()),
                409,
                "conflicts",
            ),
            (
                request(
                    DraftError::ReplayFailed {
                        ord: 0,
                        error: ReplayError::Schedule(ScheduleError::MissingChoice("r".into())),
                    }
                    .into(),
                ),
                422,
                "choices_required",
            ),
            (
                request(DraftError::Store(StoreError::Backend("/secret".into())).into()),
                503,
                "unavailable",
            ),
            (
                proposal(ProposalError::Refused(V4::RunGone)),
                409,
                "conflicts",
            ),
            (proposal(ProposalError::NoEffect), 409, "no_effect"),
            (proposal(ProposalError::Unauthorised), 403, "forbidden"),
            (proposal(ProposalError::NotAProposal), 404, "not_found"),
            (proposal(ProposalError::Expired), 410, "expired"),
            (
                proposal(ProposalError::EditNotApplicable),
                422,
                "edit_not_applicable",
            ),
            (proposal(ProposalError::EditInPast), 422, "invalid"),
            (
                proposal(ProposalError::Draft(DraftError::AlreadyMerged { seq: 3 })),
                409,
                "stale",
            ),
            (
                proposal(ProposalError::Draft(DraftError::UnknownDraft("x".into()))),
                404,
                "not_found",
            ),
        ];
        for (refusal, status, code) in table {
            assert_eq!((refusal.status.as_u16(), refusal.error), (status, code));
            assert!(!refusal.message.contains("/secret"));
        }
    }
}
