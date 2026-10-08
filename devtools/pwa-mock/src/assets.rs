//! Boss art and the bot's identity art, served same-origin (img-src 'self').
//! Both are deployment-private files; nothing here is ever committed.

use crate::{
    App,
    mock::catalog::{Kind, SUFFIXES, content_type},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::{path::PathBuf, time::UNIX_EPOCH};

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
    /// The bot's Discord user id on the admin origin; null on the public one.
    bot_user_id: Option<&'static str>,
}

/// The mock bot's Discord user id (seeded chat questions mention it).
pub const BOT_USER_ID: &str = "1543532497948909578";

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

pub async fn art(
    State(app): State<App>,
    Path((kind, key)): Path<(String, String)>,
    request: HeaderMap,
) -> Response {
    // As the server's public catch-all (member-auth-contract §1): `closed`
    // while the portal is closed, `404` while it is open, until member reads
    // serve art behind the session.
    if app.public {
        return if app.store.lock().await.public_portal() {
            crate::api::not_found().await
        } else {
            crate::api::closed()
        };
    }
    let Some(kind) = Kind::parse(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let catalog = app.store.lock().await.catalog().clone();
    // Catalog keys, else a key an event knowledge document declares (exact case).
    let found = catalog.file(kind, &key).or_else(|| {
        app.knowledge
            .is_event(&key)
            .then(|| catalog.event_file(kind, &key))
            .flatten()
    });
    match found {
        Some(path) if matches!(kind, Kind::Animated) => send_ranged(path, &request).await,
        Some(path) => send(path).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The byte range a `Range` header asks of a `len`-byte body, as the Rust API parses it.
#[derive(Debug, PartialEq, Eq)]
enum ByteRange {
    /// Absent, another unit, malformed or multi-range: ignored, so 200.
    Full,
    /// Inclusive, clamped to the body.
    Part(u64, u64),
    /// A valid single range wholly past the end (416).
    Unsatisfiable,
}

fn byte_range(header: Option<&str>, len: u64) -> ByteRange {
    let digits = |text: &str| {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
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

/// Video with byte ranges, ETag, `If-Range` and `If-None-Match`, like the Rust API:
/// Safari (and iOS PWAs) will not play media without `206`.
async fn send_ranged(path: PathBuf, request: &HeaderMap) -> Response {
    let (Ok(meta), Ok(bytes)) = (
        tokio::fs::metadata(&path).await,
        tokio::fs::read(&path).await,
    ) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let etag = format!("\"{:x}-{modified:x}\"", meta.len());
    ranged(content_type(&path), etag, bytes, request)
}

fn ranged(
    content_type: &'static str,
    etag: String,
    bytes: Vec<u8>,
    request: &HeaderMap,
) -> Response {
    let text = |name| request.get(name).and_then(|value| value.to_str().ok());
    if text(header::IF_NONE_MATCH).is_some_and(|tags| tags.split(',').any(|t| t.trim() == etag)) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
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

pub async fn identity(State(app): State<App>) -> Json<Identity> {
    let cfg = &app.identity;
    Json(Identity {
        name: cfg.name.clone(),
        avatar: "/identity/avatar",
        banner: "/identity/banner",
        cached: file(&cfg.dir, "avatar").is_some(),
        bot_user_id: (!app.public).then_some(BOT_USER_ID),
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

#[cfg(test)]
mod tests {
    use super::{ByteRange, byte_range, ranged};
    use axum::{
        body::to_bytes,
        http::{HeaderMap, HeaderValue, StatusCode},
    };

    #[test]
    fn single_byte_ranges_parse_and_everything_else_is_ignored_or_unsatisfiable() {
        use ByteRange::{Full, Part, Unsatisfiable};
        for (header, expected) in [
            ("bytes=0-9", Part(0, 9)),
            ("bytes=2-", Part(2, 99)),
            ("bytes=90-500", Part(90, 99)),
            ("bytes=-10", Part(90, 99)),
            ("bytes=100-", Unsatisfiable),
            ("bytes=-0", Unsatisfiable),
            ("bytes=0-1,5-6", Full),
            ("bytes=5-3", Full),
            ("bytes=+1-2", Full),
            ("items=0-1", Full),
        ] {
            assert_eq!(byte_range(Some(header), 100), expected, "{header}");
        }
    }

    async fn reply(range: Option<&str>) -> (StatusCode, Option<String>, Vec<u8>) {
        let mut request = HeaderMap::new();
        if let Some(range) = range {
            request.insert("range", HeaderValue::from_str(range).unwrap());
        }
        let response = ranged(
            "video/mp4",
            "\"t\"".into(),
            b"0123456789".to_vec(),
            &request,
        );
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .map(|v| v.to_str().unwrap().to_owned())
        };
        assert_eq!(header("accept-ranges").as_deref(), Some("bytes"));
        let (status, range) = (response.status(), header("content-range"));
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, range, body.to_vec())
    }

    #[tokio::test]
    async fn animated_art_answers_206_416_and_full() {
        assert_eq!(reply(None).await.2, b"0123456789");
        assert_eq!(
            reply(Some("bytes=2-5")).await,
            (
                StatusCode::PARTIAL_CONTENT,
                Some("bytes 2-5/10".into()),
                b"2345".to_vec()
            )
        );
        assert_eq!(
            reply(Some("bytes=-3")).await,
            (
                StatusCode::PARTIAL_CONTENT,
                Some("bytes 7-9/10".into()),
                b"789".to_vec()
            )
        );
        assert_eq!(
            reply(Some("bytes=10-")).await,
            (
                StatusCode::RANGE_NOT_SATISFIABLE,
                Some("bytes */10".into()),
                Vec::new()
            )
        );
        assert_eq!(reply(Some("bytes=0-1,4-5")).await.0, StatusCode::OK);
    }
}
