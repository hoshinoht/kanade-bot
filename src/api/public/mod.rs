//! Public-origin routes (`member-auth-contract.md` §1). The portal is open
//! only while the admin switch `self_service.public_portal` is on and the
//! public Discord application is configured; then members sign in and see
//! their own session and devices, the boss week, their weekly timings (and
//! move their ownership), their chat allowance, boss guides and boss art. Closed, the
//! origin serves the shell, status and identity, sign-in answers `closed`
//! and data and art answer `503 closed`. Every session route sits behind
//! [`member::require_session`]; nothing here reads an admin credential.

mod auth;
mod bosses;
mod ownership;
mod read;
mod sessions;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
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
        .route_layer(from_fn_with_state(site.clone(), member::require_session));
    // Data, art and the member's writes: `closed` before the session check,
    // so a site without the member realm answers as the catch-alls do; other
    // methods are unmounted.
    let reads = Router::new()
        .route("/api/public/week", get(read::week).fallback(unmounted))
        .route(
            "/api/public/me/allowance",
            get(read::allowance).fallback(unmounted),
        )
        .route("/api/public/bosses", get(bosses::list).fallback(unmounted))
        .route(
            "/api/public/bosses/events",
            get(bosses::events).fallback(unmounted),
        )
        .route(
            "/api/public/bosses/{key}/knowledge",
            get(bosses::knowledge).fallback(unmounted),
        )
        .route(
            "/api/public/timings",
            get(ownership::timings).fallback(unmounted),
        )
        .route(
            "/api/public/timings/{id}/owner",
            post(ownership::hand_off).fallback(unmounted),
        )
        .route(
            "/api/public/timings/{id}/owner-requests",
            post(ownership::ask).fallback(unmounted),
        )
        .route(
            "/api/public/owner-requests/{id}/accept",
            post(ownership::accept).fallback(unmounted),
        )
        .route(
            "/api/public/owner-requests/{id}/decline",
            post(ownership::decline).fallback(unmounted),
        )
        .route(
            "/api/public/owner-requests/{id}/withdraw",
            post(ownership::withdraw).fallback(unmounted),
        )
        .route("/art/{*rest}", get(read::art).fallback(unmounted))
        .route_layer(from_fn_with_state(site.clone(), member::require_session))
        .route_layer(from_fn_with_state(site, closed));
    Router::new()
        .route("/api/public/status", get(status))
        .route("/api/public/auth/discord/start", get(auth::start))
        .route("/api/public/auth/discord/callback", get(auth::callback))
        .route("/api/public/auth/logout", post(auth::logout))
        .merge(signed_in)
        .merge(reads)
        .route("/api/public/{*rest}", any(unmounted))
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

/// `503 closed` unless the portal is open.
async fn closed(State(site): State<Arc<Site>>, request: Request, next: Next) -> Response {
    if open(&site).is_some() {
        next.run(request).await
    } else {
        ApiError::CLOSED.into_response()
    }
}

/// Paths not mounted (yet): `closed` while the portal is, else a plain 404.
async fn unmounted(State(site): State<Arc<Site>>) -> ApiError {
    if open(&site).is_some() {
        ApiError::NOT_FOUND
    } else {
        ApiError::CLOSED
    }
}
