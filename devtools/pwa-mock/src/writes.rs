//! Admin write guard mirroring the Rust server (API-5): unsafe admin requests
//! need the session's `X-Kanade-CSRF` token, and an `Idempotency-Key` replays
//! the first successful answer instead of applying the change twice.

use crate::App;
use axum::{
    Json,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::Mutex as StdMutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub const CSRF_HEADER: &str = "x-kanade-csrf";
const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// The mock has one signed-in admin; the server scopes keys per actor.
const ACTOR: &str = "admin:mock";
const BODY_LIMIT: usize = 1 << 20;

#[derive(PartialEq)]
struct Fingerprint {
    method: Method,
    path: String,
    body: Bytes,
}

struct Recorded {
    fingerprint: Fingerprint,
    status: StatusCode,
    content_type: Option<HeaderValue>,
    body: Bytes,
}

impl Recorded {
    /// The server answers the current state; the first answer is close enough for a mock.
    fn replay(&self) -> Response {
        let mut response = (self.status, self.body.clone()).into_response();
        if let Some(value) = &self.content_type {
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, value.clone());
        }
        response
    }
}

pub struct Writes {
    csrf: StdMutex<String>,
    replays: Mutex<HashMap<(&'static str, String), Recorded>>,
}

impl Default for Writes {
    fn default() -> Self {
        Self {
            csrf: StdMutex::new(fresh_token()),
            replays: Mutex::default(),
        }
    }
}

/// Unguessability does not matter here; only that a new sign-in changes it.
fn fresh_token() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("mock-{:x}-{n}", nanos ^ u128::from(std::process::id()))
}

impl Writes {
    pub fn token(&self) -> String {
        self.csrf.lock().map(|t| t.clone()).unwrap_or_default()
    }

    /// Stand-in for signing in again: the old token stops working.
    pub fn rotate(&self) {
        if let Ok(mut token) = self.csrf.lock() {
            *token = fresh_token();
        }
    }

    pub async fn forget(&self) {
        self.replays.lock().await.clear();
    }

    /// One matching token, and a browser that says same-origin when it says anything.
    fn allows(&self, headers: &HeaderMap) -> bool {
        let site = headers
            .get("sec-fetch-site")
            .is_none_or(|v| v == "same-origin");
        let mut values = headers.get_all(CSRF_HEADER).iter();
        let token = match (values.next(), values.next()) {
            (Some(value), None) => value.as_bytes() == self.token().as_bytes(),
            _ => false,
        };
        site && token
    }
}

fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

/// The request's key, or why it is refused (`400 invalid_idempotency_key`).
fn key(headers: &HeaderMap) -> Result<Option<String>, &'static str> {
    let mut values = headers.get_all(IDEMPOTENCY_KEY).iter();
    match (values.next(), values.next()) {
        (None, _) => Ok(None),
        (Some(value), None) => value
            .to_str()
            .ok()
            .filter(|key| {
                (1..=128).contains(&key.len())
                    && key.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')
                    })
            })
            .map(|key| Some(key.to_owned()))
            .ok_or("Idempotency-Key is 1-128 letters, digits, '-', '_', '.' or ':'."),
        (Some(_), Some(_)) => Err("Send one Idempotency-Key."),
    }
}

/// Route layer on the admin API.
pub async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    // The e2e reset is mock control, not an admin route.
    let control = path == "/api/admin/reset";
    if !control && !crate::auth::open(&path) && !app.store.lock().await.signed_in() {
        return crate::auth::unauthenticated();
    }
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    // Sign-in POSTs need only the same-origin markers, as on the server.
    if safe || control || crate::auth::open(&path) {
        return next.run(req).await;
    }
    if !app.writes.allows(req.headers()) {
        return refusal(
            StatusCode::FORBIDDEN,
            "csrf",
            "The request did not come from this portal.",
        );
    }
    let key = match key(req.headers()) {
        Ok(Some(key)) => key,
        Ok(None) => return next.run(req).await,
        Err(message) => {
            return refusal(StatusCode::BAD_REQUEST, "invalid_idempotency_key", message);
        }
    };
    let (parts, body) = req.into_parts();
    let Ok(body) = to_bytes(body, BODY_LIMIT).await else {
        return refusal(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "The request body is too large.",
        );
    };
    let fingerprint = Fingerprint {
        method: parts.method.clone(),
        path: parts
            .uri
            .path_and_query()
            .map(|p| p.as_str().to_owned())
            .unwrap_or_default(),
        body: body.clone(),
    };
    // Held across the handler so a concurrent retry waits for the first answer.
    let mut replays = app.writes.replays.lock().await;
    if let Some(seen) = replays.get(&(ACTOR, key.clone())) {
        return if seen.fingerprint == fingerprint {
            seen.replay()
        } else {
            refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "idempotency_mismatch",
                "That Idempotency-Key was already used for a different request.",
            )
        };
    }
    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    // Refusals apply nothing, so a retry runs again (the server records only changes).
    if !response.status().is_success() {
        return response;
    }
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap_or_default();
    replays.insert(
        (ACTOR, key),
        Recorded {
            fingerprint,
            status: parts.status,
            content_type: parts.headers.get(header::CONTENT_TYPE).cloned(),
            body: body.clone(),
        },
    );
    Response::from_parts(parts, Body::from(body))
}

/// `POST /__mock/csrf/rotate`: lets e2e prove the client refreshes a stale token.
pub async fn rotate(State(app): State<App>) -> StatusCode {
    app.writes.rotate();
    StatusCode::NO_CONTENT
}
