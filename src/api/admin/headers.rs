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
    limits::{LimitsDesk, Remembered, actor, mismatch},
    write::{Refusal, origin},
};
use crate::{
    api::{auth::AdminSession, error::ApiError, listeners::Site},
    bot::delivery::{ManualRequest, ManualStart},
};

const DIGEST: &str = "headers.rewrite";

/// A second trigger while a run is queued or running.
const RUNNING: &str =
    "A header rewrite is already running; wait for it to finish (see the Rewrites log).";
/// No rewrite model or persona.
const DISABLED: &str = "Header rewrites aren't set up: no rewrite model or persona.";

type Reply = Result<axum::response::Response, Refusal>;

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/headers/rewrite", post(rewrite))
}

fn accepted(message: String) -> axum::response::Response {
    (StatusCode::ACCEPTED, Json(json!({ "message": message }))).into_response()
}

async fn rewrite(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let port = state.header_rewrite.as_ref().ok_or(ApiError::UNAVAILABLE)?;
    let actor = actor(&session);
    // Held across the trigger, so a concurrent retry with this key replays.
    let mut keys = state.limits.keys.lock().await;
    if let Some(key) = &key
        && let Some(entry) = LimitsDesk::recall(&keys, &actor, key)
    {
        return if entry.digest == DIGEST {
            Ok(accepted(entry.message))
        } else {
            Err(mismatch())
        };
    }
    let message = match port(ManualRequest {
        actor: actor.clone(),
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
    if let Some(key) = key {
        LimitsDesk::remember(
            &mut keys,
            Remembered {
                actor,
                key,
                digest: DIGEST.into(),
                message: message.clone(),
            },
        );
    }
    Ok(accepted(message))
}
