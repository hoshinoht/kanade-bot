//! Same-origin static serving: the built PWA shell with SPA fallback, boss art
//! and the bot identity. Request paths reach the filesystem only after a strict
//! segment check and a canonical-prefix check against the configured root.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::{Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use super::{error::ApiError, listeners::Site};

const ART_SUFFIXES: [&str; 4] = ["png", "webp", "jpg", "jpeg"];
const IDENTITY_SUFFIXES: [&str; 5] = ["png", "webp", "jpg", "jpeg", "gif"];

#[derive(Serialize)]
pub struct Identity {
    name: String,
    avatar: &'static str,
    banner: &'static str,
    cached: bool,
}

pub async fn identity(State(site): State<Arc<Site>>) -> Json<Identity> {
    Json(Identity {
        name: site.identity_name.clone(),
        avatar: "/identity/avatar",
        banner: "/identity/banner",
        cached: identity_file(&site, "avatar").is_some(),
    })
}

fn identity_file(site: &Site, stem: &str) -> Option<PathBuf> {
    let dir = site.identity_dir.as_ref()?;
    IDENTITY_SUFFIXES
        .iter()
        .map(|suffix| dir.join(format!("{stem}.{suffix}")))
        .find(|path| path.is_file())
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
pub async fn avatar(State(site): State<Arc<Site>>) -> Response {
    if let Some(path) = identity_file(&site, "avatar") {
        return send(&path).await;
    }
    let initial = escape(&site.identity_name.chars().next().unwrap_or('K').to_string());
    svg(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#5f6579"/><text x="32" y="44" text-anchor="middle" font-family="Georgia, serif" font-weight="700" font-size="34" fill="#fbf6e8">{initial}</text></svg>"##
    ))
}

/// Nothing cached: v4's accent wash.
pub async fn banner(State(site): State<Arc<Site>>) -> Response {
    if let Some(path) = identity_file(&site, "banner") {
        return send(&path).await;
    }
    svg(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 600 150" preserveAspectRatio="xMidYMid slice"><rect width="600" height="150" fill="#eec75f"/><path d="M0 110 L600 40 L600 150 L0 150 Z" fill="#4d5c9e" opacity=".22"/><path d="M0 150 L600 90 L600 150 Z" fill="#4d5c9e" opacity=".25"/></svg>"##.to_owned())
}

/// `/art/{portraits,icons,entry}/{key}`; absent art is a plain 404 ("absent means absent").
pub async fn art(
    State(site): State<Arc<Site>>,
    path: Result<UrlPath<(String, String)>, PathRejection>,
) -> Response {
    let Ok(UrlPath((kind, key))) = path else {
        return ApiError::NOT_FOUND.into_response();
    };
    let dir = match kind.as_str() {
        "portraits" => "portraits",
        "icons" => "portraits/icon",
        "entry" => "artwork/entry",
        _ => return ApiError::NOT_FOUND.into_response(),
    };
    // Catalog-key shape until the catalog is wired into the API state.
    let key_ok = (1..=64).contains(&key.len())
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        });
    let Some(root) = site.boss_dir.as_ref().filter(|_| key_ok) else {
        return ApiError::NOT_FOUND.into_response();
    };
    let found = ART_SUFFIXES
        .iter()
        .map(|suffix| root.join(dir).join(format!("{key}.{suffix}")))
        .find(|path| path.is_file());
    match found {
        Some(path) => send(&path).await,
        None => ApiError::NOT_FOUND.into_response(),
    }
}

/// Paths owned by the server: never answered with the SPA shell.
fn reserved(path: &str) -> bool {
    ["/api", "/art", "/identity", "/healthz"]
        .iter()
        .any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        || path.starts_with("/__")
}

pub async fn fallback(State(site): State<Arc<Site>>, method: Method, uri: Uri) -> Response {
    let path = uri.path();
    if reserved(path) {
        return ApiError::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return ApiError::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(root) = site.app_dir.as_ref() else {
        return ApiError::NOT_FOUND.into_response();
    };
    let last = path.rsplit('/').next().unwrap_or_default();
    // Extensionless paths are client routes; a missing asset stays a 404, never HTML.
    if !last.contains('.') {
        return match contained(root, Path::new("index.html")) {
            Some(file) => send(&file).await,
            None => ApiError::NOT_FOUND.into_response(),
        };
    }
    match relative(path).and_then(|relative| contained(root, &relative)) {
        Some(file) => send(&file).await,
        None => ApiError::NOT_FOUND.into_response(),
    }
}

/// Plain segments only: no traversal, dotfiles, encodings, backslashes or empty parts.
fn relative(path: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for segment in path.strip_prefix('/')?.split('/') {
        let plain = !segment.is_empty()
            && !segment.starts_with('.')
            && segment.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'-' | b'_' | b'~' | b'@' | b'+')
            });
        if !plain {
            return None;
        }
        relative.push(segment);
    }
    Some(relative)
}

/// The canonical file, only if it is a regular file inside the canonical root (no symlink escape).
fn contained(root: &Path, relative: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let file = root.join(relative).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

async fn send(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type(path))],
            bytes,
        )
            .into_response(),
        Err(_) => ApiError::NOT_FOUND.into_response(),
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{relative, reserved};

    #[test]
    fn only_plain_segments_reach_the_filesystem() {
        assert!(relative("/assets/index-Ab_1.js").is_some());
        for path in [
            "/../Cargo.toml",
            "/assets/../../secret.js",
            "/%2e%2e/secret.txt",
            "/assets/..%2fsecret.js",
            "/.env.txt",
            "/assets//x.js",
            "/a\\b.js",
        ] {
            assert!(relative(path).is_none(), "{path}");
        }
    }

    #[test]
    fn server_paths_never_fall_back_to_the_shell() {
        for path in [
            "/api",
            "/api/admin/week",
            "/art/x",
            "/identity/x",
            "/healthz",
            "/__test/reset",
        ] {
            assert!(reserved(path), "{path}");
        }
        for path in ["/", "/week", "/apis", "/artwork", "/runs/abc"] {
            assert!(!reserved(path), "{path}");
        }
    }
}
