//! Sign-in, session and sign-out routes of the admin origin.
//!
//! Browser flows: `GET …/discord/start?next=/path` → Discord → `GET
//! …/discord/callback` → `200` landing page that navigates to `next` (or
//! `303 /?login_error=<code>`). JSON flows: `POST …/tailscale`, `POST …/token`
//! `{token}`. All end in the `__Host-kanade_admin` session cookie;
//! `GET /api/admin/session` returns the caller and the CSRF token in
//! `X-Kanade-CSRF`.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State, rejection::JsonRejection},
    http::{
        HeaderMap, HeaderValue, StatusCode, Uri,
        header::{CONTENT_TYPE, LOCATION, SET_COOKIE},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    api::{
        auth::{
            AdminAuth, AdminSession,
            audit::{AuditContext, AuditEvent},
            crypto,
            csrf::{self, CSRF_HEADER},
            discord::{BeginError, CodeExchange, DiscordError, DiscordLogin},
            rate::Route,
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
    /// `discord`, `tailscale` or `token`: only Discord sessions may decide proposals.
    method: &'static str,
}

fn session_cookie(auth: &AdminAuth, id: &str) -> HeaderValue {
    wire::set_cookie(
        SESSION_COOKIE,
        id,
        SESSION_SAME_SITE,
        auth.policy().absolute.num_seconds(),
    )
}

fn signed_in(
    display: String,
    method: LoginMethod,
    csrf: Option<String>,
    cookie: Option<HeaderValue>,
) -> Response {
    let mut response = Json(SessionView {
        display,
        method: method.as_str(),
    })
    .into_response();
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

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A same-origin page that navigates to `next`. A `SameSite=Strict` cookie
/// set during Discord's cross-site redirect is not sent on a redirect chain
/// that started cross-site; a navigation started by our own page is
/// same-site, so the landing request carries the session. No script: the
/// CSP (`default-src 'none'`) does not govern meta refresh.
fn landing(next: &str, cookies: impl IntoIterator<Item = HeaderValue>) -> Response {
    let next = escape_html(next);
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0; url={next}\"><title>Signed in</title></head>\
         <body><p><a href=\"{next}\">Continue to Kanade</a></p></body></html>"
    );
    let mut response = (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response();
    for cookie in cookies {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

async fn session(session: AdminSession) -> Response {
    let csrf = session.csrf_token();
    signed_in(session.display, session.method, csrf, None)
}

#[derive(Serialize)]
struct Methods {
    discord: bool,
    /// This request carries an allow-listed identity from the authenticated edge.
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

fn rate_limited(auth: &AdminAuth, context: &AuditContext, route: Route) -> Response {
    auth.audit(
        context,
        AuditEvent::RateLimited {
            route: route.as_str(),
        },
    );
    ApiError::RATE_LIMITED.into_response()
}

/// Take a rate-limit token; audits and answers `false` when refused.
fn admitted(auth: &AdminAuth, context: &AuditContext, route: Route) -> bool {
    let admitted = auth.rate().take(route, context.client, auth.now());
    if !admitted {
        auth.audit(
            context,
            AuditEvent::RateLimited {
                route: route.as_str(),
            },
        );
    }
    admitted
}

async fn discord_start(State(site): State<Arc<Site>>, context: AuditContext, uri: Uri) -> Response {
    let pairs = wire::query_pairs(uri.query());
    let next = wire::safe_next(wire::query_value(&pairs, "next").as_deref());
    let Some((auth, discord)) = discord_of(&site) else {
        return login_error("unavailable");
    };
    if !admitted(auth, &context, Route::DiscordStart) {
        return login_error("rate_limited");
    }
    let now = auth.now();
    if discord.cooling_down(now) {
        return login_error("unavailable");
    }
    match discord.begin(next, now, context.client) {
        Ok(started) => see_other(
            &started.authorize_url,
            [wire::set_cookie(
                LOGIN_COOKIE,
                &started.login_id,
                LOGIN_SAME_SITE,
                LOGIN_COOKIE_SECONDS,
            )],
        ),
        Err(BeginError::Busy) => {
            auth.audit(
                &context,
                AuditEvent::RateLimited {
                    route: "pending_logins",
                },
            );
            login_error("rate_limited")
        }
        Err(BeginError::Unavailable) => login_error("unavailable"),
    }
}

fn discord_of(site: &Site) -> Option<(&AdminAuth, &DiscordLogin)> {
    site.auth
        .as_ref()
        .and_then(|auth| auth.discord().map(|discord| (auth.as_ref(), discord)))
}

async fn discord_callback(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some((auth, discord)) = discord_of(&site) else {
        return login_error("unavailable");
    };
    let pairs = wire::query_pairs(uri.query());
    let resumed = match (
        wire::cookie(&headers, LOGIN_COOKIE),
        wire::query_value(&pairs, "state"),
    ) {
        (Some(login_id), Some(state)) => discord.resume(&login_id, &state, auth.now()),
        _ => None,
    };
    if !admitted(auth, &context, Route::DiscordCallback) {
        return login_error("rate_limited");
    }
    let refused = |reason: &'static str, user: Option<&str>, code: &'static str| {
        auth.audit(
            &context,
            AuditEvent::LoginRefused {
                method: "discord",
                reason,
                user: user.map(str::to_owned),
            },
        );
        login_error(code)
    };
    let cooled = |wait| {
        discord.cool_down(auth.now(), wait);
        refused("discord_rate_limited", None, "unavailable")
    };
    let Some(resumed) = resumed else {
        return refused("state", None, "state");
    };
    if wire::query_value(&pairs, "error").is_some() {
        return refused("denied", None, "denied");
    }
    let Some(code) =
        wire::query_value(&pairs, "code").filter(|code| (1..=512).contains(&code.len()))
    else {
        return refused("no_code", None, "discord");
    };
    if discord.cooling_down(auth.now()) {
        return refused("discord_cooling_down", None, "unavailable");
    }
    let grant = match discord
        .api
        .exchange_code(CodeExchange {
            client: &discord.client,
            code: &code,
            code_verifier: &resumed.verifier,
        })
        .await
    {
        Ok(grant) => grant,
        Err(DiscordError::Rejected) => return refused("code_rejected", None, "discord"),
        Err(DiscordError::RateLimited(wait)) => return cooled(wait),
        Err(_) => return refused("discord_unavailable", None, "unavailable"),
    };
    let scope_ok = grant.identify_only();
    // A broader grant is refused without using the token at all.
    let user = if scope_ok {
        Some(discord.api.current_user(&grant.token).await)
    } else {
        None
    };
    // One call per token: it is revoked before the identity is even used.
    let user_id = user
        .as_ref()
        .and_then(|user| user.as_ref().ok())
        .map(|user| user.id.clone());
    if let Err(error) = discord.api.revoke(&discord.client, grant.token).await {
        if let DiscordError::RateLimited(wait) = error {
            discord.cool_down(auth.now(), wait);
        }
        auth.audit(
            &context,
            AuditEvent::RevokeFailed {
                reason: match error {
                    DiscordError::RateLimited(_) => "rate_limited",
                    DiscordError::Rejected => "rejected",
                    _ => "unavailable",
                },
                user: user_id.clone(),
            },
        );
    }
    let user = match user {
        None => return refused("scope", None, "discord"),
        Some(Ok(user)) => user,
        Some(Err(DiscordError::Rejected)) => return refused("user_rejected", None, "discord"),
        Some(Err(DiscordError::RateLimited(wait))) => return cooled(wait),
        Some(Err(_)) => return refused("discord_unavailable", None, "unavailable"),
    };
    if user.bot {
        return refused("bot_account", Some(&user.id), "forbidden");
    }
    match auth.staff().check(&user.id).await {
        StaffCheck::Staff => {}
        StaffCheck::NotStaff => return refused("not_staff", Some(&user.id), "forbidden"),
        StaffCheck::Unavailable => {
            return refused("staff_unavailable", Some(&user.id), "unavailable");
        }
    }
    let replaces = wire::cookie(&headers, SESSION_COOKIE);
    let Some(id) = auth
        .start_session(
            &context,
            LoginMethod::Discord,
            &user.id,
            &user.display(),
            replaces.as_deref(),
        )
        .await
    else {
        return login_error("unavailable");
    };
    landing(
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
    let context = AuditContext::of(&parts);
    let auth = match auth_for_login(&site, &parts.headers) {
        Ok(auth) => auth,
        Err(error) => return error.into_response(),
    };
    let Some((login, name)) = auth.tailscale_identity(&parts) else {
        auth.audit(
            &context,
            AuditEvent::LoginRefused {
                method: "tailscale",
                reason: "no_identity",
                user: None,
            },
        );
        return ApiError::UNAUTHENTICATED.into_response();
    };
    let replaces = wire::cookie(&parts.headers, SESSION_COOKIE);
    match auth
        .start_session(
            &context,
            LoginMethod::Tailscale,
            &login,
            &name,
            replaces.as_deref(),
        )
        .await
    {
        Some(id) => signed_in(
            name,
            LoginMethod::Tailscale,
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
    context: AuditContext,
    headers: HeaderMap,
    body: Result<Json<TokenLogin>, JsonRejection>,
) -> Response {
    let auth = match auth_for_login(&site, &headers) {
        Ok(auth) => auth,
        Err(error) => return error.into_response(),
    };
    // Every attempt counts per client; only wrong tokens count globally, so a
    // flood of guesses from many addresses can never lock out the right token.
    if !auth
        .rate()
        .take_client(Route::TokenLogin, context.client, auth.now())
    {
        return rate_limited(&auth, &context, Route::TokenLogin);
    }
    let Ok(Json(TokenLogin { token })) = body else {
        return ApiError::INVALID_BODY.into_response();
    };
    let Some(fingerprint) = auth.breakglass_matches(token.as_bytes()).map(str::to_owned) else {
        if !auth.rate().take_global(Route::TokenLogin, auth.now()) {
            return rate_limited(&auth, &context, Route::TokenLogin);
        }
        auth.audit(
            &context,
            AuditEvent::LoginRefused {
                method: "token",
                reason: "bad_token",
                user: None,
            },
        );
        return ApiError::UNAUTHENTICATED.into_response();
    };
    drop(token);
    auth.audit(
        &context,
        AuditEvent::BreakGlassUsed {
            via: "login",
            request: "POST /api/admin/auth/token".into(),
        },
    );
    let display = "Break-glass token";
    let replaces = wire::cookie(&headers, SESSION_COOKIE);
    match auth
        .start_session(
            &context,
            LoginMethod::Token,
            &fingerprint,
            display,
            replaces.as_deref(),
        )
        .await
    {
        Some(id) => signed_in(
            display.into(),
            LoginMethod::Token,
            Some(csrf::token(&id)),
            Some(session_cookie(&auth, &id)),
        ),
        None => ApiError::UNAVAILABLE.into_response(),
    }
}

async fn logout(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    session: AdminSession,
) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let (Some(auth), Some(id)) = (site.auth.as_ref(), session.session_id()) {
        let _ = auth
            .sessions()
            .delete_session(&crypto::sha256_hex(id.as_bytes()))
            .await;
        auth.audit(
            &context,
            AuditEvent::SessionEnded {
                actor: session.actor.id().to_owned(),
                reason: "logout",
            },
        );
        response.headers_mut().append(
            SET_COOKIE,
            wire::clear_cookie(SESSION_COOKIE, SESSION_SAME_SITE),
        );
    }
    response
}
