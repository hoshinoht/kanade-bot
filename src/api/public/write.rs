//! What every member write on the public origin shares: admission (a member
//! write token, then the required `Idempotency-Key`), the member's origin and
//! the small body and path checks, and the audit of refused writes. Fresh
//! sign-in stays with each write, since not all of them need it.

use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    extract::{Path as UrlPath, Request, State, rejection::PathRejection},
    http::{HeaderMap, Method, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    api::{
        admin::write::{Refusal, required_idempotency_key},
        auth::{
            audit::{AuditContext, AuditEvent},
            crypto,
            member::{MemberAuth, MemberSession},
            rate::MEMBER_WRITE_ROUTE,
        },
        error::ApiError,
        listeners::Site,
    },
    domain::history::{Actor, Origin, Surface},
};

/// A Discord user id: 17-20 digits.
pub(super) fn snowflake(text: &str) -> bool {
    (17..=20).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn invalid_body() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_body",
        "The request body is not valid.",
    )
}

pub(super) fn path_id(path: Result<UrlPath<String>, PathRejection>) -> Result<String, Refusal> {
    path.map(|UrlPath(id)| id)
        .map_err(|_| ApiError::NOT_FOUND.into())
}

/// Every member write: one of the member's write tokens (a refusal is
/// audited), then the required `Idempotency-Key`.
pub(super) fn admit<'a>(
    site: &'a Site,
    audit: &AuditContext,
    session: &MemberSession,
    headers: &'a HeaderMap,
) -> Result<(&'a MemberAuth, &'a str), Refusal> {
    let member = site.member.as_deref().ok_or(ApiError::AUTH_UNAVAILABLE)?;
    if !member
        .rate()
        .take_member_write(&session.user_id, member.now())
    {
        member.audit(
            audit,
            AuditEvent::RateLimited {
                route: MEMBER_WRITE_ROUTE,
            },
        );
        return Err(ApiError::RATE_LIMITED.into());
    }
    Ok((member, required_idempotency_key(headers)?))
}

/// Audits one refused member write (log line only). The hook for member
/// write handlers outside [`audit_refusals`]: `reason` is the refusal's code.
pub(super) fn refused_write(
    member: &MemberAuth,
    audit: &AuditContext,
    session: &MemberSession,
    method: &Method,
    uri: &Uri,
    reason: &str,
) {
    member.audit(
        audit,
        AuditEvent::write_refused(session.actor(), method, uri, reason),
    );
}

/// Refusal bodies are one short `{error, message}` object.
const MAX_REFUSAL_BODY: usize = 16 * 1024;

#[derive(Deserialize)]
struct RefusalCode {
    error: String,
}

/// Middleware on member write routes (inside the session check): a write
/// refused `403`, `404` or `409`, or `401 reauth_required`, is audited with
/// its code. CSRF refusals are audited by the session check itself; rate
/// refusals by [`admit`].
pub(super) async fn audit_refusals(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    request: Request,
    next: Next,
) -> Response {
    let (method, uri) = (request.method().clone(), request.uri().clone());
    let session = request.extensions().get::<MemberSession>().cloned();
    let response = next.run(request).await;
    let status = response.status();
    let audited = matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::CONFLICT
    );
    let (Some(member), Some(session), true) = (site.member.as_deref(), session, audited) else {
        return response;
    };
    let (parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_REFUSAL_BODY).await else {
        return ApiError::UNAVAILABLE.into_response();
    };
    if let Ok(RefusalCode { error }) = serde_json::from_slice(&bytes)
        && (status != StatusCode::UNAUTHORIZED || error == ApiError::REAUTH_REQUIRED.error)
    {
        refused_write(member, &audit, &session, &method, &uri, &error);
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// The member through the portal; `public:` keeps the request id apart from
/// Discord's and the admin portal's.
pub(super) fn origin(session: &MemberSession, key: &str) -> Origin {
    Origin::new(
        Actor::member(session.user_id.clone()),
        Surface::PublicPortal,
    )
    .with_request_id(format!("public:{key}"))
}

/// The id of what `member` creates with `key` (an ownership ask, a request),
/// so a retry with the same key finds it.
pub(super) fn keyed_id(member: &str, key: &str) -> String {
    let digest = crypto::sha256_hex(format!("{member}:{key}").as_bytes());
    format!("public-{}", &digest[..40])
}
