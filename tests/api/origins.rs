//! Host allow-lists, authorization by mounting, `/healthz` locality and the
//! closed public portal.

use std::sync::Arc;

use kanade::api::{auth::crypto::SealedSecret, listeners::Site};

use crate::support::{self, ADMIN_HOST, Fixture, PUBLIC_HOST, get, raw, request};

const EDGE_SECRET: &[u8] = b"edge-secret-shared-with-the-caddy-edge!!";

const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// Every admin path the PWA calls (docs/notes/admin-api.md) plus test/mock hooks.
const ADMIN_PATHS: [&str; 48] = [
    "/api/admin/week?week=next",
    "/api/admin/stats",
    "/api/admin/summary",
    "/api/admin/members",
    "/api/admin/members/m1",
    "/api/admin/members/m1/aliases",
    "/api/admin/members/m1/aliases/ali",
    "/api/admin/channels",
    "/api/admin/roles",
    "/api/admin/session",
    "/api/admin/auth/tonight",
    "/api/admin/runs/r1/move",
    "/api/admin/runs/r1/status",
    "/api/admin/runs/r1/rsvp",
    "/api/admin/runs/r1/participants",
    "/api/admin/runs/r1/reset",
    "/api/admin/runs/r1/ping",
    "/api/admin/fixed",
    "/api/admin/fixed/f1",
    "/api/admin/validate/bosses",
    "/api/admin/bosses",
    "/api/admin/bosses/events",
    "/api/admin/bosses/carling/knowledge",
    "/api/admin/inbox",
    "/api/admin/inbox/past",
    "/api/admin/inbox/i1/approve",
    "/api/admin/extractions",
    "/api/admin/extractions/e1",
    "/api/admin/rescan",
    "/api/admin/rescan/targets",
    "/api/admin/rescan/j1",
    "/api/admin/chat",
    "/api/admin/chat/c1",
    "/api/admin/limits/windows/w1",
    "/api/admin/personas",
    "/api/admin/reminders",
    "/api/admin/reminders/m1/preview",
    "/api/admin/members/1/avatar",
    "/api/admin/config",
    "/api/admin/config/profiles/reload",
    "/api/admin/digest",
    "/api/admin/access/recheck",
    "/api/admin/history/revert",
    "/api/admin/history/1/cherry-pick",
    "/api/admin/requests/q1/approve",
    "/__test/whoami",
    "/__mock/reports",
    "/healthz",
];

#[tokio::test]
async fn every_admin_path_and_method_is_a_generic_404_on_public() {
    let fixture = Fixture::new();
    let public = support::public(&fixture.http()).await;
    for path in ADMIN_PATHS {
        for method in METHODS {
            // Admin credentials change nothing on the public origin.
            let reply = request(
                public,
                method,
                PUBLIC_HOST,
                path,
                &[
                    ("Authorization", "Bearer break-glass"),
                    ("Cookie", "__Host-kanade_admin=abc"),
                    ("Content-Length", "0"),
                ],
            )
            .await;
            assert_eq!(reply.status, 404, "{method} {path}");
            if method != "HEAD" {
                assert_eq!(reply.api_error(), "not_found", "{method} {path}");
            }
        }
    }
}

#[tokio::test]
async fn admin_api_is_not_served_before_auth_exists() {
    let fixture = Fixture::new();
    let admin = support::admin(&fixture.http()).await;
    // Drafts routes are deferred (API-6), so this path stays unmounted.
    for path in ["/api/admin/drafts", "/api/public/status"] {
        let reply = get(admin, ADMIN_HOST, path).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.api_error(), "not_found");
    }
    // Without a configured sign-in, mounted admin routes fail closed rather than disappear.
    for path in ["/api/admin/session", "/api/admin/week"] {
        let reply = get(admin, ADMIN_HOST, path).await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (503, "auth_unavailable".into()),
            "{path}"
        );
    }
    let reply = get(admin, ADMIN_HOST, "/api/admin/auth/discord/start").await;
    assert_eq!(reply.header("location"), Some("/?login_error=unavailable"));
}

#[tokio::test]
async fn unknown_missing_duplicated_or_mismatched_hosts_are_misdirected() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let admin = support::admin(&http).await;
    let public = support::public(&http).await;

    assert_eq!(get(admin, ADMIN_HOST, "/api/identity").await.status, 200);
    assert_eq!(get(public, PUBLIC_HOST, "/api/identity").await.status, 200);
    // Hosts compare case-insensitively.
    assert_eq!(get(admin, "KANADE.test", "/api/identity").await.status, 200);

    for (address, host) in [
        (admin, PUBLIC_HOST),
        (public, ADMIN_HOST),
        (admin, "evil.example"),
        (admin, "kanade.test:8443"),
        (public, "localhost"),
        (admin, "localhost"),
    ] {
        let reply = get(address, host, "/api/identity").await;
        assert_eq!(reply.status, 421, "{host}");
        assert_eq!(reply.api_error(), "misdirected");
    }

    let missing = raw(
        admin,
        b"GET /api/identity HTTP/1.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(missing.status, 421);
    let duplicated = raw(
        admin,
        b"GET /api/identity HTTP/1.1\r\nHost: kanade.test\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(duplicated.status, 421);
    let absolute = raw(
        admin,
        b"GET http://evil.example/api/identity HTTP/1.1\r\nHost: kanade.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(absolute.status, 421);
}

#[tokio::test]
async fn unconfigured_admin_accepts_only_loopback_names() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    http.admin_host = None;
    let admin = support::admin(&http).await;
    for host in ["localhost", "127.0.0.1:8080", "[::1]:4393"] {
        assert_eq!(
            get(admin, host, "/api/identity").await.status,
            200,
            "{host}"
        );
    }
    for host in [ADMIN_HOST, "localhost.evil.example", "127.0.0.2"] {
        assert_eq!(
            get(admin, host, "/api/identity").await.status,
            421,
            "{host}"
        );
    }
}

#[tokio::test]
async fn healthz_answers_only_direct_loopback_clients_of_the_admin_listener() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    let admin = support::admin(&http).await;
    // The local healthcheck sends `Host: localhost` even when a real admin host is configured.
    let reply = get(admin, "localhost", "/healthz").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json()["mode"], "offline");
    assert_eq!(get(admin, ADMIN_HOST, "/healthz").await.status, 200);
    assert_eq!(get(admin, "evil.example", "/healthz").await.status, 421);

    // The local healthcheck works whatever the trusted-proxy setting.
    http.trusted_proxy = Some([127, 0, 0, 1].into());
    let mut site = Site::admin(&http);
    site.edge_secret = Some(Arc::new(SealedSecret::new(EDGE_SECRET).unwrap()));
    let behind_edge = support::spawn(site).await;
    assert_eq!(get(behind_edge, "localhost", "/healthz").await.status, 200);
    // Relayed by the authenticated edge: not local, so not answered.
    let edge = std::str::from_utf8(EDGE_SECRET).unwrap();
    let reply = request(
        behind_edge,
        "GET",
        ADMIN_HOST,
        "/healthz",
        &[
            ("X-Kanade-Edge-Auth", edge),
            ("X-Forwarded-For", "100.64.0.7"),
        ],
    )
    .await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.api_error(), "not_found");

    // A client on the listener's own private address is local too (container healthcheck).
    assert!(
        kanade::api::guard::proxy::Peer {
            addr: Some(([172, 18, 0, 5], 40000).into()),
            trusted: false,
            edge_authenticated: false,
        }
        .is_local(Some([172, 18, 0, 5].into()))
    );
    assert!(
        !kanade::api::guard::proxy::Peer {
            addr: Some(([172, 18, 0, 2], 40000).into()),
            trusted: true,
            edge_authenticated: false,
        }
        .is_local(Some([172, 18, 0, 5].into())),
        "the edge's address is not this host"
    );

    let public = support::public(&fixture.http()).await;
    assert_eq!(get(public, PUBLIC_HOST, "/healthz").await.status, 404);
    assert_eq!(get(public, "localhost", "/healthz").await.status, 421);
}

#[tokio::test]
async fn closed_public_portal_serves_status_identity_and_shell_only() {
    let fixture = Fixture::new();
    let public = support::public(&fixture.http()).await;

    let status = get(public, PUBLIC_HOST, "/api/public/status").await;
    assert_eq!(status.status, 200);
    assert_eq!(status.json(), serde_json::json!({"portal": "closed"}));

    for path in [
        "/api/public/week",
        "/api/public/requests/mine",
        "/art/portraits/Carling",
        "/art/entry/Carling",
        "/art/anything/at/all",
    ] {
        for method in ["GET", "POST"] {
            let reply = request(
                public,
                method,
                PUBLIC_HOST,
                path,
                &[("Content-Length", "0")],
            )
            .await;
            assert_eq!(reply.status, 503, "{method} {path}");
            assert_eq!(reply.api_error(), "closed");
        }
    }

    let identity = get(public, PUBLIC_HOST, "/api/identity").await.json();
    assert!(
        identity["avatar"]
            .as_str()
            .unwrap()
            .starts_with("/identity/avatar?v=")
    );
    assert_eq!(identity["bot_user_id"], serde_json::Value::Null);
    assert_eq!(
        get(public, PUBLIC_HOST, "/").await.text(),
        "<!doctype html>public shell"
    );
}

#[test]
fn public_site_requires_a_public_host() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    http.public_host = None;
    assert!(Site::public(&http).is_none());
}
