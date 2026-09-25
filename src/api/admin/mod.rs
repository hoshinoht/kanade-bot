//! Admin-origin routes. Every admin API handler takes
//! [`crate::api::auth::AdminSession`]; unmounted `/api/admin/*` paths are the
//! generic 404.

mod auth;

use std::sync::Arc;

use axum::{Extension, Json, Router, response::IntoResponse, routing::get};

use super::{assets, error::ApiError, guard::proxy::Peer, listeners::Site};
use crate::runtime::application::OfflineApplication;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/healthz", get(health))
        .route("/art/{kind}/{key}", get(assets::art))
        .merge(auth::routes())
}

/// Answers only direct loopback clients, never requests relayed by the edge.
async fn health(Extension(peer): Extension<Peer>) -> axum::response::Response {
    if !peer.is_direct_loopback() {
        return ApiError::NOT_FOUND.into_response();
    }
    Json(OfflineApplication.health()).into_response()
}
