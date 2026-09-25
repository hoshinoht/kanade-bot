//! Rollbacks: `preview: true` plans through the writer and writes nothing
//! (the request id is ignored); otherwise the rollback is applied as the
//! session's admin, strict unless `force`. Every outcome, strict conflicts
//! included, is a `200 RevertPlan`; refusals follow the A4 table.

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::{encoded, parse};
use crate::{
    api::{
        admin::write::{Refusal, bad_body, origin, scheduler, state},
        auth::AdminSession,
        dto::history as dto,
        error::ApiError,
        listeners::Site,
        write::{RollbackRequest, RollbackSelection, WriteContext},
    },
    domain::{
        history::{HistoryRefusal, Origin, RevertMode, RevertOutcome},
        members::Roster,
        scheduler::SchedulerError,
    },
};

type Reply = Result<Response, Refusal>;

/// `RollbackMode` (api-types), common to the three bodies. Spelled out in
/// each: serde's `flatten` cannot be combined with `deny_unknown_fields`.
struct Mode {
    force: bool,
    preview: bool,
    /// The same request id `Idempotency-Key` carries; one of them suffices.
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevertBody {
    seqs: Vec<u64>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBody {
    week: String,
    revision: u64,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorBody {
    actor: String,
    since: String,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    request_id: Option<String>,
}

macro_rules! mode {
    ($body:expr) => {
        Mode {
            force: $body.force,
            preview: $body.preview,
            request_id: $body.request_id,
        }
    };
}

fn key_refused(message: &str) -> Refusal {
    Refusal::new(
        StatusCode::BAD_REQUEST,
        "invalid_idempotency_key",
        message.to_owned(),
    )
}

/// The header's request id, or the body's under the same rules; both must agree.
fn request_origin(
    session: &AdminSession,
    headers: &HeaderMap,
    body: Option<String>,
) -> Result<Origin, Refusal> {
    let mut origin = origin(session, headers)?;
    let Some(id) = body else {
        return Ok(origin);
    };
    let valid = (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if !valid {
        return Err(key_refused(
            "request_id is 1-128 letters, digits, '-', '_', '.' or ':'.",
        ));
    }
    match &origin.request_id {
        Some(header) if *header != id => Err(key_refused(
            "Idempotency-Key and request_id name different requests.",
        )),
        _ => {
            origin.request_id = Some(id);
            Ok(origin)
        }
    }
}

async fn run(
    site: &Site,
    session: &AdminSession,
    headers: &HeaderMap,
    selection: RollbackSelection,
    mode: Mode,
) -> Reply {
    let state = state(site)?;
    let mut origin = request_origin(session, headers, mode.request_id)?;
    if mode.preview {
        origin.request_id = None;
    }
    let selected = match &selection {
        RollbackSelection::Seqs(seqs) => seqs.clone(),
        _ => Vec::new(),
    };
    let by_seqs = matches!(selection, RollbackSelection::Seqs(_));
    let request = RollbackRequest {
        selection,
        mode: if mode.force {
            RevertMode::Force
        } else {
            RevertMode::Strict
        },
        preview: mode.preview,
    };
    let ctx = WriteContext {
        policy: state.policy.clone(),
        directory: Roster::new(),
    };
    let unavailable = |_| Refusal::from(ApiError::UNAVAILABLE);
    let outcome = match state
        .writer
        .rollback(origin, request, state.store.as_ref(), &ctx)
        .await
    {
        Ok(outcome) => outcome,
        // A retried apply answers the rollback it recorded the first time.
        Err(SchedulerError::AlreadyApplied { seq, .. }) => {
            let record = state
                .store
                .change(seq)
                .await
                .map_err(unavailable)?
                .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
            return Ok(Json(encoded(dto::replayed(&record))?).into_response());
        }
        // A week or actor with nothing to undo is a no-op, not an error.
        Err(SchedulerError::History(HistoryRefusal::NothingToRevert)) if !by_seqs => {
            RevertOutcome::Unchanged {
                seqs: Vec::new(),
                skipped: Vec::new(),
            }
        }
        Err(error) => return Err(scheduler(error)),
    };
    let applied = match &outcome {
        RevertOutcome::Reverted { seq: Some(seq), .. } => Some(
            state
                .store
                .change(*seq)
                .await
                .map_err(unavailable)?
                .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?,
        ),
        _ => None,
    };
    Ok(Json(encoded(dto::plan(&outcome, &selected, applied.as_ref()))?).into_response())
}

pub async fn revert(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<RevertBody>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    if body.seqs.is_empty() {
        return Err(Refusal::invalid("Choose at least one change."));
    }
    run(
        &site,
        &session,
        &headers,
        RollbackSelection::Seqs(body.seqs),
        mode!(body),
    )
    .await
}

pub async fn restore_week(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<RestoreBody>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let week = parse::week(&state.policy, &body.week)
        .ok_or_else(|| Refusal::invalid("That is not the start of a boss week."))?;
    run(
        &site,
        &session,
        &headers,
        RollbackSelection::Week {
            week,
            revision: body.revision,
        },
        mode!(body),
    )
    .await
}

pub async fn revert_actor(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<ActorBody>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let actor = parse::actor(&body.actor)
        .ok_or_else(|| Refusal::invalid("Actor is kind:id, e.g. member:1005."))?;
    let since = parse::since(&state.policy, &body.since).ok_or_else(|| {
        Refusal::invalid("Since is a date (YYYY-MM-DD) or a local time (YYYY-MM-DDTHH:MM).")
    })?;
    run(
        &site,
        &session,
        &headers,
        RollbackSelection::Actor { actor, since },
        mode!(body),
    )
    .await
}
