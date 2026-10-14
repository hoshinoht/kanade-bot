//! Conditional admin reads: a strong `ETag` over each successful JSON `GET`
//! body, and `304 Not Modified` (no body, every other header kept, so the
//! cache and security headers match the `200`) when `If-None-Match` names it.
//! Live hints make pages re-read often; most of those reads change nothing.

use axum::{
    body::{Body, HttpBody, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::api::{auth::crypto::sha256_hex, error::ApiError};

/// Admin JSON reads are small; a larger (or unsized) body is passed through
/// untagged, never buffered.
const MAX_TAGGED: u64 = 4 * 1024 * 1024;

pub async fn revalidate(request: Request, next: Next) -> Response {
    let conditional =
        request.method() == Method::GET && !request.uri().path().starts_with("/api/admin/auth/");
    let asked = request.headers().get(header::IF_NONE_MATCH).cloned();
    let response = next.run(request).await;
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    let sized = response
        .body()
        .size_hint()
        .upper()
        .is_some_and(|upper| upper <= MAX_TAGGED);
    if !conditional || response.status() != StatusCode::OK || !json || !sized {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    // Bounded by the size check above; a body that lies about its size fails here.
    let Ok(bytes) = to_bytes(body, MAX_TAGGED as usize).await else {
        return ApiError::UNAVAILABLE.into_response();
    };
    let tag = format!("\"{}\"", &sha256_hex(&bytes)[..32]);
    let matched = asked.as_ref().is_some_and(|asked| matches(asked, &tag));
    if let Ok(value) = HeaderValue::from_str(&tag) {
        parts.headers.insert(header::ETAG, value);
    }
    if matched {
        parts.status = StatusCode::NOT_MODIFIED;
        strip_body_headers(&mut parts.headers);
        return Response::from_parts(parts, Body::empty());
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// `If-None-Match` uses the weak comparison: `*`, or any listed tag equal
/// once a `W/` prefix is dropped.
fn matches(asked: &HeaderValue, tag: &str) -> bool {
    let Ok(asked) = asked.to_str() else {
        return false;
    };
    asked.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == tag
    })
}

fn strip_body_headers(headers: &mut HeaderMap) {
    headers.remove(header::CONTENT_LENGTH);
    headers.remove(header::CONTENT_TYPE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, middleware::from_fn, routing::get};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One GET over loopback against `router`: (head, body length).
    async fn fetch(router: Router, path: &str) -> (String, usize) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (
            String::from_utf8_lossy(&bytes[..split]).to_ascii_lowercase(),
            bytes.len() - split - 4,
        )
    }

    #[tokio::test]
    async fn an_over_cap_read_passes_through_untagged_and_whole() {
        let big = "x".repeat(MAX_TAGGED as usize + 10);
        let expected = big.len() + 2; // the JSON string's quotes
        // Paths as constants: scripts/api_routes reads `.route("…")` literals as served routes.
        const BIG: &str = "/api/admin/big";
        const SMALL: &str = "/api/admin/small";
        let router = Router::new()
            .route(BIG, get(move || async move { Json(big.clone()) }))
            .route(SMALL, get(|| async { Json("small") }))
            .layer(from_fn(revalidate));
        let (head, length) = fetch(router.clone(), BIG).await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(!head.contains("etag:"), "{head}");
        assert_eq!(length, expected);
        let (head, _) = fetch(router, SMALL).await;
        assert!(
            head.starts_with("http/1.1 200") && head.contains("etag:"),
            "{head}"
        );
    }

    #[test]
    fn if_none_match_lists_and_weak_tags_match() {
        let tag = "\"abc\"";
        for asked in ["\"abc\"", "W/\"abc\"", "\"x\", \"abc\"", "*"] {
            assert!(matches(&HeaderValue::from_static(asked), tag), "{asked}");
        }
        for asked in ["\"abd\"", "abc", ""] {
            assert!(!matches(&HeaderValue::from_static(asked), tag), "{asked}");
        }
    }
}
