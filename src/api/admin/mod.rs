//! Admin-origin routes. Every admin API handler takes
//! [`crate::api::auth::AdminSession`]; unmounted `/api/admin/*` paths are the
//! generic 404.

mod account;
mod auth;
mod avatars;
pub mod config;
pub(crate) mod context;
mod etag;
mod events;
mod headers;
mod history;
mod inbox;
pub mod limits;
mod logs;
pub(crate) mod read;
mod reminders;
mod replay;
mod tonight;
pub(crate) mod write;

// Private response types the TypeScript bindings test names.
#[cfg(test)]
pub(crate) use {
    auth::{Methods, SessionView},
    write::{Message, MoveResult, Previous, RunResult, SwapResult, ValidateResult},
};

use std::sync::Arc;

use axum::{
    Extension, Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get,
};

use super::{assets, error::ApiError, guard::proxy::Peer, listeners::Site};
use crate::runtime::application::OfflineApplication;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/healthz", get(health))
        .route("/art/{kind}/{key}", get(assets::art))
        .merge(auth::routes())
        .merge(account::routes())
        .merge(avatars::routes())
        .merge(tonight::routes())
        .merge(read::routes())
        .merge(reminders::routes())
        .merge(write::routes())
        .merge(history::routes())
        .merge(inbox::routes())
        .merge(logs::routes())
        .merge(limits::routes())
        .merge(headers::routes())
        .merge(config::routes())
        .merge(events::routes())
        .route_layer(axum::middleware::from_fn(etag::revalidate))
}

/// Answers only clients on this host (the local healthcheck), whatever the
/// trusted-proxy setting, and never requests the authenticated edge relays.
async fn health(
    State(site): State<Arc<Site>>,
    Extension(peer): Extension<Peer>,
) -> axum::response::Response {
    if !peer.is_local(site.listener_ip) {
        return ApiError::NOT_FOUND.into_response();
    }
    let health = match &site.health {
        Some(probe) => probe.health().await,
        None => OfflineApplication.health(),
    };
    let status = if health.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(health)).into_response()
}
