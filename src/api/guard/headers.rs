//! Security and caching headers on every response, matching `devtools/pwa-mock`
//! minus its dev-only `report-uri` sink; HSTS only on the public origin.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};

use crate::api::listeners::{Origin, Site};

pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
media-src 'self'; font-src 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

pub const CSP_REPORT_ONLY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
img-src 'self' data:; media-src 'self'; font-src 'self'; connect-src 'self'; manifest-src 'self'; \
worker-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; require-trusted-types-for 'script'; \
trusted-types kanade-sw";

pub const HSTS: &str = "max-age=31536000; includeSubDomains";

/// The portraits: unversioned (or name-versioned) URLs that change with the avatar.
fn portrait(path: &str) -> bool {
    path == "/api/admin/me/avatar"
        || path == "/api/public/session/avatar"
        || path
            .strip_prefix("/api/admin/members/")
            .and_then(|rest| rest.strip_suffix("/avatar"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

pub fn cache_policy(path: &str, success: bool) -> &'static str {
    // An explicit max-age would let caches keep a 404/503 (missing chunk, closed art).
    if !success {
        "no-store"
    } else if portrait(path) {
        // Per-user and behind the session: the browser alone keeps it, and
        // revalidates each use against the content ETag (304 when unchanged).
        "private, no-cache"
    } else if path.starts_with("/api/") || path == "/api" || path == "/healthz" {
        "no-store"
    } else if path.starts_with("/identity/") {
        // v4's policy; `/api/identity` versions the URLs and responses carry an ETag.
        "public, max-age=86400, must-revalidate"
    } else if path.starts_with("/art/") {
        // Unhashed, deployment-replaceable art.
        "public, max-age=3600"
    } else if path.starts_with("/assets/") {
        // Content-hashed by Vite: a changed file is a new URL.
        "public, max-age=31536000, immutable"
    } else {
        // index.html, sw.js, manifest and icons revalidate so updates land.
        "no-cache"
    }
}

fn set(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(name, HeaderValue::from_static(value));
}

pub async fn apply(State(site): State<Arc<Site>>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    let success = response.status().is_success() || response.status().is_redirection();
    let headers = response.headers_mut();
    set(headers, "content-security-policy", CSP);
    set(
        headers,
        "content-security-policy-report-only",
        CSP_REPORT_ONLY,
    );
    set(headers, "x-content-type-options", "nosniff");
    set(headers, "referrer-policy", "no-referrer");
    set(headers, "cross-origin-opener-policy", "same-origin");
    set(headers, "cross-origin-resource-policy", "same-origin");
    set(
        headers,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_policy(&path, success)),
    );
    if site.origin == Origin::Public {
        set(headers, "strict-transport-security", HSTS);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::cache_policy;

    #[test]
    fn hashed_assets_are_immutable_entrypoints_revalidate_and_errors_are_never_stored() {
        assert_eq!(
            cache_policy("/assets/index-abc123.js", true),
            "public, max-age=31536000, immutable"
        );
        for path in ["/", "/index.html", "/sw.js", "/manifest.webmanifest"] {
            assert_eq!(cache_policy(path, true), "no-cache", "{path}");
        }
        assert_eq!(cache_policy("/api/identity", true), "no-store");
        assert_eq!(
            cache_policy("/art/entry/carling", true),
            "public, max-age=3600"
        );
        assert_eq!(
            cache_policy("/identity/avatar", true),
            "public, max-age=86400, must-revalidate"
        );
        assert_eq!(
            cache_policy("/api/admin/members/1003/avatar", true),
            "private, no-cache"
        );
        assert_eq!(
            cache_policy("/api/admin/me/avatar", true),
            "private, no-cache"
        );
        assert_eq!(
            cache_policy("/api/public/session/avatar", true),
            "private, no-cache"
        );
        assert_eq!(cache_policy("/api/public/session", true), "no-store");
        assert_eq!(
            cache_policy("/api/admin/members/1003/avatar", false),
            "no-store"
        );
        assert_eq!(cache_policy("/api/admin/members/1003", true), "no-store");
        assert_eq!(
            cache_policy("/api/admin/members/a/b/avatar", true),
            "no-store"
        );
        assert_eq!(cache_policy("/assets/missing.js", false), "no-store");
        assert_eq!(cache_policy("/art/entry/carling", false), "no-store");
    }
}
