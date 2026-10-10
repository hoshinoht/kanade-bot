//! Exact response header sets per origin, no CORS, and request bounds.

use std::{sync::Arc, time::Duration};

use axum::{Router, middleware::from_fn_with_state, routing};
use kanade::api::{
    guard::{headers, limits},
    listeners::Site,
};

use crate::support::{self, ADMIN_HOST, Fixture, PUBLIC_HOST, get, raw, request, spawn_router};

const TRANSPORT: [&str; 4] = ["connection", "content-length", "content-type", "date"];

fn expected(public: bool) -> Vec<(&'static str, &'static str)> {
    let mut headers = vec![
        ("content-security-policy", headers::CSP),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ];
    if public {
        headers.push(("strict-transport-security", headers::HSTS));
    }
    headers
}

fn assert_exact(reply: &support::Reply, public: bool, cache: &str) {
    assert_with(reply, public, cache, &[]);
}

/// Static app files of a compressible type also vary on `Accept-Encoding`.
fn assert_static(reply: &support::Reply, public: bool, cache: &str) {
    assert_with(reply, public, cache, &[("vary", "Accept-Encoding")]);
}

fn assert_with(reply: &support::Reply, public: bool, cache: &str, extra: &[(&str, &str)]) {
    let mut expected = expected(public);
    expected.push(("cache-control", cache));
    expected.extend_from_slice(extra);
    for (name, value) in &expected {
        assert_eq!(reply.header(name), Some(*value), "{name}");
    }
    let mut names: Vec<_> = reply
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    let mut allowed: Vec<_> = expected
        .iter()
        .map(|(name, _)| *name)
        .chain(TRANSPORT)
        .collect();
    allowed.sort_unstable();
    assert_eq!(names, allowed);
}

#[tokio::test]
async fn each_origin_sends_exactly_the_mock_security_headers() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let admin = support::admin(&http).await;
    let public = support::public(&http).await;

    assert_exact(
        &get(admin, ADMIN_HOST, "/api/identity").await,
        false,
        "no-store",
    );
    assert_static(&get(admin, ADMIN_HOST, "/").await, false, "no-cache");
    assert_static(
        &get(admin, ADMIN_HOST, "/assets/app-abc123.js").await,
        false,
        "public, max-age=31536000, immutable",
    );
    assert_exact(
        &get(admin, ADMIN_HOST, "/art/portraits/Carling").await,
        false,
        "public, max-age=3600",
    );
    // Refusals carry the same set and are never cacheable.
    assert_exact(&get(admin, "evil.example", "/").await, false, "no-store");
    assert_exact(
        &get(admin, ADMIN_HOST, "/assets/missing.js").await,
        false,
        "no-store",
    );

    assert_exact(
        &get(public, PUBLIC_HOST, "/api/public/status").await,
        true,
        "no-store",
    );
    // The public shell is filled per request and never negotiated.
    assert_exact(&get(public, PUBLIC_HOST, "/week").await, true, "no-cache");
    assert_exact(
        &get(public, PUBLIC_HOST, "/art/entry/Carling").await,
        true,
        "no-store",
    );
    assert_exact(&get(public, "evil.example", "/").await, true, "no-store");
}

#[tokio::test]
async fn no_cors_headers_even_for_preflights() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let admin = support::admin(&http).await;
    let public = support::public(&http).await;
    for (address, host, path) in [
        (admin, ADMIN_HOST, "/api/identity"),
        (admin, ADMIN_HOST, "/api/admin/week"),
        (public, PUBLIC_HOST, "/api/public/status"),
        (public, PUBLIC_HOST, "/"),
    ] {
        for method in ["OPTIONS", "GET"] {
            let reply = request(
                address,
                method,
                host,
                path,
                &[
                    ("Origin", "https://evil.example"),
                    ("Access-Control-Request-Method", "POST"),
                ],
            )
            .await;
            assert!(
                !reply
                    .headers
                    .iter()
                    .any(|(name, _)| name.starts_with("access-control-")),
                "{method} {path}"
            );
            if method == "OPTIONS" {
                assert!(matches!(reply.status, 404 | 405), "{path}");
            }
        }
    }
}

#[tokio::test]
async fn oversized_bodies_are_refused_before_any_handler() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let admin = support::admin(&http).await;
    let public = support::public(&http).await;
    let too_large = (limits::MAX_BODY_BYTES + 1).to_string();
    for (address, host, path) in [
        (admin, ADMIN_HOST, "/api/identity"),
        (admin, ADMIN_HOST, "/api/admin/config"),
        (public, PUBLIC_HOST, "/api/public/requests"),
    ] {
        let reply = request(
            address,
            "POST",
            host,
            path,
            &[("Content-Length", &too_large)],
        )
        .await;
        assert_eq!(reply.status, 413, "{path}");
        assert_eq!(reply.api_error(), "payload_too_large");
    }
    // Within bounds, routing decides as usual.
    let reply = request(
        admin,
        "POST",
        ADMIN_HOST,
        "/api/identity",
        &[("Content-Length", "0")],
    )
    .await;
    assert_eq!(reply.status, 405);
    assert_eq!(reply.api_error(), "method_not_allowed");
}

#[tokio::test]
async fn slow_handlers_time_out_with_a_generic_error() {
    let fixture = Fixture::new();
    let mut site = Site::admin(&fixture.http());
    site.limits.request_timeout = Duration::from_millis(50);
    let router = Router::new()
        .route(
            "/slow",
            routing::get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .layer(from_fn_with_state(Arc::new(site), limits::enforce));
    let address = spawn_router(router).await;
    let reply = raw(
        address,
        b"GET /slow HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(reply.status, 503);
    assert_eq!(reply.api_error(), "timeout");
}

#[test]
fn the_policy_allows_same_origin_media() {
    assert!(
        headers::CSP.contains("; media-src 'self';"),
        "{}",
        headers::CSP
    );
}
