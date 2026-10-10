//! Host allow-list per listener; anything else is 421 before routing.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::header::HOST,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::api::{
    error::ApiError,
    listeners::{HostPolicy, Origin, Site},
};

pub async fn enforce(State(site): State<Arc<Site>>, request: Request, next: Next) -> Response {
    match request_host(&request) {
        Some(host) if accepts(&site, &host, request.uri().path()) => next.run(request).await,
        _ => ApiError::MISDIRECTED.into_response(),
    }
}

/// Exactly one Host header; an absolute-form target must name the same authority.
fn request_host(request: &Request) -> Option<String> {
    let mut values = request.headers().get_all(HOST).iter();
    let header = values.next();
    if values.next().is_some() {
        return None;
    }
    let header = match header {
        Some(value) => Some(value.to_str().ok()?.to_ascii_lowercase()),
        None => None,
    };
    let authority = request
        .uri()
        .authority()
        .map(|authority| authority.as_str().to_ascii_lowercase());
    match (header, authority) {
        (Some(header), Some(authority)) if header != authority => None,
        (Some(host), _) | (None, Some(host)) => Some(host),
        (None, None) => None,
    }
}

fn accepts(site: &Site, host: &str, path: &str) -> bool {
    match &site.hosts {
        HostPolicy::Exact(expected) if expected == host => true,
        HostPolicy::LoopbackNames if is_loopback_name(host) => true,
        // The local healthcheck always says `localhost`; the handler still requires a loopback peer.
        _ => site.origin == Origin::Admin && path == "/healthz" && is_loopback_name(host),
    }
}

fn is_loopback_name(host: &str) -> bool {
    let rest = ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .find_map(|name| host.strip_prefix(name));
    match rest {
        Some("") => true,
        Some(rest) => rest.strip_prefix(':').is_some_and(|port| {
            !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit())
        }),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback_name;

    #[test]
    fn loopback_names_allow_only_a_numeric_port() {
        for host in ["localhost", "localhost:8080", "127.0.0.1:1", "[::1]:4393"] {
            assert!(is_loopback_name(host), "{host}");
        }
        for host in [
            "localhost.evil.example",
            "localhost:",
            "localhost:80x",
            "127.0.0.10",
            "[::1]x",
            "kanade.example",
        ] {
            assert!(!is_loopback_name(host), "{host}");
        }
    }
}
