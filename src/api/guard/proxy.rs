//! Proxy trust: forwarding and identity headers survive only from the one
//! configured peer of each listener (admin: the edge; public: cloudflared) and
//! only in that peer's own family. When an edge secret is configured, the
//! admin edge must also present it in [`EDGE_AUTH`]; Tailscale identity
//! headers survive only then, so another process that shares the edge's
//! address cannot forge a login. Everything else, and the secret header
//! itself, is stripped before routing.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};

use axum::response::IntoResponse;

use crate::api::{
    auth::crypto,
    error::ApiError,
    listeners::{Origin, Site},
};

/// Header carrying the edge's shared secret; never reaches a handler.
pub const EDGE_AUTH: &str = "x-kanade-edge-auth";

/// The TCP peer; missing connect info (in-process calls) counts as untrusted and non-local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub addr: Option<SocketAddr>,
    /// Its forwarding headers were kept.
    pub trusted: bool,
    /// The configured edge proved itself with the edge secret: identity headers were kept.
    pub edge_authenticated: bool,
}

impl Peer {
    /// A client on this host (loopback, or the listener's own private address)
    /// that is not the authenticated edge relaying someone else.
    pub fn is_local(self, listener_ip: Option<IpAddr>) -> bool {
        !self.edge_authenticated
            && self
                .addr
                .is_some_and(|addr| addr.ip().is_loopback() || Some(addr.ip()) == listener_ip)
    }
}

/// Best-known client address for rate limiting and audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientIp(pub Option<IpAddr>);

/// Random per-request id for audit correlation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestId(pub String);

const CLOUDFLARE_CLIENT: &str = "cf-connecting-ip";
const FORWARDED_FOR: &str = "x-forwarded-for";

fn is_forwarding(name: &str) -> bool {
    name == "forwarded"
        || name == "x-real-ip"
        || name == "x-client-ip"
        || name == "true-client-ip"
        || name.starts_with("x-forwarded-")
}

fn is_cloudflare(name: &str) -> bool {
    name.starts_with("cf-")
}

fn is_tailscale(name: &str) -> bool {
    name.starts_with("tailscale-")
}

pub async fn sanitize(State(site): State<Arc<Site>>, mut request: Request, next: Next) -> Response {
    let addr = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let from_proxy = addr.is_some_and(|addr| site.trusted_peer == Some(addr.ip()));
    let edge_authenticated = from_proxy
        && site.origin == Origin::Admin
        && site
            .edge_secret
            .as_ref()
            .is_some_and(|secret| presents(request.headers(), secret));
    let trusted = if site.origin == Origin::Admin && site.edge_secret.is_some() {
        edge_authenticated
    } else {
        from_proxy
    };
    strip(
        request.headers_mut(),
        site.origin,
        trusted,
        edge_authenticated,
    );
    let forwarded = if trusted {
        forwarded_client(request.headers(), site.origin)
    } else {
        None
    };
    // The authenticated edge must name the client: falling back to its own
    // address would pool every client into one rate-limit bucket.
    if edge_authenticated && forwarded.is_none() {
        return ApiError::BAD_FORWARDING.into_response();
    }
    let client = forwarded.or(addr.map(|addr| addr.ip()));
    let request_id = crypto::random_id().unwrap_or_default();
    request.extensions_mut().insert(Peer {
        addr,
        trusted,
        edge_authenticated,
    });
    request.extensions_mut().insert(ClientIp(client));
    request.extensions_mut().insert(RequestId(request_id));
    next.run(request).await
}

/// Exactly one `EDGE_AUTH` value, compared in constant time.
fn presents(headers: &HeaderMap, secret: &crypto::SealedSecret) -> bool {
    let mut values = headers.get_all(EDGE_AUTH).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => secret.matches(value.as_bytes()),
        _ => false,
    }
}

fn strip(headers: &mut HeaderMap, origin: Origin, trusted: bool, identity: bool) {
    let doomed: Vec<_> = headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            let kept = trusted
                && match origin {
                    Origin::Admin => is_forwarding(name) || (identity && is_tailscale(name)),
                    Origin::Public => is_forwarding(name) || is_cloudflare(name),
                };
            name == EDGE_AUTH
                || (!kept && (is_forwarding(name) || is_cloudflare(name) || is_tailscale(name)))
        })
        .cloned()
        .collect();
    for name in doomed {
        headers.remove(name);
    }
}

fn forwarded_client(headers: &HeaderMap, origin: Origin) -> Option<IpAddr> {
    let value = match origin {
        Origin::Public => headers.get(CLOUDFLARE_CLIENT)?.to_str().ok()?,
        // The edge appends the address it saw; earlier entries are client-supplied.
        Origin::Admin => headers
            .get_all(FORWARDED_FOR)
            .iter()
            .next_back()?
            .to_str()
            .ok()?
            .rsplit(',')
            .next()?,
    };
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        for name in [
            "x-forwarded-for",
            "x-forwarded-host",
            "forwarded",
            "x-real-ip",
            "true-client-ip",
            "cf-connecting-ip",
            "cf-ray",
            "tailscale-user-login",
            "tailscale-user-name",
            "x-kanade-edge-auth",
            "accept",
        ] {
            headers.insert(name, HeaderValue::from_static("203.0.113.9"));
        }
        headers
    }

    fn names(headers: &HeaderMap) -> Vec<&str> {
        let mut names: Vec<_> = headers.keys().map(|name| name.as_str()).collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn untrusted_peers_lose_every_forwarding_and_identity_header() {
        for origin in [Origin::Admin, Origin::Public] {
            let mut map = headers();
            strip(&mut map, origin, false, false);
            assert_eq!(names(&map), ["accept"]);
        }
    }

    #[test]
    fn trusted_peers_keep_only_their_own_family() {
        let mut admin = headers();
        strip(&mut admin, Origin::Admin, true, true);
        assert!(admin.contains_key("tailscale-user-login"));
        assert!(admin.contains_key("x-forwarded-for"));
        assert!(!admin.contains_key("cf-connecting-ip"));
        assert!(
            !admin.contains_key(EDGE_AUTH),
            "the secret never reaches handlers"
        );

        // Forwarding trust without the edge secret carries no identity.
        let mut forwarding = headers();
        strip(&mut forwarding, Origin::Admin, true, false);
        assert!(forwarding.contains_key("x-forwarded-for"));
        assert!(!forwarding.contains_key("tailscale-user-login"));

        let mut public = headers();
        strip(&mut public, Origin::Public, true, false);
        assert!(public.contains_key("cf-connecting-ip"));
        assert!(!public.contains_key("tailscale-user-login"));
        assert!(!public.contains_key("tailscale-user-name"));
    }

    #[test]
    fn edge_client_is_the_last_forwarded_hop() {
        let mut map = HeaderMap::new();
        map.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.1, 100.64.0.7"),
        );
        assert_eq!(
            forwarded_client(&map, Origin::Admin),
            Some("100.64.0.7".parse().unwrap())
        );
        map.insert("cf-connecting-ip", HeaderValue::from_static("not-an-ip"));
        assert_eq!(forwarded_client(&map, Origin::Public), None);
    }
}
