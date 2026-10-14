//! Draft refusals as `ApiError` bodies. Rule refusals keep the domain's
//! words; store and backend text never leaves.

use axum::http::StatusCode as S;

use super::super::write::{Refusal, scheduler};
use crate::{
    api::error::ApiError,
    domain::scheduler::{DraftError, SchedulerError},
};

pub fn not_found() -> Refusal {
    Refusal::not_found("not_found", "You have no draft with that id.")
}

pub fn version_required() -> Refusal {
    Refusal::unprocessable(
        "version_required",
        "Name the draft version you edited (`version`).",
    )
}

fn stale() -> Refusal {
    Refusal::conflict(
        "stale",
        "The draft changed or was closed since you loaded it; reload it.",
    )
}

/// What a draft write was doing: staging refusals are the request's to fix
/// (422); a rebase or merge that no longer applies is the schedule's (409).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Staging,
    Landing,
}

pub fn draft(error: DraftError, doing: Doing) -> Refusal {
    let message = error.to_string();
    match error {
        DraftError::Schedule(error) => scheduler(SchedulerError::Schedule(error)),
        DraftError::Store(error) => scheduler(SchedulerError::Store(error)),
        DraftError::UnknownDraft(_) | DraftError::RequestDraft => not_found(),
        DraftError::Stale { .. } | DraftError::AlreadyMerged { .. } => stale(),
        // Callers answer an exact merge retry as the first result.
        DraftError::AlreadyApplied { .. } => stale(),
        DraftError::IdempotencyMismatch { .. } | DraftError::RequestMismatch { .. } => {
            Refusal::unprocessable(
                "idempotency_mismatch",
                "That Idempotency-Key was already used for a different request.",
            )
        }
        DraftError::Conflicts(conflicts) => Refusal::conflict(
            "conflicts",
            format!(
                "The draft has {} unsettled conflict(s) with the schedule; open its preview.",
                conflicts.len()
            ),
        ),
        DraftError::BaseMoved { .. } => Refusal::conflict("base_moved", message),
        DraftError::ReplayFailed { .. } if doing == Doing::Staging => {
            Refusal::unprocessable("op_refused", message)
        }
        DraftError::ReplayFailed { .. } => Refusal::conflict("conflicts", message),
        DraftError::EditRefused(_) => Refusal::unprocessable("op_needed", message),
        DraftError::ProposalOnlyOp(_) => Refusal::invalid(message),
        DraftError::Empty => Refusal::unprocessable("empty", "Stage a change before merging."),
        DraftError::NoEffect => Refusal::conflict(
            "no_effect",
            "The schedule is already like that; nothing would change.",
        ),
        DraftError::InvalidTitle => {
            Refusal::unprocessable("invalid_title", "A draft title is 1 to 200 characters.")
        }
        DraftError::Expired => Refusal::new(
            S::GONE,
            "expired",
            "The draft's boss week has passed; it is closed and nothing was applied.",
        ),
        DraftError::RequesterUnauthorised | DraftError::HistoryGap(_) => {
            ApiError::UNAVAILABLE.into()
        }
    }
}
