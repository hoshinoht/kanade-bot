//! Rate limits, Discord 429 cooldown, scope and bot refusals, revocation
//! failures, audit context and the gateway revocation hooks.

use std::time::Duration;

use chrono::TimeDelta;
use kanade::api::auth::{audit::AuditEvent, wire};

use super::{ADMIN_ROLE, Harness, ORIGIN, TOKEN, cookie, member, user};

fn start_path() -> &'static str {
    "/api/admin/auth/discord/start"
}

#[tokio::test]
async fn discord_start_and_callback_are_rate_limited_per_client() {
    let harness = Harness::new().await;
    for _ in 0..10 {
        assert_eq!(harness.get(start_path(), &[]).await.status, 303);
    }
    let limited = harness.get(start_path(), &[]).await;
    assert_eq!(
        limited.destination().as_deref(),
        Some("/?login_error=rate_limited")
    );
    assert!(
        limited
            .cookie(wire::LOGIN_COOKIE)
            .is_some_and(|value| value.is_empty())
    );
    harness.advance(TimeDelta::minutes(1));
    let again = harness.get(start_path(), &[]).await;
    assert!(
        again
            .header("location")
            .unwrap()
            .starts_with("https://discord.com/")
    );

    for _ in 0..10 {
        let reply = harness
            .get("/api/admin/auth/discord/callback?code=x&state=y", &[])
            .await;
        assert_eq!(reply.destination().as_deref(), Some("/?login_error=state"));
    }
    let limited = harness
        .get("/api/admin/auth/discord/callback?code=x&state=y", &[])
        .await;
    assert_eq!(
        limited.destination().as_deref(),
        Some("/?login_error=rate_limited")
    );

    let record = harness
        .audit
        .records()
        .into_iter()
        .find(|record| {
            record.event
                == AuditEvent::RateLimited {
                    route: "discord_start",
                }
        })
        .unwrap();
    assert_eq!(record.client.as_deref(), Some("127.0.0.1"));
}

#[tokio::test]
async fn token_logins_and_bearer_failures_are_rate_limited() {
    let harness = Harness::new().await;
    for _ in 0..5 {
        let reply = harness
            .post(
                "/api/admin/auth/token",
                &[ORIGIN],
                Some(r#"{"token":"guess"}"#),
            )
            .await;
        assert_eq!(reply.status, 401);
    }
    let right = format!(r#"{{"token":"{TOKEN}"}}"#);
    let limited = harness
        .post("/api/admin/auth/token", &[ORIGIN], Some(&right))
        .await;
    assert_eq!(
        (limited.status, limited.api_error()),
        (429, "rate_limited".into())
    );

    for _ in 0..5 {
        let reply = harness
            .get("/api/admin/session", &[("Authorization", "Bearer guess")])
            .await;
        assert_eq!(reply.status, 401);
    }
    let bearer = format!("Bearer {TOKEN}");
    let limited = harness
        .get("/api/admin/session", &[("Authorization", &bearer)])
        .await;
    assert_eq!(
        limited.status, 429,
        "even the right token waits once failures run out"
    );
    harness.advance(TimeDelta::minutes(1));
    assert_eq!(
        harness
            .get("/api/admin/session", &[("Authorization", &bearer)])
            .await
            .status,
        200
    );
    // Successful bearer calls cost nothing.
    for _ in 0..10 {
        assert_eq!(
            harness
                .get("/api/admin/session", &[("Authorization", &bearer)])
                .await
                .status,
            200
        );
    }
}

#[tokio::test]
async fn a_client_evicts_only_its_own_oldest_pending_login() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    let (first_path, first_cookie) = harness.discord_approved(user(111, "Alice"), "%2F").await;
    for _ in 0..5 {
        harness.advance(TimeDelta::seconds(1));
        assert_eq!(harness.get(start_path(), &[]).await.status, 303);
    }
    let reply = harness.get(&first_path, &[("Cookie", &first_cookie)]).await;
    assert_eq!(reply.destination().as_deref(), Some("/?login_error=state"));
}

#[tokio::test]
async fn discord_429_starts_a_cooldown_that_fails_closed_until_retry_after() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    harness.discord.rate_limit_next(Duration::from_secs(30));
    let reply = harness.discord_login(user(111, "Alice"), "%2F").await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=unavailable")
    );
    assert_eq!(harness.discord.exchanges(), 1);

    harness.advance(TimeDelta::seconds(29));
    let cooling = harness.get(start_path(), &[]).await;
    assert_eq!(
        cooling.destination().as_deref(),
        Some("/?login_error=unavailable")
    );
    assert_eq!(
        harness.discord.exchanges(),
        1,
        "no Discord call during the cooldown"
    );

    harness.advance(TimeDelta::seconds(1));
    let reply = harness.discord_login(user(111, "Alice"), "%2F").await;
    assert_eq!(reply.destination().as_deref(), Some("/"));
}

#[tokio::test]
async fn a_broader_scope_is_refused_and_the_token_revoked_unused() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    harness.discord.grant_scope("identify email");
    let reply = harness.discord_login(user(111, "Alice"), "%2F").await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=discord")
    );
    assert!(reply.cookie(wire::SESSION_COOKIE).is_none());
    assert_eq!(harness.discord.revoked(), 1);
    assert_eq!(harness.discord.live_tokens(), 0);
    assert!(harness.audit.events().contains(&AuditEvent::LoginRefused {
        method: "discord",
        reason: "scope",
        user: None,
    }));
}

#[tokio::test]
async fn bot_accounts_are_refused_with_their_id_logged() {
    let harness = Harness::new().await;
    harness.guild.put(member(777, &[ADMIN_ROLE], true));
    let mut bot = user(777, "Some Bot");
    bot.bot = true;
    let reply = harness.discord_login(bot, "%2F").await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=forbidden")
    );
    assert!(harness.audit.events().contains(&AuditEvent::LoginRefused {
        method: "discord",
        reason: "bot_account",
        user: Some("777".into()),
    }));
    // Refusals after /users/@me name the user; plain staff refusals too.
    let reply = harness.discord_login(user(222, "Stranger"), "%2F").await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=forbidden")
    );
    assert!(harness.audit.events().contains(&AuditEvent::LoginRefused {
        method: "discord",
        reason: "not_staff",
        user: Some("222".into()),
    }));
}

#[tokio::test]
async fn revocation_failures_are_logged_without_the_token_and_every_event_has_context() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    harness.discord.fail_revocation(true);
    let reply = harness.discord_login(user(111, "Alice"), "%2F").await;
    assert_eq!(reply.destination().as_deref(), Some("/"), "best effort");
    let records = harness.audit.records();
    let failed = records
        .iter()
        .find(|record| matches!(record.event, AuditEvent::RevokeFailed { .. }))
        .unwrap();
    assert_eq!(
        failed.event,
        AuditEvent::RevokeFailed {
            reason: "unavailable",
            user: Some("111".into()),
        }
    );
    for record in &records {
        assert_eq!(record.request_id.len(), 16, "{record:?}");
        assert_eq!(record.client.as_deref(), Some("127.0.0.1"), "{record:?}");
    }
    let lines = harness.audit.lines();
    assert!(lines.iter().all(|line| line.contains("\"request_id\"")));
}

#[tokio::test]
async fn gateway_hooks_revoke_members_who_leave_or_lose_staff() {
    let harness = Harness::new().await;
    let auth = harness.site.auth.clone().unwrap();
    for id in [111, 222, 333] {
        harness.guild.put(member(id, &[ADMIN_ROLE], false));
    }
    let mut sessions = Vec::new();
    for id in [111, 222, 333] {
        let reply = harness.discord_login(user(id, "Admin"), "%2F").await;
        sessions.push(cookie(&reply.cookie(wire::SESSION_COOKIE).unwrap()));
    }
    let status = |index: usize| {
        let (name, value) = sessions[index].clone();
        let harness = &harness;
        async move {
            harness
                .get("/api/admin/session", &[(name, &value)])
                .await
                .status
        }
    };

    assert_eq!(auth.member_left("111").await, 1);
    assert_eq!(
        status(0).await,
        401,
        "left the guild: gone at once, no TTL wait"
    );

    assert_eq!(auth.member_changed("222").await, 0, "still staff");
    assert_eq!(status(1).await, 200);
    harness.guild.put(member(222, &[10], false));
    assert_eq!(auth.member_changed("222").await, 1);
    assert_eq!(status(1).await, 401, "lost the admin role");

    harness.guild.put(member(333, &[], false));
    harness.guild.set_unavailable(true);
    assert_eq!(
        auth.member_changed("333").await,
        0,
        "unknown data leaves it to the re-check"
    );
    harness.guild.set_unavailable(false);
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: "discord:111".into(),
        reason: "member_left",
    }));
}
