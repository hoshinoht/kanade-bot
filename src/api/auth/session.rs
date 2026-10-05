//! The [`AdminSession`] extractor: bearer break-glass or the session cookie,
//! idle/absolute expiry, per-method re-check, CSRF on unsafe methods.

use std::sync::Arc;

use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
};

use super::{
    AdminAuth, TAILSCALE_LOGIN, TAILSCALE_NAME, TOUCH_EVERY, actor_id,
    audit::{AuditContext, AuditEvent},
    crypto, csrf,
    rate::Route,
    staff::StaffCheck,
    wire::{self, SESSION_COOKIE},
};
use crate::{
    api::{error::ApiError, guard::proxy::Peer, listeners::Site},
    domain::history::{Actor, Origin, Surface},
    infrastructure::store::web_sessions::{LoginMethod, SessionOrigin, WebSession},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminSession {
    pub actor: Actor,
    pub method: LoginMethod,
    pub display: String,
    /// The Discord avatar hash stored at sign-in (Discord sessions only).
    pub avatar_hash: Option<String>,
    /// `None` for bearer (CLI) requests, which carry no ambient credential.
    session_id: Option<String>,
}

impl AdminSession {
    pub fn csrf_token(&self) -> Option<String> {
        self.session_id.as_deref().map(csrf::token)
    }

    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The Discord user id of a Discord sign-in (Tailscale and token
    /// sessions have none).
    pub fn discord_user(&self) -> Option<&str> {
        match self.method {
            LoginMethod::Discord => self.actor.id().strip_prefix("discord:"),
            LoginMethod::Tailscale | LoginMethod::Token => None,
        }
    }

    /// Attribution for history records.
    pub fn origin(&self) -> Origin {
        let surface = if self.session_id.is_some() {
            Surface::AdminPortal
        } else {
            Surface::Cli
        };
        Origin::new(self.actor.clone(), surface)
    }
}

impl FromRequestParts<Arc<Site>> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        let auth = site.auth.as_ref().ok_or(ApiError::AUTH_UNAVAILABLE)?;
        auth.authenticate(parts).await
    }
}

impl AdminAuth {
    /// The allow-listed Tailscale login (and name) the trusted edge vouches for.
    pub(crate) fn tailscale_identity(&self, parts: &Parts) -> Option<(String, String)> {
        // The proxy guard already stripped these headers unless the edge proved
        // itself with the edge secret; check again anyway.
        let edge = parts
            .extensions
            .get::<Peer>()
            .is_some_and(|peer| peer.edge_authenticated);
        if !edge || !self.tailscale_enabled() {
            return None;
        }
        let single = |name: &str| {
            let mut values = parts.headers.get_all(name).iter();
            match (values.next(), values.next()) {
                (Some(value), None) => value.to_str().ok().map(str::trim).map(str::to_owned),
                _ => None,
            }
        };
        let login = single(TAILSCALE_LOGIN)?.to_ascii_lowercase();
        if login.is_empty() || login.len() > 320 || !self.tailscale_allows(&login) {
            return None;
        }
        let name = single(TAILSCALE_NAME)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| login.clone());
        Some((
            login,
            name.chars().filter(|c| !c.is_control()).take(100).collect(),
        ))
    }

    pub(crate) async fn authenticate(&self, parts: &Parts) -> Result<AdminSession, ApiError> {
        self.authenticate_with(parts, true).await
    }

    /// As [`Self::authenticate`], but never extends the idle window: an open
    /// event stream (and its reconnects) is not activity, so a hidden or
    /// forgotten tab still signs out after the idle limit. A due staff
    /// re-check is still recorded.
    pub(crate) async fn authenticate_quietly(
        &self,
        parts: &Parts,
    ) -> Result<AdminSession, ApiError> {
        self.authenticate_with(parts, false).await
    }

    async fn authenticate_with(
        &self,
        parts: &Parts,
        activity: bool,
    ) -> Result<AdminSession, ApiError> {
        let context = AuditContext::of(parts);
        if let Some(header) = parts.headers.get(AUTHORIZATION) {
            return self.bearer(header.as_bytes(), parts, &context);
        }
        let id = wire::cookie(&parts.headers, SESSION_COOKIE).ok_or(ApiError::UNAUTHENTICATED)?;
        let hash = crypto::sha256_hex(id.as_bytes());
        let session = self
            .sessions()
            .load_session(&hash)
            .await
            .map_err(|_| ApiError::UNAVAILABLE)?
            .filter(|session| session.origin == SessionOrigin::Admin)
            .ok_or(ApiError::UNAUTHENTICATED)?;
        let now = self.now();
        let policy = self.policy();
        if now >= session.expires_at {
            return Err(self.end(&context, &session, "expired").await);
        }
        if now - session.last_seen_at >= policy.idle {
            return Err(self.end(&context, &session, "idle").await);
        }
        let mut checked_at = session.checked_at;
        let due = now - checked_at >= policy.recheck;
        match session.method {
            LoginMethod::Discord if due => match self.staff().check(&session.subject).await {
                StaffCheck::Staff => checked_at = now,
                StaffCheck::NotStaff => {
                    let _ = self
                        .sessions()
                        .delete_subject_sessions(
                            SessionOrigin::Admin,
                            LoginMethod::Discord,
                            &session.subject,
                        )
                        .await;
                    self.audit(
                        &context,
                        AuditEvent::SessionEnded {
                            actor: actor_id(session.method, &session.subject),
                            reason: "not_staff",
                        },
                    );
                    return Err(ApiError::UNAUTHENTICATED);
                }
                StaffCheck::Unavailable => return Err(ApiError::AUTH_UNAVAILABLE),
            },
            LoginMethod::Discord => {}
            // Every request must still carry the same edge-vouched identity.
            LoginMethod::Tailscale => match self.tailscale_identity(parts) {
                Some((login, _)) if login == session.subject => {
                    if due {
                        checked_at = now;
                    }
                }
                _ => return Err(self.end(&context, &session, "identity_changed").await),
            },
            LoginMethod::Token => {
                if self.breakglass_fingerprint() != Some(session.subject.as_str()) {
                    return Err(self.end(&context, &session, "token_rotated").await);
                }
            }
        }
        if csrf::is_unsafe(&parts.method)
            && !(csrf::same_origin(&parts.headers) && csrf::token_matches(&parts.headers, &id))
        {
            return Err(ApiError::CSRF);
        }
        let seen = if activity { now } else { session.last_seen_at };
        let stale = activity && now - session.last_seen_at >= TOUCH_EVERY;
        if (stale || checked_at != session.checked_at)
            && !self
                .sessions()
                .touch_session(&hash, seen, checked_at)
                .await
                .map_err(|_| ApiError::UNAVAILABLE)?
        {
            return Err(ApiError::UNAUTHENTICATED);
        }
        Ok(AdminSession {
            actor: Actor::admin(actor_id(session.method, &session.subject)),
            method: session.method,
            display: session.display,
            avatar_hash: session.avatar_hash,
            session_id: Some(id),
        })
    }

    /// Whether the cookie session `id` still stands: not signed out or
    /// revoked, not past its absolute or idle expiry. An open event stream
    /// asks this on every heartbeat; the full re-checks run when it reconnects.
    pub(crate) async fn session_live(&self, id: &str) -> bool {
        let hash = crypto::sha256_hex(id.as_bytes());
        let Ok(Some(session)) = self.sessions().load_session(&hash).await else {
            return false;
        };
        let now = self.now();
        session.origin == SessionOrigin::Admin
            && now < session.expires_at
            && now - session.last_seen_at < self.policy().idle
    }

    fn bearer(
        &self,
        header: &[u8],
        parts: &Parts,
        context: &AuditContext,
    ) -> Result<AdminSession, ApiError> {
        let now = self.now();
        // The client's failures are checked before comparing, so it cannot keep
        // guessing; the global bucket only ever refuses wrong tokens, so guesses
        // from many addresses cannot lock out the right one.
        if !self
            .rate()
            .allows_client(Route::BearerFailure, context.client, now)
        {
            self.audit(
                context,
                AuditEvent::RateLimited {
                    route: Route::BearerFailure.as_str(),
                },
            );
            return Err(ApiError::RATE_LIMITED);
        }
        let token = header
            .strip_prefix(b"Bearer ")
            .or_else(|| header.strip_prefix(b"bearer "));
        if let Some(token) = token
            && self.breakglass_matches(token).is_some()
        {
            self.audit(
                context,
                AuditEvent::BreakGlassUsed {
                    via: "bearer",
                    request: format!("{} {}", parts.method, parts.uri.path()),
                },
            );
            return Ok(AdminSession {
                actor: Actor::admin(super::TOKEN_ACTOR),
                method: LoginMethod::Token,
                display: "Break-glass token".into(),
                avatar_hash: None,
                session_id: None,
            });
        }
        self.rate()
            .take_client(Route::BearerFailure, context.client, now);
        if !self.rate().take_global(Route::BearerFailure, now) {
            self.audit(
                context,
                AuditEvent::RateLimited {
                    route: Route::BearerFailure.as_str(),
                },
            );
            return Err(ApiError::RATE_LIMITED);
        }
        self.audit(
            context,
            AuditEvent::LoginRefused {
                method: "token",
                reason: "bad_bearer",
                user: None,
            },
        );
        Err(ApiError::UNAUTHENTICATED)
    }

    async fn end(
        &self,
        context: &AuditContext,
        session: &WebSession,
        reason: &'static str,
    ) -> ApiError {
        let _ = self.sessions().delete_session(&session.id_hash).await;
        self.audit(
            context,
            AuditEvent::SessionEnded {
                actor: actor_id(session.method, &session.subject),
                reason,
            },
        );
        ApiError::UNAUTHENTICATED
    }
}
