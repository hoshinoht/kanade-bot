//! Session lifetime, rotation, CSRF, logout, staff re-check and the public origin.

use std::sync::Arc;

use chrono::TimeDelta;
use kanade::api::auth::wire;

use super::{ADMIN_ROLE, Harness, ORIGIN, TOKEN, actor_probe, cookie, member, user};
use crate::support::{PUBLIC_HOST, request, send};

#[tokio::test]
async fn missing_foreign_or_cross_site_csrf_is_refused_and_logout_ends_the_session() {
    let harness = Harness::new().await;
    let (id, csrf) = harness.token_login().await;
    let (other_id, other_csrf) = harness.token_login().await;
    assert_ne!(csrf, other_csrf);
    let (name, value) = cookie(&id);
    let session = (name, value.as_str());

    for (case, extra) in [
        ("no token", vec![session, ORIGIN]),
        (
            "another session's token",
            vec![session, ORIGIN, ("X-Kanade-CSRF", &other_csrf)],
        ),
        (
            "cross-site",
            vec![
                session,
                ("Origin", "https://evil.example"),
                ("X-Kanade-CSRF", &csrf),
            ],
        ),
        (
            "same-site",
            vec![
                session,
                ("Sec-Fetch-Site", "same-site"),
                ("X-Kanade-CSRF", &csrf),
            ],
        ),
        ("no origin markers", vec![session, ("X-Kanade-CSRF", &csrf)]),
    ] {
        let reply = harness.post("/api/admin/auth/logout", &extra, None).await;
        assert_eq!(reply.status, 403, "{case}");
        assert_eq!(reply.api_error(), "csrf", "{case}");
    }
    assert_eq!(
        harness.get("/api/admin/session", &[session]).await.status,
        200
    );

    let reply = harness
        .post(
            "/api/admin/auth/logout",
            &[
                session,
                ("Sec-Fetch-Site", "same-origin"),
                ("X-Kanade-CSRF", &csrf),
            ],
            None,
        )
        .await;
    assert_eq!(reply.status, 204);
    assert_eq!(reply.cookie(wire::SESSION_COOKIE).as_deref(), Some(""));
    let after = harness.get("/api/admin/session", &[session]).await;
    assert_eq!(after.status, 401);
    assert_eq!(after.api_error(), "unauthenticated");

    // The other session is untouched.
    let (name, value) = cookie(&other_id);
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn idle_and_absolute_timeouts_end_sessions() {
    let harness = Harness::new().await;
    let (id, _) = harness.token_login().await;
    let (name, value) = cookie(&id);
    harness.advance(TimeDelta::minutes(59));
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        200
    );
    harness.advance(TimeDelta::minutes(59));
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        200,
        "activity keeps it alive"
    );
    harness.advance(TimeDelta::minutes(61));
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        401,
        "idle"
    );

    let (id, _) = harness.token_login().await;
    let (name, value) = cookie(&id);
    for _ in 0..14 {
        harness.advance(TimeDelta::minutes(50));
        assert_eq!(
            harness
                .get("/api/admin/session", &[(name, &value)])
                .await
                .status,
            200
        );
    }
    harness.advance(TimeDelta::minutes(50));
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        401,
        "absolute lifetime of 12 h"
    );
}

#[tokio::test]
async fn logging_in_rotates_the_session_id() {
    let harness = Harness::new().await;
    let (old, _) = harness.token_login().await;
    let (name, value) = cookie(&old);
    let reply = harness
        .post(
            "/api/admin/auth/token",
            &[ORIGIN, (name, &value)],
            Some(&format!(r#"{{"token":"{TOKEN}"}}"#)),
        )
        .await;
    let new = reply.cookie(wire::SESSION_COOKIE).unwrap();
    assert_ne!(new, old);
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        401
    );
    let (name, value) = cookie(&new);
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn staff_is_rechecked_after_the_ttl_and_revocation_ends_every_session() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    let first = harness.discord_login(user(111, "Alice"), "%2F").await;
    let second = harness.discord_login(user(111, "Alice"), "%2F").await;
    let sessions: Vec<_> = [first, second]
        .iter()
        .map(|reply| cookie(&reply.cookie(wire::SESSION_COOKIE).unwrap()))
        .collect();

    harness.guild.remove(twilight_model::id::Id::new(111));
    harness.advance(TimeDelta::minutes(4));
    let (name, value) = &sessions[0];
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, value)])
            .await
            .status,
        200,
        "within the re-check TTL"
    );

    harness.guild.set_unavailable(true);
    harness.advance(TimeDelta::minutes(2));
    let reply = harness.get("/api/admin/session", &[(name, value)]).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (503, "auth_unavailable".into()),
        "fails closed"
    );

    harness.guild.set_unavailable(false);
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, value)])
            .await
            .status,
        401
    );
    let (name, value) = &sessions[1];
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, value)])
            .await
            .status,
        401,
        "every session of the revoked identity ended"
    );
}

#[tokio::test]
async fn actors_are_attributed_per_method() {
    let harness = Harness::with(Some([127, 0, 0, 1].into()), TOKEN).await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    let probe = actor_probe(harness.site.clone()).await;
    let host = crate::support::ADMIN_HOST;

    let discord = harness.discord_login(user(111, "Alice"), "%2F").await;
    let discord_id = discord.cookie(wire::SESSION_COOKIE).unwrap();
    let me = harness
        .get("/api/admin/session", &[cookie(&discord_id).as_pair()])
        .await;
    let csrf = me.header("x-kanade-csrf").unwrap().to_owned();
    let reply = send(
        probe,
        "POST",
        host,
        "/probe",
        &[
            cookie(&discord_id).as_pair(),
            ORIGIN,
            ("X-Kanade-CSRF", &csrf),
        ],
        None,
    )
    .await;
    assert_eq!(reply.text(), "discord:111");

    let bearer = format!("Bearer {TOKEN}");
    let reply = send(
        probe,
        "POST",
        host,
        "/probe",
        &[("Authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(reply.text(), "token", "bearer calls need no CSRF token");

    let ts = harness
        .post(
            "/api/admin/auth/tailscale",
            &[ORIGIN, ("Tailscale-User-Login", "Ops@Example.com")],
            None,
        )
        .await;
    let ts_id = ts.cookie(wire::SESSION_COOKIE).unwrap();
    let ts_csrf = ts.header("x-kanade-csrf").unwrap().to_owned();
    let reply = send(
        probe,
        "POST",
        host,
        "/probe",
        &[
            cookie(&ts_id).as_pair(),
            ORIGIN,
            ("X-Kanade-CSRF", &ts_csrf),
            ("Tailscale-User-Login", "ops@example.com"),
        ],
        None,
    )
    .await;
    assert_eq!(reply.text(), "tailscale:ops@example.com");
}

#[tokio::test]
async fn admin_cookies_and_bearers_mean_nothing_on_the_public_listener() {
    let harness = Harness::new().await;
    let (id, csrf) = harness.token_login().await;
    let (name, value) = cookie(&id);
    let bearer = format!("Bearer {TOKEN}");
    for path in [
        "/api/admin/session",
        "/api/admin/auth/logout",
        "/api/admin/auth/token",
    ] {
        for method in ["GET", "POST"] {
            let reply = request(
                harness.public,
                method,
                PUBLIC_HOST,
                path,
                &[
                    (name, &value),
                    ("Authorization", &bearer),
                    ("X-Kanade-CSRF", &csrf),
                    ORIGIN,
                ],
            )
            .await;
            assert_eq!(reply.status, 404, "{method} {path}");
            assert!(reply.cookie(wire::SESSION_COOKIE).is_none());
        }
    }
    // The admin session survived the attempts.
    assert_eq!(
        harness
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn token_rotation_ends_sessions_made_with_the_old_token() {
    let store = Arc::new(kanade::infrastructure::store::MemoryScheduleStore::new());
    let before = Harness::sharing(None, TOKEN, store.clone()).await;
    let (id, _) = before.token_login().await;
    let after = Harness::sharing(None, "a-completely-different-break-glass-token!!", store).await;
    let (name, value) = cookie(&id);
    assert_eq!(
        after
            .get("/api/admin/session", &[(name, &value)])
            .await
            .status,
        401
    );
}

trait AsPair {
    fn as_pair(&self) -> (&str, &str);
}

impl AsPair for (&'static str, String) {
    fn as_pair(&self) -> (&str, &str) {
        (self.0, self.1.as_str())
    }
}
