//! Module `guides`' controls (both origins), over the live generation's
//! fake Discord:
//!
//! - `POST /__e2e/discord/guide` `{script}`: what Discord does next.
//!   `ambiguous_create` (the next post times out unposted, so the slot is
//!   left unconfirmed), `edit_404` (the next edit finds its message deleted
//!   by hand), `offline` / `online` (the gateway disconnects or comes back),
//!   `hold` (the next post waits) and `release` (it goes on). 204; 422 for
//!   another script; 404 while the module is off.
//! - `GET /__e2e/discord/calls`: every call the fake Discord answered, in
//!   order (`op`, `channel`, `message`, `outcome`, and for a post `silent`,
//!   `embeds`, `files`).

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;

use crate::controls::Shared;

pub fn routes<S: Clone + Send + Sync + 'static>(shared: &Arc<Shared>) -> Router<S> {
    Router::new()
        .route("/__e2e/discord/guide", post(script))
        .route("/__e2e/discord/calls", get(calls))
        .with_state(Arc::clone(shared))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    script: String,
}

async fn script(
    State(shared): State<Arc<Shared>>,
    body: Result<Json<Script>, JsonRejection>,
) -> Response {
    let Ok(Json(Script { script })) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(live) = shared.live().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match live.guides() {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(side) if side.script(&script) => StatusCode::NO_CONTENT.into_response(),
        Some(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("no script {script:?}"),
        )
            .into_response(),
    }
}

async fn calls(State(shared): State<Arc<Shared>>) -> Response {
    let Some(live) = shared.live().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match live.guides() {
        Some(side) => Json(side.calls()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
