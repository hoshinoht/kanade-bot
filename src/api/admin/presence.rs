//! `/api/admin/presence`: which other admins have the same Inbox request or
//! run sheet open (`PUT` checks in and answers them, `GET` only reads, and
//! `DELETE` leaves). Every call authenticates quietly: like the events
//! stream it never extends the session's idle window, so a forgotten tab
//! still signs out. The writes need CSRF like any admin write but take no
//! `Idempotency-Key` (a check-in is idempotent and records nothing). The
//! board is in memory only ([`crate::api::presence`]); arrivals, leaves and
//! mode changes hint `presence` on the events stream.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, State, rejection::JsonRejection},
    http::{StatusCode, Uri, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};

use super::context::state;
use crate::api::{
    auth::{AdminSession, crypto, wire},
    dto::{
        events::Topic,
        presence::{CheckIn, PresenceHere, PresenceList},
    },
    error::ApiError,
    listeners::Site,
    presence::{CHECK_IN, LIFETIME, Place, Visitor},
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/presence", get(read).put(check_in).delete(leave))
}

/// [`AdminSession`] (CSRF included) without counting as activity.
struct Quiet(AdminSession);

impl FromRequestParts<Arc<Site>> for Quiet {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        let auth = site.auth.as_ref().ok_or(ApiError::AUTH_UNAVAILABLE)?;
        auth.authenticate_quietly(parts).await.map(Self)
    }
}

/// The session's tab key: the cookie is a credential, so only its hash is
/// held (as the store keeps it); a bearer (CLI) call is its actor.
fn tab(session: &AdminSession) -> String {
    session.session_id().map_or_else(
        || session.actor.id().to_owned(),
        |id| crypto::sha256_hex(id.as_bytes()),
    )
}

/// `?at=<kind>:<id>`, plus `&seq=<n>` exactly when `seq` is wanted (the
/// leave); anything else is refused.
fn query(uri: &Uri, seq: bool) -> Result<(Place, u64), ApiError> {
    let pairs = wire::query_pairs(uri.query());
    let sent = uri.query().map_or(0, |query| {
        query.split('&').filter(|pair| !pair.is_empty()).count()
    });
    if pairs.len() != sent || pairs.len() != 1 + usize::from(seq) {
        return Err(ApiError::INVALID_QUERY);
    }
    let place = wire::query_value(&pairs, "at")
        .and_then(|at| Place::parse(&at))
        .ok_or(ApiError::INVALID_QUERY)?;
    if !seq {
        return Ok((place, 0));
    }
    let seq = wire::query_value(&pairs, "seq")
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse().ok())
        .ok_or(ApiError::INVALID_QUERY)?;
    Ok((place, seq))
}

fn visitor<'a>(session: &'a AdminSession, tab: &'a str) -> Visitor<'a> {
    Visitor {
        tab,
        admin: session.actor.id(),
        name: &session.display,
    }
}

async fn check_in(
    State(site): State<Arc<Site>>,
    Quiet(session): Quiet,
    body: Result<Json<CheckIn>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::INVALID_BODY)?;
    let state = state(&site)?;
    let place = Place::new(body.at.kind, &body.at.id).ok_or(ApiError::INVALID_BODY)?;
    let tab = tab(&session);
    let (here, changed) =
        state
            .presence
            .check_in(place, visitor(&session, &tab), body.mode, body.seq);
    if changed {
        state.events.notify(Topic::Presence);
    }
    Ok(Json(PresenceHere {
        heartbeat_s: CHECK_IN.as_secs(),
        ttl_s: LIFETIME.as_secs(),
        here,
    })
    .into_response())
}

async fn read(
    State(site): State<Arc<Site>>,
    Quiet(session): Quiet,
    uri: Uri,
) -> Result<Response, ApiError> {
    let (place, _) = query(&uri, false)?;
    let state = state(&site)?;
    let here = state.presence.here(&place, session.actor.id());
    Ok(Json(PresenceList { here }).into_response())
}

async fn leave(
    State(site): State<Arc<Site>>,
    Quiet(session): Quiet,
    uri: Uri,
) -> Result<StatusCode, ApiError> {
    let (place, seq) = query(&uri, true)?;
    let state = state(&site)?;
    let tab = tab(&session);
    if state.presence.leave(place, visitor(&session, &tab), seq) {
        state.events.notify(Topic::Presence);
    }
    Ok(StatusCode::NO_CONTENT)
}
