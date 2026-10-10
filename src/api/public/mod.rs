//! Public-origin routes (`member-auth-contract.md` §1). The portal is open
//! only while the admin switch `self_service.public_portal` is on and the
//! public Discord application is configured; then members sign in and see
//! their own session and devices, the boss week, their weekly timings (and
//! move their ownership), their chat allowance, boss guides and boss art;
//! they answer and move their own runs and send and withdraw requests.
//! Closed, the origin serves the shell, status and identity, sign-in answers
//! `closed` and data and art answer `503 closed`. Every session route sits
//! behind [`member::require_session`]; nothing here reads an admin credential.
//! Data reads take a per-member read bucket, art responses share a small
//! concurrency cap, and refused run and request writes are audited.

mod auth;
mod bosses;
mod ownership;
mod read;
mod requests;
mod runs;
mod sessions;
mod write;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post, put},
};
use serde::Serialize;
use tokio::sync::Semaphore;

use super::{
    assets,
    auth::{
        audit::{AuditContext, AuditEvent},
        member::{self, MemberAuth, MemberSession},
        rate::MEMBER_READ_ROUTE,
    },
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
    // Data reads take the member's read bucket.
    let data = Router::new()
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
        .route("/api/public/runs/{id}", get(runs::link).fallback(unmounted))
        .route(
            "/api/public/requests/mine",
            get(requests::mine).fallback(unmounted),
        )
        .route(
            "/api/public/timings",
            get(ownership::timings).fallback(unmounted),
        )
        .route_layer(from_fn_with_state(site.clone(), read_bucket));
    // Refused run and request writes are audited here; the ownership writes
    // can call `write::refused_write` or take this layer too.
    let writes = Router::new()
        .route(
            "/api/public/runs/{id}/answer",
            put(runs::answer).fallback(unmounted),
        )
        .route(
            "/api/public/runs/{id}/move",
            post(runs::move_run).fallback(unmounted),
        )
        .route(
            "/api/public/requests",
            post(requests::submit).fallback(unmounted),
        )
        .route(
            "/api/public/requests/{id}/withdraw",
            post(requests::withdraw).fallback(unmounted),
        )
        .route_layer(from_fn_with_state(site.clone(), write::audit_refusals));
    let art_slots = Arc::new(Semaphore::new(assets::ART_STREAMS));
    // Data, art and the member's writes: `closed` before the session check,
    // so a site without the member realm answers as the catch-alls do; other
    // methods are unmounted.
    let reads = Router::new()
        .merge(data)
        .merge(writes)
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
        .route(
            "/art/{*rest}",
            get(read::art)
                .fallback(unmounted)
                .route_layer(from_fn_with_state(art_slots, assets::capped)),
        )
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

/// One of the member's read tokens per data read (inside the session
/// check); a refusal is `429 rate_limited`, audited as the write bucket's.
async fn read_bucket(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    request: Request,
    next: Next,
) -> Response {
    if let (Some(member), Some(session)) = (
        site.member.as_deref(),
        request.extensions().get::<MemberSession>(),
    ) && !member
        .rate()
        .take_member_read(&session.user_id, member.now())
    {
        member.audit(
            &audit,
            AuditEvent::RateLimited {
                route: MEMBER_READ_ROUTE,
            },
        );
        return ApiError::RATE_LIMITED.into_response();
    }
    next.run(request).await
}

/// Paths not mounted (yet): `closed` while the portal is, else a plain 404.
async fn unmounted(State(site): State<Arc<Site>>) -> ApiError {
    if open(&site).is_some() {
        ApiError::NOT_FOUND
    } else {
        ApiError::CLOSED
    }
}
