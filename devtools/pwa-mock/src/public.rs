//! The public origin's member routes (`docs/notes/member-auth-contract.md`
//! §1–§3) over the mock's sessions (`mock/portal.rs`), without Discord:
//! `start` stands in for Discord's consent page and goes straight to the
//! callback, which signs the one mock member in. The session cookie is
//! `kanade_pub` (the server's `__Host-kanade_pub` needs HTTPS, which the mock
//! does not serve); `POST /__mock/public/*` drives what Discord, the roster
//! or a network change would.

use crate::{
    App,
    api::{closed, error, outcome},
    auth::{landing, query_value, safe_next, see_other},
    mock::portal::{Current, LOGIN_ERRORS},
    writes::CSRF_HEADER,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

pub const COOKIE: &str = "kanade_pub";
/// The server's `Max-Age` is the remaining absolute lifetime (8 h by default).
const MAX_AGE_SECS: u32 = 8 * 3600;

fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE && !value.is_empty()).then(|| value.to_owned())
        })
}

fn set_cookie(response: &mut Response, value: &str, max_age: u32) {
    let line = format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}");
    if let Ok(line) = HeaderValue::from_str(&line) {
        response.headers_mut().append(header::SET_COOKIE, line);
    }
}

fn clear_cookie(mut response: Response) -> Response {
    set_cookie(&mut response, "", 0);
    response
}

/// A rotated session's new cookie and token ride on whatever this request answers.
fn carry(mut response: Response, current: &Current) -> Response {
    if current.rotated {
        set_cookie(&mut response, &current.id, MAX_AGE_SECS);
        if let Ok(token) = HeaderValue::from_str(&current.csrf) {
            response.headers_mut().insert(CSRF_HEADER, token);
        }
    }
    response
}

fn unauthenticated() -> Response {
    clear_cookie(error(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "Sign in to continue.",
    ))
}

/// Same-origin when the browser says, and exactly one matching token.
fn csrf_ok(headers: &HeaderMap, expected: &str) -> bool {
    let site = headers
        .get("sec-fetch-site")
        .is_none_or(|v| v == "same-origin");
    let mut values = headers.get_all(CSRF_HEADER).iter();
    let token = match (values.next(), values.next()) {
        (Some(value), None) => value.as_bytes() == expected.as_bytes(),
        _ => false,
    };
    site && token
}

fn refused_csrf() -> Response {
    error(
        StatusCode::FORBIDDEN,
        "csrf",
        "The request did not come from this portal.",
    )
}

/// `require_session` in the order the contract lists: closed, cookie,
/// rotation, CSRF on unsafe methods, touch. The refusal is the whole answer.
async fn member(app: &App, method: &Method, headers: &HeaderMap) -> Result<Current, Box<Response>> {
    let mut store = app.store.lock().await;
    if !store.public_portal() {
        return Err(Box::new(closed()));
    }
    let refused = || Box::new(unauthenticated());
    let id = cookie(headers).ok_or_else(refused)?;
    let (current, expected) = store.public_sessions().resolve(&id).ok_or_else(refused)?;
    let safe = matches!(*method, Method::GET | Method::HEAD);
    if !safe && !csrf_ok(headers, &expected) {
        return Err(Box::new(carry(refused_csrf(), &current)));
    }
    Ok(current)
}

/// "Firefox · Android" from a User-Agent, as the server labels devices; None when unrecognised.
fn device(headers: &HeaderMap) -> Option<String> {
    let ua = headers.get(header::USER_AGENT)?.to_str().ok()?;
    let browser = [
        ("Edg/", "Edge"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .into_iter()
    .find_map(|(mark, name)| ua.contains(mark).then_some(name))?;
    let system = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Mac OS X", "macOS"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .into_iter()
    .find_map(|(mark, name)| ua.contains(mark).then_some(name))?;
    Some(format!("{browser} · {system}"))
}

/// A successful sign-in: a new session, its cookie and its token.
async fn signed_in(app: &App, headers: &HeaderMap, mut response: Response) -> Response {
    let current = app
        .store
        .lock()
        .await
        .public_sessions()
        .sign_in(device(headers));
    set_cookie(&mut response, &current.id, MAX_AGE_SECS);
    if let Ok(token) = HeaderValue::from_str(&current.csrf) {
        response.headers_mut().insert(CSRF_HEADER, token);
    }
    response
}

/// `GET /api/public/status`: the admin Config switch, read live (D7-B). The
/// mock always has its stand-in for the public Discord keys.
pub async fn status(State(app): State<App>) -> Response {
    let open = app.store.lock().await.public_portal();
    Json(json!({ "portal": if open { "open" } else { "closed" } })).into_response()
}

pub async fn start(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Response {
    let next = safe_next(q.get("next"));
    let mut store = app.store.lock().await;
    if !store.public_portal() {
        return see_other("/?login_error=closed");
    }
    if let Some(code) = store.public_sessions().take_error() {
        // `not_eligible` included: no session row and no session cookie.
        return see_other(&format!("/?login_error={code}"));
    }
    see_other(&format!(
        "/api/public/auth/discord/callback?next={}",
        query_value(&next)
    ))
}

pub async fn callback(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !app.store.lock().await.public_portal() {
        return see_other("/?login_error=closed");
    }
    let next = safe_next(q.get("next"));
    signed_in(&app, &headers, landing(&next)).await
}

/// `POST /api/public/auth/logout`: allowed while closed; a live session needs its token.
pub async fn logout(State(app): State<App>, headers: HeaderMap) -> Response {
    let mut store = app.store.lock().await;
    let sessions = store.public_sessions();
    let live = cookie(&headers).and_then(|id| {
        let expected = sessions.token(&id)?.to_owned();
        Some((id, expected))
    });
    match live {
        Some((_, expected)) if !csrf_ok(&headers, &expected) => return refused_csrf(),
        Some((id, _)) => sessions.end(&id),
        None if headers
            .get("sec-fetch-site")
            .is_some_and(|v| v != "same-origin") =>
        {
            return refused_csrf();
        }
        None => {}
    }
    clear_cookie(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/public/session` with the session's `X-Kanade-CSRF`.
pub async fn session(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let body = app
        .store
        .lock()
        .await
        .public_sessions()
        .session(&current.id);
    let mut response = carry(Json(body).into_response(), &current);
    if let Ok(token) = HeaderValue::from_str(&current.csrf) {
        response.headers_mut().insert(CSRF_HEADER, token);
    }
    response
}

pub async fn avatar(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    match member(&app, &method, &headers).await {
        Ok(current) => carry(crate::avatars::public_member(&headers), &current),
        Err(refused) => *refused,
    }
}

pub async fn sessions(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let body = app.store.lock().await.public_sessions().list(&current.id);
    carry(Json(body).into_response(), &current)
}

pub async fn end_one(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let ended = app
        .store
        .lock()
        .await
        .public_sessions()
        .end_one(&current.id, &handle);
    let response = match ended {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(refused) => outcome::<()>(Err(refused)),
    };
    carry(response, &current)
}

/// Sign out everywhere: every session of the member, this one too.
pub async fn end_all(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    if let Err(refused) = member(&app, &method, &headers).await {
        return *refused;
    }
    let ended = app.store.lock().await.public_sessions().end_all();
    clear_cookie(Json(json!({ "ended": ended })).into_response())
}

/// `GET /api/public/week?week=`: absent or `this` is this boss week, `next`
/// the next; anything else `422 invalid_query`, as the admin week.
pub async fn week(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let next = match q.get("week").map(String::as_str) {
        None | Some("this") => false,
        Some("next") => true,
        Some(_) => {
            let refused = error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_query",
                "A query parameter is not valid.",
            );
            return carry(refused, &current);
        }
    };
    let body = app.store.lock().await.member_week(next);
    carry(Json(body).into_response(), &current)
}

/// `GET /api/public/me/allowance`: the caller's own chat allowance only.
pub async fn allowance(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let body = app.store.lock().await.member_allowance();
    carry(Json(body).into_response(), &current)
}

/// `GET /art/{kind}/{key}` on the public origin: the admin listener's art,
/// for a signed-in member while the portal is open (`closed` while closed).
pub async fn art(app: &App, kind: &str, key: &str, headers: &HeaderMap) -> Response {
    match member(app, &Method::GET, headers).await {
        Ok(current) => carry(
            crate::assets::serve_art(app, kind, key, headers).await,
            &current,
        ),
        Err(refused) => *refused,
    }
}

/// Every other `/api/public/` path: `closed` while closed, else not mounted.
pub async fn unmounted(State(app): State<App>) -> Response {
    if app.store.lock().await.public_portal() {
        crate::api::not_found().await
    } else {
        closed()
    }
}

/// `POST /__mock/public/sign-in`: the test shortcut for a Discord sign-in
/// that succeeded (no Discord, no consent page). Refused while closed, as the
/// callback would be.
pub async fn mock_sign_in(State(app): State<App>, headers: HeaderMap) -> Response {
    if !app.store.lock().await.public_portal() {
        return closed();
    }
    signed_in(&app, &headers, StatusCode::NO_CONTENT.into_response()).await
}

#[derive(Deserialize)]
pub struct Outcome {
    error: String,
}

/// `POST /__mock/public/discord {error}`: the next sign-in ends with `/?login_error=<error>`.
pub async fn mock_discord(State(app): State<App>, Json(req): Json<Outcome>) -> Response {
    let Some(code) = LOGIN_ERRORS.iter().find(|c| **c == req.error) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "A login_error code.",
        );
    };
    app.store.lock().await.public_sessions().fail_next(code);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /__mock/public/end`: the member's sessions end on the server's side
/// (expired, or eligibility lost); the next request answers `401`.
pub async fn mock_end(State(app): State<App>) -> StatusCode {
    app.store.lock().await.public_sessions().end_all();
    StatusCode::NO_CONTENT
}

/// `POST /__mock/public/rotate`: every session rotates on its next request,
/// as after a client IP change (D9).
pub async fn mock_rotate(State(app): State<App>) -> StatusCode {
    app.store.lock().await.public_sessions().rotate_all();
    StatusCode::NO_CONTENT
}
