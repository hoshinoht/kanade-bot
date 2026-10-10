//! [`require_session`]: the session-required public routes' middleware, in
//! the contract's order (closed, load, expiry, D9 grace, client-tag
//! rotation, eligibility re-check, CSRF, touch). It inserts the
//! [`MemberSession`] handlers take as an argument. The member event stream
//! sits behind [`MemberSession::require_quietly`] instead: holding it open
//! (and its reconnects) is not activity.

use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, Method, header::SET_COOKIE, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};

use super::{Eligibility, MemberAuth};
use crate::{
    api::{
        auth::{
            TOUCH_EVERY, actor_id,
            audit::{AuditContext, AuditEvent},
            crypto,
            csrf::{self, CSRF_HEADER},
            wire::{self, MEMBER_SESSION_COOKIE},
        },
        error::ApiError,
        listeners::Site,
    },
    infrastructure::store::web_sessions::{LoginMethod, SessionOrigin, WebSession},
};

/// The member cookie's `SameSite`: never sent on a cross-site request.
pub(crate) const SESSION_SAME_SITE: &str = "Strict";

/// The signed-in member, set by [`require_session`].
#[derive(Clone)]
pub struct MemberSession {
    pub user_id: String,
    pub display: String,
    pub avatar_hash: Option<String>,
    pub fresh_until: DateTime<Utc>,
    id_hash: String,
    /// The cookie value; `None` while a rotated-out id is served in its grace.
    id: Option<String>,
}

impl MemberSession {
    fn of(row: &WebSession, id: Option<String>, fresh: chrono::TimeDelta) -> Self {
        Self {
            user_id: row.subject.clone(),
            display: row.display.clone(),
            avatar_hash: row.avatar_hash.clone(),
            // Rotation keeps `created_at`, so it never extends the window.
            fresh_until: row.created_at + fresh,
            id_hash: row.id_hash.clone(),
            id,
        }
    }

    /// `401 reauth_required` once the fresh-write window has passed.
    pub fn require_fresh(&self, now: DateTime<Utc>) -> Result<(), ApiError> {
        if now < self.fresh_until {
            Ok(())
        } else {
            Err(ApiError::REAUTH_REQUIRED)
        }
    }

    /// `discord:<id>`, audited with realm `member`.
    pub fn actor(&self) -> String {
        actor_id(LoginMethod::Discord, &self.user_id)
    }

    /// The member CSRF token. `None` while a rotated-out id is served in its
    /// grace: its token can no longer authorize a write, so it is not handed
    /// out to replace the new one the client already holds.
    pub fn csrf(&self) -> Option<String> {
        self.id.as_deref().map(|id| csrf::token(csrf::MEMBER, id))
    }

    pub(crate) fn id_hash(&self) -> &str {
        &self.id_hash
    }
}

impl<S: Send + Sync> FromRequestParts<S> for MemberSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(ApiError::UNAUTHENTICATED)
    }
}

/// What the response does with the session cookie.
enum Cookie {
    Keep,
    Clear,
    /// The id rotated: the new cookie (remaining lifetime) and its CSRF token.
    Rotated {
        id: String,
        expires_at: DateTime<Utc>,
    },
}

pub async fn require_session(
    State(site): State<Arc<Site>>,
    request: Request,
    next: Next,
) -> Response {
    admit(&site, request, next, true).await
}

impl MemberSession {
    /// As [`require_session`], but the request never counts as activity: no
    /// touch (a due eligibility re-check is still recorded) and no client-tag
    /// rotation, whose new cookie an `EventSource` could take but whose CSRF
    /// token the page could never read; the next ordinary request rotates.
    /// Closed, expiry and eligibility refuse it as usual.
    pub async fn require_quietly(
        State(site): State<Arc<Site>>,
        request: Request,
        next: Next,
    ) -> Response {
        admit(&site, request, next, false).await
    }
}

async fn admit(site: &Site, request: Request, next: Next, activity: bool) -> Response {
    let Some(auth) = site.member.clone() else {
        return ApiError::AUTH_UNAVAILABLE.into_response();
    };
    let (mut parts, body) = request.into_parts();
    let (admitted, cookie) = auth.authenticate(&parts, activity).await;
    let mut response = match admitted {
        Ok(session) => {
            parts.extensions.insert(session);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(error) => error.into_response(),
    };
    let headers = response.headers_mut();
    match cookie {
        Cookie::Keep => {}
        Cookie::Clear => {
            headers.append(
                SET_COOKIE,
                wire::clear_cookie(MEMBER_SESSION_COOKIE, SESSION_SAME_SITE),
            );
        }
        Cookie::Rotated { id, expires_at } => {
            headers.append(SET_COOKIE, auth.session_cookie(&id, expires_at));
            if let Ok(token) = HeaderValue::from_str(&csrf::token(csrf::MEMBER, &id)) {
                headers.insert(CSRF_HEADER, token);
            }
        }
    }
    response
}

impl MemberAuth {
    /// `__Host-kanade_pub` living exactly as long as the session may.
    pub(crate) fn session_cookie(&self, id: &str, expires_at: DateTime<Utc>) -> HeaderValue {
        wire::set_cookie(
            MEMBER_SESSION_COOKIE,
            id,
            SESSION_SAME_SITE,
            (expires_at - self.now()).num_seconds(),
        )
    }

    async fn authenticate(
        &self,
        parts: &Parts,
        activity: bool,
    ) -> (Result<MemberSession, ApiError>, Cookie) {
        // 2. Closed: the row is left to expire as usual.
        if !self.is_open() {
            return (Err(ApiError::CLOSED), Cookie::Keep);
        }
        let context = AuditContext::of(parts);
        let policy = self.policy();
        let now = self.now();
        // 3. Load: a live id, else a rotated-out one still in its grace.
        let Some(id) = wire::cookie(&parts.headers, MEMBER_SESSION_COOKIE) else {
            return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear);
        };
        let hash = crypto::sha256_hex(id.as_bytes());
        let row = match self.sessions().load_session(&hash).await {
            Ok(Some(row)) => row,
            Ok(None) => match self.sessions().load_superseded(&hash, now).await {
                Ok(Some(row)) => row,
                Ok(None) => return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear),
                Err(_) => return (Err(ApiError::UNAVAILABLE), Cookie::Keep),
            },
            Err(_) => return (Err(ApiError::UNAVAILABLE), Cookie::Keep),
        };
        if row.origin != SessionOrigin::Public || row.method != LoginMethod::Discord {
            return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear);
        }
        // 4. Expiry.
        let expired = if row.expires_at <= now {
            Some("expired")
        } else if row.last_seen_at + policy.idle <= now {
            Some("idle")
        } else {
            None
        };
        if let Some(reason) = expired {
            let _ = self.sessions().delete_session(&hash).await;
            self.audit(
                &context,
                AuditEvent::SessionEnded {
                    actor: actor_id(LoginMethod::Discord, &row.subject),
                    reason,
                },
            );
            return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear);
        }
        // 5. A rotated-out id (D9): reads only, and only within its grace.
        // The browser may already hold the new cookie, so it is not cleared.
        if row.superseded_until.is_some() {
            return if matches!(parts.method, Method::GET | Method::HEAD) {
                (
                    Ok(MemberSession::of(&row, None, policy.fresh)),
                    Cookie::Keep,
                )
            } else {
                (Err(ApiError::UNAUTHENTICATED), Cookie::Keep)
            };
        }
        // 6. A new client address rotates the id, never the lifetimes (not
        // on a quiet request: see `MemberSession::require_quietly`).
        let Some(tag) = self.client_tag(context.client) else {
            return (Err(ApiError::AUTH_UNAVAILABLE), Cookie::Keep);
        };
        let mut current = (id.clone(), row.clone());
        let mut cookie = Cookie::Keep;
        if activity && row.client_tag.as_deref() != Some(tag.as_str()) {
            let Some(new_id) = crypto::random_token() else {
                return (Err(ApiError::AUTH_UNAVAILABLE), Cookie::Keep);
            };
            let rotated = WebSession {
                id_hash: crypto::sha256_hex(new_id.as_bytes()),
                last_seen_at: now,
                client_tag: Some(tag),
                superseded_until: None,
                ..row.clone()
            };
            match self
                .sessions()
                .rotate_session(&rotated, &hash, now + policy.grace)
                .await
            {
                Ok(true) => {}
                // Ended or rotated by a concurrent request.
                Ok(false) => return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear),
                Err(_) => return (Err(ApiError::UNAVAILABLE), Cookie::Keep),
            }
            self.audit(
                &context,
                AuditEvent::SessionRotated {
                    actor: actor_id(LoginMethod::Discord, &row.subject),
                },
            );
            cookie = Cookie::Rotated {
                id: new_id.clone(),
                expires_at: rotated.expires_at,
            };
            current = (new_id, rotated);
        }
        let rotated = matches!(cookie, Cookie::Rotated { .. });
        // 7. Re-check eligibility when due, and always after a rotation.
        let mut checked_at = current.1.checked_at;
        if rotated || checked_at + policy.recheck <= now {
            match self.gate().check(&row.subject).await {
                Eligibility::Eligible => checked_at = now,
                Eligibility::NotEligible => {
                    let _ = self.end_all(&context, &row.subject, "not_eligible").await;
                    return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear);
                }
                Eligibility::Unavailable => return (Err(ApiError::AUTH_UNAVAILABLE), cookie),
            }
        }
        // 8. CSRF: the token of the id the request came with.
        if csrf::is_unsafe(&parts.method)
            && !(csrf::same_origin(&parts.headers)
                && csrf::token_matches(csrf::MEMBER, &parts.headers, &id))
        {
            self.audit(
                &context,
                AuditEvent::write_refused(
                    actor_id(LoginMethod::Discord, &row.subject),
                    &parts.method,
                    &parts.uri,
                    "csrf",
                ),
            );
            return (Err(ApiError::CSRF), cookie);
        }
        // 9. Touch at most once a minute (or to record the re-check); a quiet
        // request records only the re-check and keeps `last_seen_at`.
        let (current_id, current_row) = current;
        let seen = if activity {
            now
        } else {
            current_row.last_seen_at
        };
        let stale = activity && now - current_row.last_seen_at >= TOUCH_EVERY;
        if stale || checked_at != current_row.checked_at {
            match self
                .sessions()
                .touch_session(&current_row.id_hash, seen, checked_at)
                .await
            {
                Ok(true) => {}
                Ok(false) => return (Err(ApiError::UNAUTHENTICATED), Cookie::Clear),
                Err(_) => return (Err(ApiError::UNAVAILABLE), cookie),
            }
        }
        (
            Ok(MemberSession::of(
                &current_row,
                Some(current_id),
                policy.fresh,
            )),
            cookie,
        )
    }
}
