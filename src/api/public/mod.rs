//! Public-origin routes (`member-auth-contract.md` §1). The portal is open
//! only while the admin switch `self_service.public_portal` is on and the
//! public Discord application is configured; then members sign in and see
//! their own session and devices. Closed, the origin serves the shell,
//! status and identity, sign-in answers `closed` and data and art answer
//! `503 closed`. Every session route sits behind
//! [`member::require_session`]; nothing here reads an admin credential.

mod auth;
mod sessions;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    middleware::from_fn_with_state,
    routing::{any, delete, get, post},
};
use serde::Serialize;

use super::{
    auth::member::{self, MemberAuth},
    error::ApiError,
    listeners::Site,
};

pub fn routes(site: Arc<Site>) -> Router<Arc<Site>> {
    let signed_in = Router::new()
        .route("/api/public/session", get(sessions::session))
        .route("/api/public/session/avatar", get(sessions::avatar_image))
        .route("/api/public/sessions", get(sessions::list))
        .route("/api/public/sessions/{handle}", delete(sessions::end_one))
        .route("/api/public/sessions/end-all", post(sessions::end_all))
        .route_layer(from_fn_with_state(site, member::require_session));
    Router::new()
        .route("/api/public/status", get(status))
        .route("/api/public/auth/discord/start", get(auth::start))
        .route("/api/public/auth/discord/callback", get(auth::callback))
        .route("/api/public/auth/logout", post(auth::logout))
        .merge(signed_in)
        .route("/api/public/{*rest}", any(unmounted))
        .route("/art/{*rest}", any(unmounted))
}

fn open(site: &Site) -> Option<&MemberAuth> {
    site.member.as_deref().filter(|member| member.is_open())
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct PublicStatus {
    #[cfg_attr(test, ts(type = "'open' | 'closed'"))]
    portal: &'static str,
}

async fn status(State(site): State<Arc<Site>>) -> Json<PublicStatus> {
    Json(PublicStatus {
        portal: if open(&site).is_some() {
            "open"
        } else {
            "closed"
        },
    })
}

/// Data and art routes not mounted yet (`member-reads`): `closed` while the
/// portal is, else a plain 404.
async fn unmounted(State(site): State<Arc<Site>>) -> ApiError {
    if open(&site).is_some() {
        ApiError::NOT_FOUND
    } else {
        ApiError::CLOSED
    }
}
