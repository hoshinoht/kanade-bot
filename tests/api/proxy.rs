//! Forwarding, Cloudflare and Tailscale headers reach handlers only from the
//! listener's configured peer; from anyone else they are stripped.

use std::sync::Arc;

use axum::{
    Extension, Json, Router, http::HeaderMap, middleware::from_fn_with_state, routing::get,
};
use kanade::api::{
    guard::proxy::{self, ClientIp, Peer},
    listeners::Site,
};
use serde_json::{Value, json};

use crate::support::{Fixture, request, spawn_router};

const SPOOFED: [(&str, &str); 6] = [
    ("X-Forwarded-For", "203.0.113.7"),
    ("Forwarded", "for=203.0.113.7"),
    ("X-Real-IP", "203.0.113.7"),
    ("CF-Connecting-IP", "203.0.113.8"),
    ("Tailscale-User-Login", "owner@example.com"),
    ("Tailscale-User-Name", "Owner"),
];

async fn echo(
    headers: HeaderMap,
    Extension(peer): Extension<Peer>,
    Extension(ClientIp(client)): Extension<ClientIp>,
) -> Json<Value> {
    let mut names: Vec<_> = headers
        .keys()
        .map(|name| name.as_str().to_owned())
        .collect();
    names.sort();
    Json(json!({
        "headers": names,
        "trusted": peer.trusted,
        "client": client.map(|ip| ip.to_string()),
    }))
}

async fn observe(site: Site) -> Value {
    let router = Router::new()
        .route("/echo", get(echo))
        .layer(from_fn_with_state(Arc::new(site), proxy::sanitize));
    let address = spawn_router(router).await;
    request(address, "GET", "x", "/echo", &SPOOFED).await.json()
}

#[tokio::test]
async fn untrusted_peers_cannot_supply_client_ip_or_identity() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    // A trusted peer that is not this client changes nothing.
    http.trusted_proxy = Some([127, 0, 0, 2].into());
    http.cloudflared_peer = Some([127, 0, 0, 2].into());
    for site in [Site::admin(&http), Site::public(&http).unwrap()] {
        let seen = observe(site).await;
        assert_eq!(seen["trusted"], false);
        assert_eq!(seen["client"], "127.0.0.1");
        assert_eq!(seen["headers"], json!(["connection", "host"]));
    }
}

#[tokio::test]
async fn the_edge_keeps_forwarding_and_tailscale_but_never_cloudflare_headers() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    http.trusted_proxy = Some([127, 0, 0, 1].into());
    let seen = observe(Site::admin(&http)).await;
    assert_eq!(seen["trusted"], true);
    assert_eq!(seen["client"], "203.0.113.7");
    assert_eq!(
        seen["headers"],
        json!([
            "connection",
            "forwarded",
            "host",
            "tailscale-user-login",
            "tailscale-user-name",
            "x-forwarded-for",
            "x-real-ip"
        ])
    );
}

#[tokio::test]
async fn cloudflared_supplies_the_client_ip_but_never_tailscale_identity() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    http.cloudflared_peer = Some([127, 0, 0, 1].into());
    let seen = observe(Site::public(&http).unwrap()).await;
    assert_eq!(seen["trusted"], true);
    assert_eq!(seen["client"], "203.0.113.8");
    assert_eq!(
        seen["headers"],
        json!([
            "cf-connecting-ip",
            "connection",
            "forwarded",
            "host",
            "x-forwarded-for",
            "x-real-ip"
        ])
    );
}
