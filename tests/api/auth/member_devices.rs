//! The member's devices (D5-A) and the wall between the two realms.

use chrono::TimeDelta;
use kanade::api::auth::{
    audit::AuditEvent,
    rate::{Limits, Route, Rule},
    wire,
};

use super::{
    TOKEN,
    member_support::{Browser, MIKAN, MemberHarness, Options, PUB_ORIGIN, cookie, discord_user},
};
use crate::support::{ADMIN_HOST, PUBLIC_HOST, Reply, send};

const IP: &str = "198.51.100.10";

async fn devices(harness: &MemberHarness, browser: &Browser) -> serde_json::Value {
    let reply = harness
        .get(
            "/api/public/sessions",
            &[
                (browser.cookie().0, &browser.cookie().1),
                ("CF-Connecting-IP", IP),
            ],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    reply.json()
}

async fn write(harness: &MemberHarness, browser: &Browser, method: &str, path: &str) -> Reply {
    harness
        .request(
            method,
            path,
            &[
                (browser.cookie().0, &browser.cookie().1),
                PUB_ORIGIN,
                ("X-Kanade-CSRF", &browser.csrf),
                ("CF-Connecting-IP", IP),
            ],
        )
        .await
}

#[tokio::test]
async fn the_device_list_names_sessions_by_handle_and_ends_one() {
    let harness = MemberHarness::new().await;
    let mut plain = discord_user(MIKAN, "Mikan");
    plain.avatar = None;
    let first = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    harness.advance(TimeDelta::minutes(1));
    let second = harness.sign_in(plain).await;

    let list = devices(&harness, &second).await;
    crate::schemas::assert_valid("public.json#/$defs/PublicSessions", "sessions", &list);
    let rows = list["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .map(|row| row["current"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [false, true],
        "oldest first, this device marked"
    );
    assert_eq!(rows[0]["signed_in_at"], "2026-10-08T04:00:00Z");
    let text = list.to_string();
    for leak in [first.id.as_str(), second.id.as_str(), IP, "client_tag"] {
        assert!(!text.contains(leak), "{leak}");
    }
    let other = rows[0]["handle"].as_str().unwrap().to_owned();
    let mine = rows[1]["handle"].as_str().unwrap().to_owned();

    let current = write(
        &harness,
        &second,
        "DELETE",
        &format!("/api/public/sessions/{mine}"),
    )
    .await;
    assert_eq!(
        (current.status, current.api_error()),
        (409, "current_session".into())
    );
    let ended = write(
        &harness,
        &second,
        "DELETE",
        &format!("/api/public/sessions/{other}"),
    )
    .await;
    assert_eq!(ended.status, 204);
    let again = write(
        &harness,
        &second,
        "DELETE",
        &format!("/api/public/sessions/{other}"),
    )
    .await;
    assert_eq!((again.status, again.api_error()), (404, "not_found".into()));
    let gone = harness
        .get(
            "/api/public/session",
            &[
                (first.cookie().0, &first.cookie().1),
                ("CF-Connecting-IP", IP),
            ],
        )
        .await;
    assert_eq!(gone.status, 401);
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "logout_other",
    }));

    // The portrait: the stored hash or the monogram, private to the browser.
    let avatar = harness
        .get(
            "/api/public/session/avatar",
            &[
                (second.cookie().0, &second.cookie().1),
                ("CF-Connecting-IP", IP),
            ],
        )
        .await;
    assert_eq!(avatar.status, 200);
    assert_eq!(avatar.header("cache-control"), Some("private, no-cache"));
    assert_eq!(avatar.header("content-type"), Some("image/svg+xml"));
}

#[tokio::test]
async fn signing_out_everywhere_ends_every_session_including_this_one() {
    let harness = MemberHarness::new().await;
    let one = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let two = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let three = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let reply = write(&harness, &two, "POST", "/api/public/sessions/end-all").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.json(), serde_json::json!({"ended": 3}));
    crate::schemas::assert_valid("identity.json#/$defs/SessionsEnded", "ended", &reply.json());
    assert_eq!(
        reply.cookie(wire::MEMBER_SESSION_COOKIE).as_deref(),
        Some("")
    );
    assert!(harness.rows(MIKAN).await.is_empty());
    for browser in [one, two, three] {
        let reply = harness
            .get(
                "/api/public/session",
                &[
                    (browser.cookie().0, &browser.cookie().1),
                    ("CF-Connecting-IP", IP),
                ],
            )
            .await;
        assert_eq!(reply.status, 401);
    }
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "logout_all",
    }));
}

#[tokio::test]
async fn an_eleventh_sign_in_ends_the_oldest_session() {
    let harness = MemberHarness::new().await;
    let mut browsers = Vec::new();
    for n in 0..11 {
        harness.advance(TimeDelta::seconds(1));
        let ip = format!("203.0.113.{}", n + 1);
        browsers.push(
            harness
                .sign_in_from(discord_user(MIKAN, "Mikan"), &ip)
                .await,
        );
    }
    let rows = harness.rows(MIKAN).await;
    assert_eq!(rows.len(), 10);
    let oldest = &browsers[0];
    let reply = harness
        .get(
            "/api/public/session",
            &[
                (oldest.cookie().0, &oldest.cookie().1),
                ("CF-Connecting-IP", "203.0.113.1"),
            ],
        )
        .await;
    assert_eq!(reply.status, 401, "the oldest was ended");
    let second = &browsers[1];
    let reply = harness
        .get(
            "/api/public/session",
            &[
                (second.cookie().0, &second.cookie().1),
                ("CF-Connecting-IP", "203.0.113.2"),
            ],
        )
        .await;
    assert_eq!(reply.status, 200);
    assert_eq!(
        harness
            .audit
            .events()
            .iter()
            .filter(|event| **event
                == AuditEvent::SessionEnded {
                    actor: format!("discord:{MIKAN}"),
                    reason: "capped",
                })
            .count(),
        1
    );
}

fn roomy(_: Route) -> Limits {
    let rule = Rule {
        burst: 100.0,
        per_minute: 100.0,
    };
    Limits {
        per_ip: rule,
        global: rule,
    }
}

/// D5: the cap is one store write, so racing sign-ins against SQLite never
/// leave more than ten live sessions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sign_ins_never_exceed_ten_sessions() {
    let harness = MemberHarness::with(Options {
        sqlite: true,
        limits: Some(roomy),
        ..Options::default()
    })
    .await;
    let mut approved = Vec::new();
    for n in 0..30 {
        let ip = format!("203.0.113.{}", n + 1);
        let (path, login) = harness
            .approved(discord_user(MIKAN, "Mikan"), "%2F", &ip)
            .await;
        approved.push((path, login, ip));
    }
    let mut callbacks = tokio::task::JoinSet::new();
    for (path, login, ip) in approved {
        let public = harness.public;
        callbacks.spawn(async move {
            send(
                public,
                "GET",
                PUBLIC_HOST,
                &path,
                &[("Cookie", &login), ("CF-Connecting-IP", &ip)],
                None,
            )
            .await
            .status
        });
    }
    while let Some(status) = callbacks.join_next().await {
        assert_eq!(status.unwrap(), 200);
    }
    assert_eq!(harness.rows(MIKAN).await.len(), 10);
    assert_eq!(
        harness
            .audit
            .events()
            .iter()
            .filter(|event| **event
                == AuditEvent::SessionEnded {
                    actor: format!("discord:{MIKAN}"),
                    reason: "capped",
                })
            .count(),
        20
    );
}

#[tokio::test]
async fn admin_credentials_mean_nothing_on_the_public_origin() {
    let harness = MemberHarness::new().await;
    // A valid admin session and the break-glass token.
    let admin = send(
        harness.admin,
        "POST",
        ADMIN_HOST,
        "/api/admin/auth/token",
        &[("Origin", "https://kanade.test")],
        Some(&format!(r#"{{"token":"{TOKEN}"}}"#)),
    )
    .await;
    assert_eq!(admin.status, 200, "{}", admin.text());
    let admin_id = admin.cookie(wire::SESSION_COOKIE).unwrap();
    let admin_cookie = format!("{}={admin_id}", wire::SESSION_COOKIE);
    let bearer = format!("Bearer {TOKEN}");
    let renamed = cookie(&admin_id).1;
    for headers in [
        vec![("Cookie", admin_cookie.as_str())],
        vec![("Authorization", bearer.as_str())],
        // The admin id presented under the member name is just unknown.
        vec![("Cookie", renamed.as_str())],
        vec![
            ("Tailscale-User-Login", "ops@example.com"),
            (
                "X-Kanade-Edge-Auth",
                "edge-secret-shared-with-the-caddy-edge!!",
            ),
        ],
    ] {
        for path in ["/api/public/session", "/api/public/sessions"] {
            let reply = harness.get(path, &headers).await;
            assert_eq!(
                (reply.status, reply.api_error()),
                (401, "unauthenticated".into()),
                "{path} {headers:?}"
            );
        }
        let reply = harness
            .request("POST", "/api/public/sessions/end-all", &headers)
            .await;
        assert_eq!(reply.status, 401);
    }
    // The admin token cannot sign in on the public origin either.
    let reply = send(
        harness.public,
        "POST",
        crate::support::PUBLIC_HOST,
        "/api/admin/auth/token",
        &[PUB_ORIGIN],
        Some(&format!(r#"{{"token":"{TOKEN}"}}"#)),
    )
    .await;
    assert_eq!(reply.status, 404);
}

#[tokio::test]
async fn a_member_session_means_nothing_on_the_admin_origin() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let member_cookie = browser.cookie().1;
    let as_admin = format!("{}={}", wire::SESSION_COOKIE, browser.id);
    for cookie in [member_cookie.as_str(), as_admin.as_str()] {
        let reply = harness
            .admin_get("/api/admin/session", &[("Cookie", cookie)])
            .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (401, "unauthenticated".into()),
            "{cookie}"
        );
    }
    // Every member route is absent on admin, even with the realm misassigned there.
    for (method, path) in [
        ("GET", "/api/public/status"),
        ("GET", "/api/public/session"),
        ("GET", "/api/public/session/avatar"),
        ("GET", "/api/public/sessions"),
        ("DELETE", "/api/public/sessions/0123456789abcdef01234567"),
        ("POST", "/api/public/sessions/end-all"),
        ("GET", "/api/public/auth/discord/start"),
        ("GET", "/api/public/auth/discord/callback"),
        ("POST", "/api/public/auth/logout"),
    ] {
        let reply = send(
            harness.admin,
            method,
            ADMIN_HOST,
            path,
            &[("Cookie", &member_cookie), ("Content-Length", "0")],
            None,
        )
        .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (404, "not_found".into()),
            "{method} {path}"
        );
    }
}
