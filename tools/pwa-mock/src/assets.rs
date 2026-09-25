//! Boss art and the bot's identity art, served same-origin (img-src 'self').
//! Both are deployment-private files; nothing here is ever committed.

use crate::{
    App,
    mock::catalog::{Kind, SUFFIXES, content_type},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone)]
pub struct IdentityConfig {
    pub name: String,
    /// Directory holding `avatar.*` and `banner.*` (v4 caches them next to the
    /// database); `None` serves generated stand-ins.
    pub dir: Option<PathBuf>,
}

#[derive(Serialize)]
pub struct Identity {
    name: String,
    avatar: &'static str,
    banner: &'static str,
    cached: bool,
}

fn file(dir: &Option<PathBuf>, stem: &str) -> Option<PathBuf> {
    let dir = dir.as_ref()?;
    SUFFIXES
        .iter()
        .chain(["gif"].iter())
        .map(|s| dir.join(format!("{stem}.{s}")))
        .find(|p| p.is_file())
}

async fn send(path: PathBuf) -> Response {
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(&path))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn art(State(app): State<App>, Path((kind, key)): Path<(String, String)>) -> Response {
    // Art is schedule dressing: on the public origin it answers closed too.
    if app.public && !app.store.lock().await.public_portal() {
        return crate::api::closed();
    }
    let Some(kind) = Kind::parse(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let found = app.store.lock().await.catalog().file(kind, &key);
    match found {
        Some(path) => send(path).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn identity(State(app): State<App>) -> Json<Identity> {
    let cfg = &app.identity;
    Json(Identity {
        name: cfg.name.clone(),
        avatar: "/identity/avatar",
        banner: "/identity/banner",
        cached: file(&cfg.dir, "avatar").is_some(),
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn svg(body: String) -> Response {
    ([(header::CONTENT_TYPE, "image/svg+xml")], body).into_response()
}

/// Nothing cached: a monogram on the window-chrome colour, like v4's fallback initial.
pub async fn avatar(State(app): State<App>) -> Response {
    if let Some(path) = file(&app.identity.dir, "avatar") {
        return send(path).await;
    }
    let initial = escape(&app.identity.name.chars().next().unwrap_or('K').to_string());
    svg(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#5f6579"/><text x="32" y="44" text-anchor="middle" font-family="Georgia, serif" font-weight="700" font-size="34" fill="#fbf6e8">{initial}</text></svg>"##
    ))
}

/// Nothing cached: v4 painted an accent wash; this is the same wash with the window dots.
pub async fn banner(State(app): State<App>) -> Response {
    if let Some(path) = file(&app.identity.dir, "banner") {
        return send(path).await;
    }
    svg(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 600 150" preserveAspectRatio="xMidYMid slice"><rect width="600" height="150" fill="#eec75f"/><path d="M0 110 L600 40 L600 150 L0 150 Z" fill="#4d5c9e" opacity=".22"/><path d="M0 150 L600 90 L600 150 Z" fill="#4d5c9e" opacity=".25"/></svg>"##.to_owned())
}
