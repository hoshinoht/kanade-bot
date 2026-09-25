//! Sign-in, session and sign-out routes of the admin origin.
//!
//! Browser flows: `GET …/discord/start?next=/path` → Discord → `GET
//! …/discord/callback` → `303` to `next` (or `/?login_error=<code>`).
//! JSON flows: `POST …/tailscale`, `POST …/token` `{token}`. All end in the
//! `__Host-kanade_admin` session cookie; `GET /api/admin/session` returns the
//! caller and the CSRF token in `X-Kanade-CSRF`.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State, rejection::JsonRejection},
    http::{
        HeaderMap, HeaderValue, StatusCode, Uri,
        header::{LOCATION, SET_COOKIE},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    api::{
        auth::{
            AdminAuth, AdminSession,
            audit::AuditEvent,
            crypto,
            csrf::{self, CSRF_HEADER},
            discord::{CodeExchange, DiscordError},
            staff::StaffCheck,
            wire::{self, LOGIN_COOKIE, SESSION_COOKIE},
        },
        error::ApiError,
        listeners::Site,
    },
    infrastructure::store::web_sessions::LoginMethod,
};

const SESSION_SAME_SITE: &str = "Strict";
/// The pre-auth cookie must survive Discord's cross-site redirect back.
const LOGIN_SAME_SITE: &str = "Lax";
const LOGIN_COOKIE_SECONDS: i64 = 600;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/session", get(session))
        .route("/api/admin/auth/methods", get(methods))
        .route("/api/admin/auth/discord/start", get(discord_start))
        .route("/api/admin/auth/discord/callback", get(discord_callback))
        .route("/api/admin/auth/tailscale", post(tailscale_login))
        .route("/api/admin/auth/token", post(token_login))
        .route("/api/admin/auth/logout", post(logout))
}

#[derive(Serialize)]
struct SessionView {
    display: String,
}

fn session_cookie(auth: &AdminAuth, id: &str) -> HeaderValue {
    wire::set_cookie(
        SESSION_COOKIE,
        id,
        SESSION_SAME_SITE,
        auth.policy().absolute.num_seconds(),
    )
}

fn signed_in(display: String, csrf: Option<String>, cookie: Option<HeaderValue>) -> Response {
    let mut response = Json(SessionView { display }).into_response();
    let headers = response.headers_mut();
    if let Some(token) = csrf.and_then(|token| HeaderValue::from_str(&token).ok()) {
        headers.insert(CSRF_HEADER, token);
    }
    if let Some(cookie) = cookie {
        headers.append(SET_COOKIE, cookie);
    }
    response
}

fn see_other(location: &str, cookies: impl IntoIterator<Item = HeaderValue>) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    if let Ok(location) = HeaderValue::from_str(location) {
        headers.insert(LOCATION, location);
    }
    for cookie in cookies {
        headers.append(SET_COOKIE, cookie);
    }
    response
}

/// Browser-flow failures land on the SPA with a fixed code, never details.
fn login_error(code: &'static str) -> Response {
    see_other(
        &format!("/?login_error={code}"),
        [wire::clear_cookie(LOGIN_COOKIE, LOGIN_SAME_SITE)],
    )
}

async fn session(session: AdminSession) -> Response {
    let csrf = session.csrf_token();
    signed_in(session.display, csrf, None)
}

#[derive(Serialize)]
struct Methods {
    discord: bool,
    /// This request carries an allow-listed identity from the trusted edge.
    tailscale: bool,
    token: bool,
}

async fn methods(State(site): State<Arc<Site>>, request: Request) -> Json<Methods> {
    let (parts, _) = request.into_parts();
    Json(match site.auth.as_ref() {
        Some(auth) => Methods {
            discord: auth.discord().is_some(),
            tailscale: auth.tailscale_identity(&parts).is_some(),
            token: auth.breakglass_enabled(),
        },
        None => Methods {
            discord: false,
            tailscale: false,
            token: false,
        },
    })
}

async fn discord_start(State(site): State<Arc<Site>>, uri: Uri) -> Response {
    let pairs = wire::query_pairs(uri.query());
    let next = wire::safe_next(wire::query_value(&pairs, "next").as_deref());
    let Some(auth) = site.auth.as_ref() else {
        return login_error("unavailable");
    };
    let Some(started) = auth
        .discord()
        .and_then(|discord| discord.begin(next, auth.now()))
    else {
        return login_error("unavailable");
    };
    see_other(
        &started.authorize_url,
        [wire::set_cookie(
            LOGIN_COOKIE,
            &started.login_id,
            LOGIN_SAME_SITE,
            LOGIN_COOKIE_SECONDS,
        )],
    )
}

async fn discord_callback(State(site): State<Arc<Site>>, headers: HeaderMap, uri: Uri) -> Response {
    let Some((auth, discord)) = site
        .auth
        .as_ref()
        .and_then(|auth| auth.discord().map(|discord| (auth, discord)))
    else {
        return login_error("unavailable");
    };
    let refused = |reason: &'static str, code: &'static str| {
        auth.audit(AuditEvent::LoginRefused {
            method: "discord",
            reason,
        });
        login_error(code)
    };
    let pairs = wire::query_pairs(uri.query());
    let resumed = match (
        wire::cookie(&headers, LOGIN_COOKIE),
        wire::query_value(&pairs, "state"),
    ) {
        (Some(login_id), Some(state)) => discord.resume(&login_id, &state, auth.now()),
        _ => None,
    };
    let Some(resumed) = resumed else {
        return refused("state", "state");
    };
    if wire::query_value(&pairs, "error").is_some() {
        return refused("denied", "denied");
    }
    let Some(code) =
        wire::query_value(&pairs, "code").filter(|code| (1..=512).contains(&code.len()))
    else {
        return refused("no_code", "discord");
    };
    let token = match discord
        .api
        .exchange_code(CodeExchange {
            client: &discord.client,
            code: &code,
            code_verifier: &resumed.verifier,
        })
        .await
    {
        Ok(token) => token,
        Err(DiscordError::Rejected) => return refused("code_rejected", "discord"),
        Err(_) => return refused("discord_unavailable", "unavailable"),
    };
    let user = discord.api.current_user(&token).await;
    // One call per token: it is revoked before the identity is even used.
    discord.api.revoke(&discord.client, token).await;
    let user = match user {
        Ok(user) => user,
        Err(DiscordError::Rejected) => return refused("user_rejected", "discord"),
        Err(_) => return refused("discord_unavailable", "unavailable"),
    };
    match auth.staff().check(&user.id).await {
        StaffCheck::Staff => {}
        StaffCheck::NotStaff => return refused("not_staff", "forbidden"),
        StaffCheck::Unavailable => return refused("staff_unavailable", "unavailable"),
    }
    let replaces = wire::cookie(&headers, SESSION_COOKIE);
    let Some(id) = auth
        .start_session(
            LoginMethod::Discord,
            &user.id,
            &user.display(),
            replaces.as_deref(),
        )
        .await
    else {
        return login_error("unavailable");
    };
    see_other(
        &resumed.next,
        [
            session_cookie(auth, &id),
            wire::clear_cookie(LOGIN_COOKIE, LOGIN_SAME_SITE),
        ],
    )
}

fn auth_for_login(site: &Site, headers: &HeaderMap) -> Result<Arc<AdminAuth>, ApiError> {
    let auth = site.auth.clone().ok_or(ApiError::AUTH_UNAVAILABLE)?;
    // Login CSRF: a cross-site page must not sign a browser into our session.
    if !csrf::same_origin(headers) {
        return Err(ApiError::CSRF);
    }
    Ok(auth)
}

async fn tailscale_login(State(site): State<Arc<Site>>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let auth = match auth_for_login(&site, &parts.headers) {
        Ok(auth) => auth,
        Err(error) => return error.into_response(),
    };
    let Some((login, name)) = auth.tailscale_identity(&parts) else {
        auth.audit(AuditEvent::LoginRefused {
            method: "tailscale",
            reason: "no_identity",
        });
        return ApiError::UNAUTHENTICATED.into_response();
    };
    let replaces = wire::cookie(&parts.headers, SESSION_COOKIE);
    match auth
        .start_session(LoginMethod::Tailscale, &login, &name, replaces.as_deref())
        .await
    {
        Some(id) => signed_in(
            name,
            Some(csrf::token(&id)),
            Some(session_cookie(&auth, &id)),
        ),
        None => ApiError::UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenLogin {
    token: String,
}

async fn token_login(
    State(site): State<Arc<Site>>,
    headers: HeaderMap,
    body: Result<Json<TokenLogin>, JsonRejection>,
) -> Response {
    let auth = match auth_for_login(&site, &headers) {
        Ok(auth) => auth,
        Err(error) => return error.into_response(),
    };
    let Ok(Json(TokenLogin { token })) = body else {
        return ApiError::INVALID_BODY.into_response();
    };
    let Some(fingerprint) = auth.breakglass_matches(token.as_bytes()).map(str::to_owned) else {
        auth.audit(AuditEvent::LoginRefused {
            method: "token",
            reason: "bad_token",
        });
        return ApiError::UNAUTHENTICATED.into_response();
    };
    drop(token);
    auth.audit(AuditEvent::BreakGlassUsed {
        via: "login",
        request: "POST /api/admin/auth/token".into(),
    });
    let display = "Break-glass token";
    let replaces = wire::cookie(&headers, SESSION_COOKIE);
    match auth
        .start_session(
            LoginMethod::Token,
            &fingerprint,
            display,
            replaces.as_deref(),
        )
        .await
    {
        Some(id) => signed_in(
            display.into(),
            Some(csrf::token(&id)),
            Some(session_cookie(&auth, &id)),
        ),
        None => ApiError::UNAVAILABLE.into_response(),
    }
}

async fn logout(State(site): State<Arc<Site>>, session: AdminSession) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let (Some(auth), Some(id)) = (site.auth.as_ref(), session.session_id()) {
        let _ = auth
            .sessions()
            .delete_session(&crypto::sha256_hex(id.as_bytes()))
            .await;
        auth.audit(AuditEvent::SessionEnded {
            actor: session.actor.id().to_owned(),
            reason: "logout",
        });
        response.headers_mut().append(
            SET_COOKIE,
            wire::clear_cookie(SESSION_COOKIE, SESSION_SAME_SITE),
        );
    }
    response
}
