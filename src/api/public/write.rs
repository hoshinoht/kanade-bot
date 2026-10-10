//! What every member write on the public origin shares: admission (a member
//! write token, then the required `Idempotency-Key`), the member's origin and
//! the small body and path checks. Fresh sign-in stays with each write, since
//! not all of them need it.

use axum::{
    extract::{Path as UrlPath, rejection::PathRejection},
    http::{HeaderMap, StatusCode},
};

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
