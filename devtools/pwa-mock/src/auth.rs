//! Sign-in routes as the server's (admin-api "Sign-in and sessions"): the
//! offered methods, the Discord browser flow (start → callback → a landing
//! page that refreshes to `next`, or `/?login_error=<code>`), break-glass
//! token login and sign-out. One mock admin, no cookie: the state is global.

use crate::{App, writes::CSRF_HEADER};
use axum::{
    Json,
    extract::{Query, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

/// The mock's break-glass token (the real one is a deployment secret).
pub const MOCK_TOKEN: &str = "kanade-mock-token";

const LOGIN_ERRORS: [&str; 6] = [
    "state",
    "denied",
    "forbidden",
    "discord",
    "unavailable",
    "rate_limited",
];

fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

/// Routes that answer while signed out.
pub fn open(path: &str) -> bool {
    path.starts_with("/api/admin/auth/") && path != "/api/admin/auth/logout"
}

pub fn unauthenticated() -> Response {
    refusal(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "Sign in to continue.",
    )
}

/// As the server's `safe_next`.
fn safe_next(next: Option<&String>) -> String {
    next.filter(|n| {
        n.starts_with('/')
            && !n.starts_with("//")
            && !n.starts_with("/api/")
            && n.len() <= 512
            && n.bytes().all(|b| b.is_ascii_graphic() && b != b'\\')
    })
    .cloned()
    .unwrap_or_else(|| "/".into())
}

fn see_other(location: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, location.to_owned())],
    )
        .into_response()
}

/// `Session` plus the new CSRF token, as a sign-in answers.
fn signed_in(app: &App, display: &str, method: &str) -> Response {
    (
        [(CSRF_HEADER, app.writes.token())],
        Json(json!({ "display": display, "method": method })),
    )
        .into_response()
}

async fn sign_in(app: &App, method: &str) -> String {
    let mut store = app.store.lock().await;
    store.set_session(method);
    // A new session, a new CSRF token.
    app.writes.rotate();
    store.session_display().to_owned()
}

pub async fn methods() -> Response {
    Json(json!({ "discord": true, "tailscale": false, "token": true })).into_response()
}

/// Signed out like `methods`: `auth::open` covers every `/api/admin/auth/` path.
pub async fn tonight(State(app): State<App>) -> Response {
    Json(app.store.lock().await.tonight()).into_response()
}

pub async fn discord_start(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let next = safe_next(q.get("next"));
    if let Some(code) = app.store.lock().await.take_discord_error() {
        return see_other(&format!("/?login_error={code}"));
    }
    // Stands in for Discord's consent page and its redirect back.
    let next = next
        .replace('%', "%25")
        .replace('&', "%26")
        .replace('#', "%23");
    see_other(&format!("/api/admin/auth/discord/callback?next={next}"))
}

/// The landing page refreshes to `next` from this origin, so the first load
/// already carries a `SameSite=Strict` cookie (on the server; no script here).
pub async fn discord_callback(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let next = safe_next(q.get("next"));
    sign_in(&app, "discord").await;
    let next = next
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">\
             <meta http-equiv=\"refresh\" content=\"0; url={next}\"><title>Signed in</title></head>\
             <body><p><a href=\"{next}\">Continue to Kanade</a></p></body></html>"
        ),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenLogin {
    token: String,
}

pub async fn token_login(
    State(app): State<App>,
    body: Result<Json<TokenLogin>, JsonRejection>,
) -> Response {
    let Ok(Json(TokenLogin { token })) = body else {
        return refusal(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "The request body is not valid.",
        );
    };
    if token != MOCK_TOKEN {
        return unauthenticated();
    }
    let display = sign_in(&app, "token").await;
    signed_in(&app, &display, "token")
}

pub async fn tailscale_login() -> Response {
    // No tailnet edge in front of the mock: never an allow-listed identity.
    unauthenticated()
}

pub async fn logout(State(app): State<App>) -> StatusCode {
    app.store.lock().await.set_session("none");
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
pub struct DiscordOutcome {
    error: String,
}

/// `POST /__mock/discord {error}`: the next Discord sign-in fails with `error`.
pub async fn fail_next_discord(
    State(app): State<App>,
    Json(req): Json<DiscordOutcome>,
) -> Response {
    let Some(code) = LOGIN_ERRORS.iter().find(|c| **c == req.error) else {
        return refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "A login_error code.",
        );
    };
    app.store.lock().await.fail_next_discord(code);
    StatusCode::NO_CONTENT.into_response()
}
