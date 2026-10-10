//! Security and caching headers applied to every response on both origins.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// The production policy under test, verbatim (Trusted Types enforced, only the
/// `kanade-sw` policy allowed), plus `report-uri`. `report-to` is deliberately
/// absent: Chrome ignores `report-uri` whenever `report-to` is present and
/// batches Reporting API deliveries for up to a minute, which would make "zero
/// reports" unobservable in the e2e suite.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
media-src 'self'; font-src 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'; require-trusted-types-for 'script'; \
trusted-types kanade-sw; report-uri /csp-report";

/// The portraits, as the server: per-user, revalidated by ETag.
fn portrait(path: &str) -> bool {
    path == "/api/admin/me/avatar"
        || path == "/api/public/session/avatar"
        || path
            .strip_prefix("/api/admin/members/")
            .and_then(|rest| rest.strip_suffix("/avatar"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// `public`: the member portal's origin, where art sits behind the member
/// session, so (as the server) only the browser keeps it, for a day, and a
/// refusal is never stored.
fn cache_policy(path: &str, public: bool, success: bool) -> &'static str {
    let member_art = public && path.starts_with("/art/");
    if portrait(path) {
        "private, no-cache"
    } else if member_art && success {
        "private, max-age=86400"
    } else if member_art
        || path.starts_with("/api/")
        || path.starts_with("/__mock/")
        || path == "/csp-report"
    {
        "no-store"
    } else if path.starts_with("/art/") || path.starts_with("/identity/") {
        // Unhashed, deployment-replaceable art: short-lived, never immutable.
        "public, max-age=3600"
    } else if path.starts_with("/assets/") {
        // Content-hashed by Vite; a changed file is a new URL.
        "public, max-age=31536000, immutable"
    } else {
        // index.html, offline.html, sw.js, manifest and icons must revalidate so updates land.
        "no-cache"
    }
}

fn set(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(name, HeaderValue::from_static(value));
}

pub async fn apply(request: Request, next: Next) -> Response {
    respond(request, next, false).await
}

/// [`apply`] on the public origin.
pub async fn apply_public(request: Request, next: Next) -> Response {
    respond(request, next, true).await
}

/// What a response's `Cache-Control` is: as the server, event streams
/// (admin and member hints) are never stored nor transformed by a proxy.
fn policy(path: &str, public: bool, success: bool, stream: bool) -> &'static str {
    if stream && success {
        "no-store, no-transform"
    } else {
        cache_policy(path, public, success)
    }
}

async fn respond(request: Request, next: Next, public: bool) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    let success = response.status().is_success();
    let headers = response.headers_mut();
    set(headers, "content-security-policy", CSP);
    set(headers, "x-content-type-options", "nosniff");
    set(headers, "referrer-policy", "no-referrer");
    set(headers, "cross-origin-opener-policy", "same-origin");
    set(headers, "cross-origin-resource-policy", "same-origin");
    set(
        headers,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    );
    let stream = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(policy(&path, public, success, stream)),
    );
    if path.ends_with(".webmanifest") {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/manifest+json"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::cache_policy;

    #[test]
    fn hashed_assets_are_immutable_and_entrypoints_revalidate() {
        assert_eq!(
            cache_policy("/assets/index-abc123.js", false, true),
            "public, max-age=31536000, immutable"
        );
        for path in [
            "/",
            "/index.html",
            "/sw.js",
            "/offline.html",
            "/manifest.webmanifest",
        ] {
            assert_eq!(cache_policy(path, false, true), "no-cache", "{path}");
        }
        assert_eq!(cache_policy("/api/admin/week", false, true), "no-store");
        assert_eq!(
            cache_policy("/art/entry/Carling", false, true),
            "public, max-age=3600"
        );
    }

    #[test]
    fn member_art_is_private_and_refusals_are_never_stored() {
        assert_eq!(
            cache_policy("/art/entry/Carling", true, true),
            "private, max-age=86400"
        );
        assert_eq!(cache_policy("/art/entry/Carling", true, false), "no-store");
        assert_eq!(
            cache_policy("/assets/index-abc123.js", true, true),
            "public, max-age=31536000, immutable"
        );
    }

    #[test]
    fn event_streams_are_never_stored_or_transformed_and_refusals_keep_the_api_rule() {
        for (path, public) in [("/api/admin/events", false), ("/api/public/events", true)] {
            assert_eq!(
                super::policy(path, public, true, true),
                "no-store, no-transform"
            );
            assert_eq!(super::policy(path, public, false, false), "no-store");
        }
    }

    #[test]
    fn the_policy_allows_same_origin_media() {
        assert!(super::CSP.contains("; media-src 'self';"), "{}", super::CSP);
    }
}
