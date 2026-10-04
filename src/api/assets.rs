//! Same-origin static serving: the built PWA shell with SPA fallback, boss art
//! and the bot identity. Request paths reach the filesystem only after a strict
//! segment check and a canonical-prefix check against the configured root.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use ring::digest::{Context, SHA256, digest};
use serde::Serialize;

use super::{dto::bosses::is_event, error::ApiError, listeners::Site};

const ART_SUFFIXES: [&str; 4] = ["png", "webp", "jpg", "jpeg"];
/// The `animated` kind only; still kinds never serve video.
const VIDEO_SUFFIXES: [&str; 1] = ["mp4"];
/// Cached identity art, in lookup order (`bot::identity` writes these).
pub const IDENTITY_SUFFIXES: [&str; 5] = ["png", "webp", "jpg", "jpeg", "gif"];

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Identity {
    name: String,
    /// Carries `?v=<version>` so a refreshed image is a new URL.
    avatar: String,
    banner: String,
    cached: bool,
    /// Changes whenever the name or the cached art does.
    version: String,
    /// Admin origin only, once the gateway is `READY`.
    bot_user_id: Option<String>,
}

pub async fn identity(State(site): State<Arc<Site>>) -> Json<Identity> {
    let name = live_name(&site);
    let avatar = identity_file(&site, "avatar");
    let banner = identity_file(&site, "banner");
    let version = version(&name, [avatar.as_deref(), banner.as_deref()]);
    Json(Identity {
        avatar: format!("/identity/avatar?v={version}"),
        banner: format!("/identity/banner?v={version}"),
        cached: avatar.is_some(),
        version,
        name,
        bot_user_id: site
            .state
            .as_ref()
            .and_then(|state| state.channels.bot_user_id()),
    })
}

/// The gateway's display name once `READY`, else the configured name.
fn live_name(site: &Site) -> String {
    site.state
        .as_ref()
        .map(|state| &state.channels)
        .or(site.bot.as_ref())
        .and_then(|bot| bot.bot_name())
        .unwrap_or_else(|| site.identity_name.clone())
}

/// Name plus each cached file's extension, size and mtime: cheap, and an
/// atomic replace always yields a new mtime.
fn version(name: &str, files: [Option<&Path>; 2]) -> String {
    let mut context = Context::new(&SHA256);
    context.update(name.as_bytes());
    for file in files {
        context.update(b"\0");
        let Some((path, meta)) = file.and_then(|path| Some((path, path.metadata().ok()?))) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        context.update(path.extension().unwrap_or_default().as_encoded_bytes());
        context.update(&meta.len().to_le_bytes());
        context.update(&modified.to_le_bytes());
    }
    hex(&context.finish().as_ref()[..6])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

/// Identity images with a content ETag; a matching `If-None-Match` is a 304.
async fn identity_image(site: &Site, stem: &str, request: &HeaderMap) -> Response {
    let (content_type, bytes) = match identity_file(site, stem) {
        Some(path) => match tokio::fs::read(&path).await {
            Ok(bytes) => (content_type(&path), bytes),
            Err(_) => return ApiError::NOT_FOUND.into_response(),
        },
        None if stem == "avatar" => ("image/svg+xml", monogram(&live_name(site)).into_bytes()),
        None => ("image/svg+xml", WASH.as_bytes().to_vec()),
    };
    let etag = format!("\"{}\"", hex(&digest(&SHA256, &bytes).as_ref()[..8]));
    let fresh = request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == etag));
    if fresh {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (header::ETAG, etag),
        ],
        bytes,
    )
        .into_response()
}

/// A monogram on the window-chrome colour, like v4's fallback initial.
fn monogram(name: &str) -> String {
    let initial: String = name
        .trim()
        .chars()
        .next()
        .unwrap_or('K')
        .to_uppercase()
        .collect();
    let initial = escape(&initial);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#5f6579"/><text x="32" y="44" text-anchor="middle" font-family="Georgia, serif" font-weight="700" font-size="34" fill="#fbf6e8">{initial}</text></svg>"##
    )
}

/// v4's accent wash.
const WASH: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 600 150" preserveAspectRatio="xMidYMid slice"><rect width="600" height="150" fill="#eec75f"/><path d="M0 110 L600 40 L600 150 L0 150 Z" fill="#4d5c9e" opacity=".22"/><path d="M0 150 L600 90 L600 150 Z" fill="#4d5c9e" opacity=".25"/></svg>"##;

pub async fn avatar(State(site): State<Arc<Site>>, headers: HeaderMap) -> Response {
    identity_image(&site, "avatar", &headers).await
}

pub async fn banner(State(site): State<Arc<Site>>, headers: HeaderMap) -> Response {
    identity_image(&site, "banner", &headers).await
}

/// `/art/{portraits,icons,entry,animated}/{key}`; absent art is a plain 404 ("absent means absent").
pub async fn art(
    State(site): State<Arc<Site>>,
    path: Result<UrlPath<(String, String)>, PathRejection>,
    request: HeaderMap,
) -> Response {
    let Ok(UrlPath((kind, key))) = path else {
        return ApiError::NOT_FOUND.into_response();
    };
    // With a catalog, only catalog keys (exact case) resolve, through their portrait basename,
    // and keys an event knowledge document declares (exact case), through the key itself.
    let basename = match site.state.as_ref() {
        Some(state) => match state.catalog.boss(&key) {
            Some(boss) => boss.portrait().unwrap_or(boss.short()).to_owned(),
            None if state
                .knowledge_dir
                .as_deref()
                .is_some_and(|dir| is_event(dir, &key)) =>
            {
                key
            }
            None => return ApiError::NOT_FOUND.into_response(),
        },
        None => key,
    };
    match art_file(site.boss_dir.as_deref(), &kind, &basename) {
        Some(path) if kind == "animated" => send_ranged(&path, &request).await,
        Some(path) => send(&path).await,
        None => ApiError::NOT_FOUND.into_response(),
    }
}

/// The art file for `kind` (`portraits`, `icons`, `entry`, `animated`) and a basename, if present.
/// Basenames are plain (mixed case allowed, as catalog keys like `MaleficStar`).
pub fn art_file(root: Option<&Path>, kind: &str, basename: &str) -> Option<PathBuf> {
    let (dir, suffixes): (&str, &[&str]) = match kind {
        "portraits" => ("portraits", &ART_SUFFIXES),
        "icons" => ("portraits/icon", &ART_SUFFIXES),
        "entry" => ("artwork/entry", &ART_SUFFIXES),
        "animated" => ("artwork/animated", &VIDEO_SUFFIXES),
        _ => return None,
    };
    let plain = (1..=64).contains(&basename.len())
        && basename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    let root = root.filter(|_| plain)?;
    suffixes
        .iter()
        .find_map(|suffix| contained(root, &Path::new(dir).join(format!("{basename}.{suffix}"))))
}

/// The byte range a `Range` header asks of a `len`-byte body (RFC 9110 §14).
#[derive(Debug, PartialEq, Eq)]
enum ByteRange {
    /// No usable range: absent, another unit, malformed or multi-range (all ignored, so 200).
    Full,
    /// Inclusive `first..=last`, already clamped to the body.
    Part(u64, u64),
    /// A valid single range wholly past the end (416).
    Unsatisfiable,
}

fn byte_range(header: Option<&str>, len: u64) -> ByteRange {
    let digits = |text: &str| {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    let Some(spec) = header.map(str::trim).and_then(|value| {
        let (unit, spec) = (value.get(..6)?, value.get(6..)?);
        unit.eq_ignore_ascii_case("bytes=").then(|| spec.trim())
    }) else {
        return ByteRange::Full;
    };
    let Some((first, last)) = spec.split_once('-').filter(|_| !spec.contains(',')) else {
        return ByteRange::Full;
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        return match digits(last) {
            None => ByteRange::Full,
            Some(0) => ByteRange::Unsatisfiable,
            Some(_) if len == 0 => ByteRange::Unsatisfiable,
            Some(suffix) => ByteRange::Part(len.saturating_sub(suffix), len - 1),
        };
    }
    let Some(first) = digits(first) else {
        return ByteRange::Full;
    };
    let last = match last {
        "" => u64::MAX,
        text => match digits(text) {
            Some(last) if last >= first => last,
            _ => return ByteRange::Full,
        },
    };
    if first >= len {
        ByteRange::Unsatisfiable
    } else {
        ByteRange::Part(first, last.min(len - 1))
    }
}

/// A strong validator from size and mtime, so `If-Range` can resume a download.
fn file_etag(meta: &std::fs::Metadata) -> String {
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    format!("\"{:x}-{modified:x}\"", meta.len())
}

/// Video with byte ranges: Safari (and iOS PWAs) will not play media without `206`.
async fn send_ranged(path: &Path, request: &HeaderMap) -> Response {
    let (Ok(meta), Ok(bytes)) = (tokio::fs::metadata(path).await, tokio::fs::read(path).await)
    else {
        return ApiError::NOT_FOUND.into_response();
    };
    ranged(content_type(path), file_etag(&meta), bytes, request)
}

fn ranged(
    content_type: &'static str,
    etag: String,
    bytes: Vec<u8>,
    request: &HeaderMap,
) -> Response {
    let text = |name| request.get(name).and_then(|value| value.to_str().ok());
    let fresh = text(header::IF_NONE_MATCH)
        .is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == etag));
    if fresh {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    // An `If-Range` that no longer matches (or is a date: no Last-Modified is sent) asks for the whole file.
    let current = text(header::IF_RANGE).is_none_or(|tag| tag.trim() == etag);
    let len = bytes.len() as u64;
    let range = if current {
        byte_range(text(header::RANGE), len)
    } else {
        ByteRange::Full
    };
    let common = [
        (header::CONTENT_TYPE, content_type.to_owned()),
        (header::ACCEPT_RANGES, "bytes".to_owned()),
        (header::ETAG, etag),
    ];
    match range {
        ByteRange::Full => (StatusCode::OK, common, bytes).into_response(),
        ByteRange::Part(first, last) => (
            StatusCode::PARTIAL_CONTENT,
            common,
            [(header::CONTENT_RANGE, format!("bytes {first}-{last}/{len}"))],
            bytes[first as usize..=last as usize].to_vec(),
        )
            .into_response(),
        ByteRange::Unsatisfiable => (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [
                (header::ACCEPT_RANGES, "bytes".to_owned()),
                (header::CONTENT_RANGE, format!("bytes */{len}")),
            ],
        )
            .into_response(),
    }
}

/// Paths owned by the server (in any case, or after a doubled slash): never the SPA shell.
fn reserved(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["/api", "/art", "/identity", "/healthz"]
        .iter()
        .any(|prefix| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        || path.starts_with("/__")
        || path.starts_with("//")
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
        Some("mp4") => "video/mp4",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteRange, byte_range, relative, reserved};

    #[test]
    fn single_byte_ranges_parse_and_everything_else_is_ignored_or_unsatisfiable() {
        use ByteRange::{Full, Part, Unsatisfiable};
        for (header, expected) in [
            ("bytes=0-9", Part(0, 9)),
            ("bytes=2-", Part(2, 99)),
            ("bytes=90-500", Part(90, 99)),
            ("bytes=-10", Part(90, 99)),
            ("bytes=-500", Part(0, 99)),
            ("Bytes= 5-5 ", Part(5, 5)),
            ("bytes=100-", Unsatisfiable),
            ("bytes=100-200", Unsatisfiable),
            ("bytes=-0", Unsatisfiable),
            ("bytes=0-1,5-6", Full),
            ("bytes=5-3", Full),
            ("bytes=+1-2", Full),
            ("bytes=x-", Full),
            ("bytes=-", Full),
            ("bytes=", Full),
            ("items=0-1", Full),
            ("bytés=0-1", Full),
        ] {
            assert_eq!(byte_range(Some(header), 100), expected, "{header}");
        }
        assert_eq!(byte_range(None, 100), Full);
        assert_eq!(byte_range(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(byte_range(Some("bytes=-1"), 0), Unsatisfiable);
    }

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
