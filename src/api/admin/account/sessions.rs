//! `GET /api/admin/me/sessions`, `DELETE /api/admin/me/sessions/{handle}` and
//! `POST /api/admin/me/sessions/sign-out-others`: the caller's own live
//! sessions (same sign-in method and subject). The caller's session itself
//! ends only through sign-out, which also clears its cookie. CSRF applies as
//! on every unsafe admin route ([`AdminSession`]); each ended session is
//! audited like a sign-out.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::super::write::Refusal;
use crate::api::{
    auth::{
        AdminAuth, AdminSession,
        audit::AuditContext,
        own::{EndError, OwnSession},
    },
    dto::{
        account::{AccountSession, AccountSessions, SessionsEnded},
        iso_instant,
    },
    error::ApiError,
    listeners::Site,
};

fn auth(site: &Site) -> Result<&AdminAuth, ApiError> {
    site.auth.as_deref().ok_or(ApiError::AUTH_UNAVAILABLE)
}

fn refusal(error: EndError) -> Refusal {
    match error {
        EndError::NotFound => Refusal::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "That session has already ended.",
        ),
        EndError::Current => Refusal::new(
            StatusCode::CONFLICT,
            "current_session",
            "This is the session you are using; sign out instead.",
        ),
        EndError::Unavailable => ApiError::UNAVAILABLE.into(),
    }
}

fn row(own: OwnSession) -> AccountSession {
    AccountSession {
        handle: own.handle,
        method: own.session.method.as_str(),
        device: own.session.device,
        signed_in_at: iso_instant(own.session.created_at),
        last_seen_at: iso_instant(own.session.last_seen_at),
        current: own.current,
    }
}

pub(super) async fn list(
    State(site): State<Arc<Site>>,
    session: AdminSession,
) -> Result<Response, Refusal> {
    let auth = auth(&site)?;
    let mut sessions: Vec<AccountSession> = auth
        .own_sessions(&session)
        .await
        .map_err(refusal)?
        .into_iter()
        .map(row)
        .collect();
    // This one first, then the most recently seen.
    sessions.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then_with(|| b.last_seen_at.cmp(&a.last_seen_at))
    });
    Ok(Json(AccountSessions {
        sessions,
        generated_at: iso_instant(auth.now()),
    })
    .into_response())
}

pub(super) async fn end_one(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    session: AdminSession,
    UrlPath(handle): UrlPath<String>,
) -> Result<Response, Refusal> {
    auth(&site)?
        .end_own_session(&context, &session, &handle)
        .await
        .map_err(refusal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn end_others(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    session: AdminSession,
) -> Result<Response, Refusal> {
    let ended = auth(&site)?
        .end_other_sessions(&context, &session)
        .await
        .map_err(refusal)?;
    Ok(Json(SessionsEnded { ended }).into_response())
}
