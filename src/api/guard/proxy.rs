//! Proxy trust: forwarding and identity headers survive only from the one
//! configured peer of each listener (admin: the edge; public: cloudflared) and
//! only in that peer's own family. Everything else is stripped before routing,
//! so no later handler can read a spoofed client IP or Tailscale login.

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

use crate::api::listeners::{Origin, Site};

/// The TCP peer; missing connect info (in-process calls) counts as untrusted and non-local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub addr: Option<SocketAddr>,
    pub trusted: bool,
}

impl Peer {
    pub fn is_direct_loopback(self) -> bool {
        !self.trusted && self.addr.is_some_and(|addr| addr.ip().is_loopback())
    }
}

/// Best-known client address for rate limiting and audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientIp(pub Option<IpAddr>);

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
    let trusted = addr.is_some_and(|addr| site.trusted_peer == Some(addr.ip()));
    strip(request.headers_mut(), site.origin, trusted);
    let client = if trusted {
        forwarded_client(request.headers(), site.origin)
    } else {
        None
    }
    .or(addr.map(|addr| addr.ip()));
    request.extensions_mut().insert(Peer { addr, trusted });
    request.extensions_mut().insert(ClientIp(client));
    next.run(request).await
}

fn strip(headers: &mut HeaderMap, origin: Origin, trusted: bool) {
    let doomed: Vec<_> = headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            let kept = trusted
                && match origin {
                    Origin::Admin => is_forwarding(name) || is_tailscale(name),
                    Origin::Public => is_forwarding(name) || is_cloudflare(name),
                };
            !kept && (is_forwarding(name) || is_cloudflare(name) || is_tailscale(name))
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
            strip(&mut map, origin, false);
            assert_eq!(names(&map), ["accept"]);
        }
    }

    #[test]
    fn trusted_peers_keep_only_their_own_family() {
        let mut admin = headers();
        strip(&mut admin, Origin::Admin, true);
        assert!(admin.contains_key("tailscale-user-login"));
        assert!(admin.contains_key("x-forwarded-for"));
        assert!(!admin.contains_key("cf-connecting-ip"));

        let mut public = headers();
        strip(&mut public, Origin::Public, true);
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
