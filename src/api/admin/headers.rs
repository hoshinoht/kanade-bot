//! `POST /api/admin/headers/rewrite`: the admin portal's manual header
//! rewrite. It only asks the delivery side to queue the run (every header
//! posted this boss week, edited in place) and answers `202` at once; the
//! work and its Rewrites-log rows follow in the background. Idempotent per
//! `Idempotency-Key` like the manual digest.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use serde_json::json;

use super::{
    context::state,
    limits::{actor, record_after_effect},
    replay::{Keyed, replayed},
    write::{Refusal, origin},
};
use crate::{
    api::{auth::AdminSession, error::ApiError, listeners::Site},
    bot::delivery::{ManualRequest, ManualStart},
    infrastructure::store::replays::ReplayScope,
};

/// The whole request: the route takes no body.
const REQUEST: &str = "headers.rewrite";

/// A second trigger while a run is queued or running.
const RUNNING: &str =
    "A header rewrite is already running; wait for it to finish (see the Rewrites log).";
/// No rewrite model or persona.
const DISABLED: &str = "Header rewrites aren't set up: no rewrite model or persona.";

type Reply = Result<axum::response::Response, Refusal>;

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/headers/rewrite", post(rewrite))
}

fn accepted(message: String) -> serde_json::Value {
    json!({ "message": message })
}

async fn rewrite(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let port = state.header_rewrite.as_ref().ok_or(ApiError::UNAVAILABLE)?;
    let now = state.now();
    // Held across the trigger, so a concurrent retry with this key replays.
    let _held = state.limits.lock.lock().await;
    let keyed = key.map(|key| Keyed::new(ReplayScope::Limits, &session, key, REQUEST));
    if let Some(keyed) = &keyed
        && let Some(found) = keyed.recall(state.store.as_ref(), now).await?
    {
        return replayed(&found);
    }
    let message = match port(ManualRequest {
        actor: actor(&session),
        report_to: None,
    })
    .await
    {
        ManualStart::Started(count) => {
            format!("Rewriting {count} header(s); see the Rewrites log.")
        }
        ManualStart::Nothing => {
            return Ok(Json(json!({
                "message": "Nothing posted this boss week has a header to rewrite."
            }))
            .into_response());
        }
        ManualStart::Running => {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "rewrite_running",
                RUNNING,
            ));
        }
        ManualStart::Disabled => {
            return Err(Refusal::new(StatusCode::CONFLICT, "rewrite_off", DISABLED));
        }
        ManualStart::Unavailable => return Err(ApiError::UNAVAILABLE.into()),
    };
    let body = accepted(message);
    if let Some(keyed) = keyed {
        // The run is queued on the delivery side, outside the store.
        record_after_effect(state, keyed.answered(StatusCode::ACCEPTED, &body, now)).await;
    }
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}
