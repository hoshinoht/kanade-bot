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

/// Enforced on both origins, Trusted Types included: `kanade-sw` (the service
/// worker registration, `@kanade/ui` `registerServiceWorker`) is the only policy
/// the apps create, so any other policy or raw HTML/script sink is refused.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
media-src 'self'; font-src 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'; require-trusted-types-for 'script'; \
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

pub fn cache_policy(path: &str, success: bool, origin: Origin) -> &'static str {
    // An explicit max-age would let caches keep a 404/503 (missing chunk, closed art).
    if !success {
        "no-store"
    } else if portrait(path) {
        // Per-user and behind the session: the browser alone keeps it, and
        // revalidates each use, so a signed-out tab cannot replay it.
        "private, no-cache"
    } else if origin == Origin::Public && path.starts_with("/art/") {
        // Behind the member session but the same for every member and not
        // sensitive (user 2026-10-10): the browser keeps it a day so sign-ins
        // and reloads stop refetching; shared caches (Cloudflare) never do.
        "private, max-age=86400"
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
    set(headers, "x-content-type-options", "nosniff");
    set(headers, "referrer-policy", "no-referrer");
    set(headers, "cross-origin-opener-policy", "same-origin");
    set(headers, "cross-origin-resource-policy", "same-origin");
    set(
        headers,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    );
    // Event streams (admin and member hints): never stored, and never
    // compressed or buffered by a proxy (Cloudflare), which would hold the
    // frames back.
    let stream = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let policy = if stream && success {
        "no-store, no-transform"
    } else {
        cache_policy(&path, success, site.origin)
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    if site.origin == Origin::Public {
        set(headers, "strict-transport-security", HSTS);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{CSP, cache_policy};
    use crate::api::listeners::Origin;

    #[test]
    fn trusted_types_are_enforced_with_only_the_service_worker_policy() {
        assert!(
            CSP.ends_with("; require-trusted-types-for 'script'; trusted-types kanade-sw"),
            "{CSP}"
        );
        assert_eq!(CSP.matches("trusted-types").count(), 2, "{CSP}");
    }

    fn admin(path: &str, success: bool) -> &'static str {
        cache_policy(path, success, Origin::Admin)
    }

    #[test]
    fn hashed_assets_are_immutable_entrypoints_revalidate_and_errors_are_never_stored() {
        assert_eq!(
            admin("/assets/index-abc123.js", true),
            "public, max-age=31536000, immutable"
        );
        for path in ["/", "/index.html", "/sw.js", "/manifest.webmanifest"] {
            assert_eq!(admin(path, true), "no-cache", "{path}");
        }
        assert_eq!(admin("/api/identity", true), "no-store");
        assert_eq!(admin("/art/entry/carling", true), "public, max-age=3600");
        assert_eq!(
            admin("/identity/avatar", true),
            "public, max-age=86400, must-revalidate"
        );
        assert_eq!(
            admin("/api/admin/members/1003/avatar", true),
            "private, no-cache"
        );
        assert_eq!(admin("/api/admin/me/avatar", true), "private, no-cache");
        assert_eq!(
            admin("/api/public/session/avatar", true),
            "private, no-cache"
        );
        assert_eq!(admin("/api/public/session", true), "no-store");
        assert_eq!(admin("/api/admin/members/1003/avatar", false), "no-store");
        assert_eq!(admin("/api/admin/members/1003", true), "no-store");
        assert_eq!(admin("/api/admin/members/a/b/avatar", true), "no-store");
        assert_eq!(admin("/assets/missing.js", false), "no-store");
        assert_eq!(admin("/art/entry/carling", false), "no-store");
    }

    #[test]
    fn member_art_stays_in_the_browser_only() {
        assert_eq!(
            cache_policy("/art/entry/carling", true, Origin::Public),
            "private, max-age=86400"
        );
        assert_eq!(
            cache_policy("/art/entry/carling", false, Origin::Public),
            "no-store"
        );
    }
}
