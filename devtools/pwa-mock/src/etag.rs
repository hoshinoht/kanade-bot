//! The server's conditional reads (`src/api/admin/etag.rs`): an `ETag` on each
//! successful JSON admin `GET`, `304` with the same headers and no body when
//! `If-None-Match` names it. The mock hashes with std's SipHash (no crypto
//! dependency); the server's tag is a SHA-256 prefix. Clients only compare.

use std::hash::{DefaultHasher, Hash, Hasher};

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};

pub async fn revalidate(request: Request, next: Next) -> Response {
    let conditional =
        request.method() == Method::GET && !request.uri().path().starts_with("/api/admin/auth/");
    let asked = request.headers().get(header::IF_NONE_MATCH).cloned();
    let response = next.run(request).await;
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if !conditional || response.status() != StatusCode::OK || !json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, usize::MAX).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let tag = format!("\"{:016x}\"", hasher.finish());
    let matched = asked
        .as_ref()
        .and_then(|asked| asked.to_str().ok())
        .is_some_and(|asked| {
            asked.split(',').map(str::trim).any(|candidate| {
                candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == tag
            })
        });
    if let Ok(value) = HeaderValue::from_str(&tag) {
        parts.headers.insert(header::ETAG, value);
    }
    if matched {
        parts.status = StatusCode::NOT_MODIFIED;
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove(header::CONTENT_TYPE);
        return Response::from_parts(parts, Body::empty());
    }
    Response::from_parts(parts, Body::from(bytes))
}
