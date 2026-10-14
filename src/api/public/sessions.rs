//! The signed-in member's own routes, all behind `require_session`:
//! `GET /api/public/session` (who, the fresh window and the CSRF token in
//! `X-Kanade-CSRF`), the portrait, and the device list with sign out one and
//! sign out everywhere (D5-A).

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header::SET_COOKIE},
    response::{IntoResponse, Response},
};

use crate::api::{
    auth::{
        audit::AuditContext,
        crypto,
        csrf::CSRF_HEADER,
        member::{EndError, MemberAuth, MemberSession, SESSION_SAME_SITE},
        wire::{self, MEMBER_SESSION_COOKIE},
    },
    avatars::{AvatarRef, respond},
    dto::{
        account::SessionsEnded,
        iso_instant,
        public::{PublicMember, PublicSession, PublicSessionRow, PublicSessions},
    },
    error::ApiError,
    listeners::Site,
};

fn member(site: &Site) -> Result<&MemberAuth, ApiError> {
    site.member.as_deref().ok_or(ApiError::AUTH_UNAVAILABLE)
}

fn refusal(error: EndError) -> ApiError {
    match error {
        EndError::NotFound => ApiError::SESSION_ENDED,
        EndError::Current => ApiError::CURRENT_SESSION,
        EndError::Unavailable => ApiError::UNAVAILABLE,
    }
}

fn avatar(session: &MemberSession) -> Option<AvatarRef> {
    AvatarRef::user(&session.user_id, session.avatar_hash.as_deref()?)
}

pub(super) async fn session(session: MemberSession) -> Response {
    // Changes with the avatar or the name, so the browser refetches the portrait.
    let version = crypto::sha256_hex(
        format!(
            "{}\0{}",
            session.avatar_hash.as_deref().unwrap_or_default(),
            session.display
        )
        .as_bytes(),
    );
    let mut response = Json(PublicSession {
        member: PublicMember {
            id: session.user_id.clone(),
            display: session.display.clone(),
            avatar: format!("/api/public/session/avatar?v={}", &version[..12]),
        },
        fresh_until: iso_instant(session.fresh_until),
    })
    .into_response();
    if let Some(token) = session
        .csrf()
        .and_then(|token| HeaderValue::from_str(&token).ok())
    {
        response.headers_mut().insert(CSRF_HEADER, token);
    }
    response
}

/// The avatar hash stored at sign-in (D8-A), else the monogram.
pub(super) async fn avatar_image(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let member = member(&site)?;
    let found = match (member.portraits(), avatar(&session)) {
        (Some(portraits), Some(avatar)) => portraits.portrait(Some(&avatar)).await,
        _ => None,
    };
    Ok(respond(found, &session.display, &headers))
}

pub(super) async fn list(
    State(site): State<Arc<Site>>,
    session: MemberSession,
) -> Result<Json<PublicSessions>, ApiError> {
    let member = member(&site)?;
    let sessions = member
        .devices(&session)
        .await
        .map_err(refusal)?
        .into_iter()
        .map(|device| PublicSessionRow {
            handle: device.handle,
            device: device.session.device,
            signed_in_at: iso_instant(device.session.created_at),
            last_seen_at: iso_instant(device.session.last_seen_at),
            current: device.current,
        })
        .collect();
    Ok(Json(PublicSessions {
        sessions,
        generated_at: iso_instant(member.now()),
    }))
}

pub(super) async fn end_one(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let member = member(&site)?;
    let Ok(UrlPath(handle)) = path else {
        return Err(ApiError::SESSION_ENDED);
    };
    member
        .end_device(&context, &session, &handle)
        .await
        .map_err(refusal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every public session of the member, this one included; clears the cookie.
pub(super) async fn end_all(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    session: MemberSession,
) -> Result<Response, ApiError> {
    let member = member(&site)?;
    let ended = member
        .end_everywhere(&context, &session)
        .await
        .map_err(refusal)?;
    let mut response = Json(SessionsEnded { ended }).into_response();
    response.headers_mut().append(
        SET_COOKIE,
        wire::clear_cookie(MEMBER_SESSION_COOKIE, SESSION_SAME_SITE),
    );
    Ok(response)
}
