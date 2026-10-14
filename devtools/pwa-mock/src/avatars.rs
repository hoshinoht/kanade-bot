//! Member and admin portraits as the server's `/api/admin/members/{id}/avatar`,
//! `/api/admin/me/avatar` and `/api/public/session/avatar` serve them: generated stand-in art (never a
//! real picture) for most seeded members, the server's initial-letter
//! monogram for the rest and for non-Discord sessions.

use crate::App;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

/// Seeded members shown without an avatar (the monogram).
const WITHOUT: [&str; 3] = ["1005", "1009", "1014"];

fn hue(id: &str) -> u32 {
    id.bytes().fold(7u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    }) % 360
}

/// A head-and-shoulders silhouette on a two-tone gradient.
fn art(id: &str) -> String {
    let (a, b) = (hue(id), (hue(id) + 47) % 360);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl({a} 62% 64%)"/><stop offset="1" stop-color="hsl({b} 58% 40%)"/></linearGradient></defs><rect width="64" height="64" fill="url(#g)"/><circle cx="32" cy="25" r="11" fill="#fff" fill-opacity=".82"/><path d="M11 64c2-13 11-21 21-21s19 8 21 21z" fill="#fff" fill-opacity=".82"/></svg>"##
    )
}

/// The server's monogram (`api::assets::monogram`).
fn monogram(name: &str) -> String {
    let initial: String = name
        .trim()
        .chars()
        .next()
        .unwrap_or('K')
        .to_uppercase()
        .collect();
    let initial = initial
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#5f6579"/><text x="32" y="44" text-anchor="middle" font-family="Georgia, serif" font-weight="700" font-size="34" fill="#fbf6e8">{initial}</text></svg>"##
    )
}

fn svg(body: String, request: &HeaderMap) -> Response {
    let etag = format!(
        "\"{:x}\"",
        body.bytes().fold(0u64, |acc, b| acc
            .wrapping_mul(131)
            .wrapping_add(u64::from(b)))
    );
    if request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == etag))
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml".to_owned()),
            (header::ETAG, etag),
        ],
        body,
    )
        .into_response()
}

fn portrait(id: &str, name: &str, request: &HeaderMap) -> Response {
    if WITHOUT.contains(&id) {
        svg(monogram(name), request)
    } else {
        svg(art(id), request)
    }
}

pub async fn member(
    State(app): State<App>,
    Path(id): Path<String>,
    request: HeaderMap,
) -> Response {
    let name = app.store.lock().await.member_display(&id);
    match name {
        Some(name) => portrait(&id, &name, &request),
        None => crate::api::not_found().await,
    }
}

pub async fn me(State(app): State<App>, request: HeaderMap) -> Response {
    let store = app.store.lock().await;
    match store.discord_user() {
        Some(id) => portrait(id, store.session_display(), &request),
        None => svg(monogram(store.session_display()), &request),
    }
}

/// `GET /api/public/session/avatar` for the mock's one member (the caller
/// checks the session first).
pub fn public_member(request: &HeaderMap) -> Response {
    use crate::mock::portal::{MEMBER_ID, MEMBER_NAME};
    portrait(MEMBER_ID, MEMBER_NAME, request)
}
