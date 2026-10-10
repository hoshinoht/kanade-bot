//! What every member write on the public origin shares: admission (a member
//! write token, then the required `Idempotency-Key`), the member's origin and
//! the small body and path checks, and the audit of refused writes (stored at
//! most once a minute per member, route and reason). Fresh sign-in stays
//! with each write, since not all of them need it.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, Weak},
};

use axum::{
    body::{Body, to_bytes},
    extract::{Path as UrlPath, Request, State, rejection::PathRejection},
    http::{HeaderMap, Method, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::{
    api::{
        admin::write::{Refusal, required_idempotency_key},
        auth::{
            audit::{AuditContext, AuditEvent, AuditRecord, AuditSink, Realm, StderrAudit},
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

/// Identical refused writes (member, route, reason) within this window are
/// stored once, so a member cannot flood `auth_audit`; each still logs.
const REFUSAL_WINDOW: TimeDelta = TimeDelta::seconds(60);

/// Most refusals remembered per member realm; past it a refusal is stored
/// uncoalesced (never dropped) until older ones leave the window.
const MAX_RECENT_REFUSALS: usize = 1024;

/// When each (actor, route, reason) was last stored.
type Recent = HashMap<(String, String, String), DateTime<Utc>>;

/// Per member realm, held weakly: a dropped realm's entries go with it, and
/// a live weak keeps its address from being reused by a new realm.
static RECENT_REFUSALS: Mutex<Vec<(Weak<MemberAuth>, Recent)>> = Mutex::new(Vec::new());

/// Whether a refusal `key` at `now` is the first of its kind within
/// [`REFUSAL_WINDOW`] (by the realm's clock), remembering it if so.
fn first_in_window(
    member: &Arc<MemberAuth>,
    key: (String, String, String),
    now: DateTime<Utc>,
) -> bool {
    let mut realms = RECENT_REFUSALS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    realms.retain(|(realm, _)| realm.strong_count() > 0);
    let index = if let Some(index) = realms
        .iter()
        .position(|(realm, _)| std::ptr::eq(realm.as_ptr(), Arc::as_ptr(member)))
    {
        index
    } else {
        realms.push((Arc::downgrade(member), HashMap::new()));
        realms.len() - 1
    };
    let recent = &mut realms[index].1;
    // A clock set back ends the window too, rather than stretching it.
    recent.retain(|_, at| (TimeDelta::zero()..REFUSAL_WINDOW).contains(&(now - *at)));
    if recent.contains_key(&key) {
        return false;
    }
    if recent.len() < MAX_RECENT_REFUSALS {
        recent.insert(key, now);
    }
    true
}

/// Audits one refused member write: the log line always, the stored
/// sign-in row only for the first identical refusal (member, route, reason)
/// within [`REFUSAL_WINDOW`]. The hook for member write handlers outside
/// [`audit_refusals`]: `reason` is the refusal's code.
pub(super) fn refused_write(
    member: &Arc<MemberAuth>,
    audit: &AuditContext,
    session: &MemberSession,
    method: &Method,
    uri: &Uri,
    reason: &str,
) {
    let event = AuditEvent::write_refused(session.actor(), method, uri, reason);
    let now = member.now();
    let stored = match &event {
        AuditEvent::WriteRefused {
            actor,
            route,
            reason,
        } => first_in_window(member, (actor.clone(), route.clone(), reason.clone()), now),
        _ => true,
    };
    if stored {
        member.audit(audit, event);
    } else {
        let mut record = AuditRecord::new(Realm::Member, audit, event, now);
        record.client = audit.client.and_then(|ip| member.client_tag(Some(ip)));
        StderrAudit.record(record);
    }
}

/// Refusal bodies are one short `{error, message}` object.
const MAX_REFUSAL_BODY: usize = 16 * 1024;

#[derive(Deserialize)]
struct RefusalCode {
    error: String,
}

/// Middleware on member write routes (inside the session check): a write
/// (`POST`, `PUT`, `PATCH`, `DELETE`; other methods hit the unmounted
/// fallback and are no write) refused `403`, `404` or `409`, or
/// `401 reauth_required`, is audited with its code. CSRF refusals are
/// audited by the session check itself; rate refusals by [`admit`].
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
    let write = [Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(&method);
    let audited = write
        && matches!(
            status,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::NOT_FOUND
                | StatusCode::CONFLICT
        );
    let (Some(member), Some(session), true) = (site.member.as_ref(), session, audited) else {
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
