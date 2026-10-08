//! The member session after sign-in: expiry, the eligibility re-check and
//! roster hooks, IP-change rotation with its read-only grace (D9), CSRF on
//! sign-out and the fresh-write window.

use std::sync::Arc;

use axum::{Router, middleware::from_fn_with_state, routing::post};
use chrono::TimeDelta;
use kanade::{
    api::{
        auth::{
            audit::AuditEvent,
            csrf,
            member::{Eligibility, MemberSession, require_session},
            roster::on_roster_update,
            wire,
        },
        error::ApiError,
        guard::proxy,
        listeners::Site,
    },
    bot::events::RosterUpdate,
};

use super::member_support::{Browser, MIKAN, MemberHarness, PUB_ORIGIN, cookie, discord_user};
use crate::support::{Fixture, PUBLIC_HOST, Reply, send, spawn_router};

const HOME: &str = "198.51.100.10";
const MOBILE: &str = "203.0.113.77";

async fn me(harness: &MemberHarness, id: &str, ip: &str) -> Reply {
    harness
        .get(
            "/api/public/session",
            &[(cookie(id).0, &cookie(id).1), ("CF-Connecting-IP", ip)],
        )
        .await
}

async fn end_other(harness: &MemberHarness, browser: &Browser, csrf: &str, ip: &str) -> Reply {
    harness
        .request(
            "DELETE",
            "/api/public/sessions/000000000000000000000000",
            &[
                (browser.cookie().0, &browser.cookie().1),
                PUB_ORIGIN,
                ("X-Kanade-CSRF", csrf),
                ("CF-Connecting-IP", ip),
            ],
        )
        .await
}

#[tokio::test]
async fn sessions_end_after_the_idle_or_the_absolute_lifetime() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    harness.advance(TimeDelta::minutes(29));
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 200);
    harness.advance(TimeDelta::minutes(30));
    let idle = me(&harness, &browser.id, HOME).await;
    assert_eq!(
        (idle.status, idle.api_error()),
        (401, "unauthenticated".into())
    );
    assert_eq!(
        idle.cookie(wire::MEMBER_SESSION_COOKIE).as_deref(),
        Some("")
    );
    assert!(harness.rows(MIKAN).await.is_empty());

    // Kept busy, a session still ends at its absolute lifetime (8 h).
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    for _ in 0..16 {
        harness.advance(TimeDelta::minutes(29));
        assert_eq!(me(&harness, &browser.id, HOME).await.status, 200);
    }
    harness.advance(TimeDelta::minutes(16));
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 401);
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "expired",
    }));
}

#[tokio::test]
async fn eligibility_is_rechecked_every_five_minutes() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let second = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let checks = harness.gate.checks();
    // The role goes without a gateway hook reaching the realm.
    harness.roster(MIKAN, false).await;
    harness.advance(TimeDelta::minutes(4));
    assert_eq!(
        me(&harness, &browser.id, HOME).await.status,
        200,
        "not due yet"
    );
    assert_eq!(harness.gate.checks(), checks);
    harness.advance(TimeDelta::minutes(1));
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 401);
    assert!(
        harness.rows(MIKAN).await.is_empty(),
        "every session of the member"
    );
    assert_eq!(me(&harness, &second.id, HOME).await.status, 401);
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "not_eligible",
    }));
}

#[tokio::test]
async fn unavailable_member_data_answers_503_and_keeps_the_session() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    harness.gate.force(Some(Eligibility::Unavailable));
    harness.advance(TimeDelta::minutes(5));
    let reply = me(&harness, &browser.id, HOME).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (503, "auth_unavailable".into())
    );
    assert_eq!(harness.rows(MIKAN).await.len(), 1);
    harness.gate.force(None);
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 200);
}

#[tokio::test]
async fn losing_the_role_or_leaving_ends_sessions_through_the_roster_hook() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let seen = |has_role| RosterUpdate::Seen {
        user_id: MIKAN.to_string(),
        display_name: "Mikan".into(),
        nickname: None,
        has_role,
        roles: Vec::new(),
        is_guild_admin: false,
    };
    let hook = |update: RosterUpdate| {
        let harness = &harness;
        async move {
            on_roster_update(
                &harness.admin_auth,
                Some(&harness.member),
                &*harness.store,
                None,
                &update,
            )
            .await
            .unwrap()
        }
    };
    assert_eq!(hook(seen(true)).await, 0, "still eligible");
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 200);
    assert_eq!(hook(seen(false)).await, 1, "role lost: ended at once");
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 401);

    harness.roster(MIKAN, true).await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let left = RosterUpdate::Left {
        user_id: MIKAN.to_string(),
    };
    assert_eq!(hook(left).await, 1);
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 401);
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "member_left",
    }));
}

#[tokio::test]
async fn a_new_client_address_rotates_the_id_rechecks_and_keeps_a_read_only_grace() {
    let harness = MemberHarness::new().await;
    let old = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let before = harness.rows(MIKAN).await[0].clone();
    harness.advance(TimeDelta::minutes(1));
    let checks = harness.gate.checks();

    // A write from the new address, with the old id's token: served, rotated.
    let rotated = end_other(&harness, &old, &old.csrf, MOBILE).await;
    assert_eq!(rotated.api_error(), "not_found", "served: no such handle");
    assert_eq!(harness.gate.checks(), checks + 1, "re-checked on rotation");
    let new_id = rotated.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert_ne!(new_id, old.id);
    let line = rotated
        .all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{}={new_id}", wire::MEMBER_SESSION_COOKIE)))
        .unwrap()
        .to_owned();
    assert!(
        line.ends_with(&format!("Max-Age={}", 8 * 3600 - 60)),
        "{line}"
    );
    let new_csrf = rotated.header("x-kanade-csrf").unwrap().to_owned();
    assert_eq!(new_csrf, csrf::token(csrf::MEMBER, &new_id));
    let rows = harness.rows(MIKAN).await;
    assert_eq!(rows.len(), 1, "the old id is superseded, not listed");
    assert_eq!(
        (rows[0].created_at, rows[0].expires_at),
        (before.created_at, before.expires_at),
        "rotation never extends a lifetime"
    );
    assert_ne!(rows[0].client_tag, before.client_tag);
    assert!(
        harness
            .audit
            .events()
            .contains(&AuditEvent::SessionRotated {
                actor: format!("discord:{MIKAN}"),
            })
    );

    // The old id: GET only, within 30 s, without handing out its token.
    let grace = me(&harness, &old.id, HOME).await;
    assert_eq!(grace.status, 200);
    assert_eq!(grace.header("x-kanade-csrf"), None);
    assert_eq!(
        grace.cookie(wire::MEMBER_SESSION_COOKIE),
        None,
        "not cleared"
    );
    assert_eq!(grace.json()["fresh_until"], "2026-10-08T04:15:00Z");
    let write = end_other(&harness, &old, &old.csrf, HOME).await;
    assert_eq!(
        (write.status, write.api_error()),
        (401, "unauthenticated".into())
    );
    let new = Browser {
        id: new_id.clone(),
        csrf: new_csrf.clone(),
    };
    assert_eq!(
        end_other(&harness, &new, &old.csrf, MOBILE)
            .await
            .api_error(),
        "csrf",
        "the old token no longer works"
    );
    assert_eq!(
        end_other(&harness, &new, &new_csrf, MOBILE)
            .await
            .api_error(),
        "not_found"
    );
    harness.advance(TimeDelta::seconds(30));
    assert_eq!(me(&harness, &old.id, HOME).await.status, 401, "grace over");
    let fresh = me(&harness, &new_id, MOBILE).await;
    assert_eq!(fresh.status, 200);
    assert_eq!(fresh.json()["fresh_until"], "2026-10-08T04:15:00Z");
}

#[tokio::test]
async fn a_rotation_that_finds_the_member_ineligible_ends_everything() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    harness.roster(MIKAN, false).await;
    let reply = me(&harness, &browser.id, MOBILE).await;
    assert_eq!(reply.status, 401);
    assert_eq!(
        reply.cookie(wire::MEMBER_SESSION_COOKIE).as_deref(),
        Some("")
    );
    assert!(harness.rows(MIKAN).await.is_empty());
    assert_eq!(
        me(&harness, &browser.id, HOME).await.status,
        401,
        "old id gone too"
    );
}

#[tokio::test]
async fn sign_out_needs_this_origin_and_the_member_token() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let session = browser.cookie();
    let admin_token = csrf::token(csrf::ADMIN, &browser.id);
    for (headers, why) in [
        (
            vec![
                (session.0, session.1.as_str()),
                ("X-Kanade-CSRF", browser.csrf.as_str()),
            ],
            "no origin",
        ),
        (
            vec![
                (session.0, session.1.as_str()),
                ("Origin", "https://evil.example"),
                ("X-Kanade-CSRF", browser.csrf.as_str()),
            ],
            "foreign origin",
        ),
        (
            vec![
                (session.0, session.1.as_str()),
                ("Sec-Fetch-Site", "cross-site"),
                ("X-Kanade-CSRF", browser.csrf.as_str()),
            ],
            "cross-site fetch",
        ),
        (
            vec![(session.0, session.1.as_str()), PUB_ORIGIN],
            "no token",
        ),
        (
            vec![
                (session.0, session.1.as_str()),
                PUB_ORIGIN,
                ("X-Kanade-CSRF", admin_token.as_str()),
            ],
            "the admin realm's token",
        ),
    ] {
        let reply = harness
            .request("POST", "/api/public/auth/logout", &headers)
            .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (403, "csrf".into()),
            "{why}"
        );
    }
    assert_eq!(harness.rows(MIKAN).await.len(), 1);

    let reply = harness
        .request(
            "POST",
            "/api/public/auth/logout",
            &[
                (session.0, &session.1),
                PUB_ORIGIN,
                ("X-Kanade-CSRF", &browser.csrf),
            ],
        )
        .await;
    assert_eq!(reply.status, 204);
    assert_eq!(
        reply.cookie(wire::MEMBER_SESSION_COOKIE).as_deref(),
        Some("")
    );
    assert_eq!(reply.cookie(wire::MEMBER_LOGIN_COOKIE).as_deref(), Some(""));
    assert!(harness.rows(MIKAN).await.is_empty());
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "logout",
    }));
    // Signed out (or closed), sign-out still answers.
    harness.set_open(false);
    let again = harness
        .request("POST", "/api/public/auth/logout", &[PUB_ORIGIN])
        .await;
    assert_eq!(again.status, 204);
}

/// D9: a rotated-out id only reads during its grace; sign-out is a write.
#[tokio::test]
async fn a_rotated_out_id_cannot_sign_out_during_its_grace() {
    let harness = MemberHarness::new().await;
    let old = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let rotated = me(&harness, &old.id, MOBILE).await;
    assert_eq!(rotated.status, 200);
    let new_id = rotated.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert_ne!(new_id, old.id);

    let reply = harness
        .request(
            "POST",
            "/api/public/auth/logout",
            &[
                (old.cookie().0, &old.cookie().1),
                PUB_ORIGIN,
                ("X-Kanade-CSRF", &old.csrf),
            ],
        )
        .await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (401, "unauthenticated".into())
    );
    assert_eq!(
        reply.cookie(wire::MEMBER_SESSION_COOKIE),
        None,
        "not cleared: the browser may already hold the new id"
    );
    assert_eq!(
        me(&harness, &old.id, HOME).await.status,
        200,
        "still in grace"
    );
    assert_eq!(me(&harness, &new_id, MOBILE).await.status, 200);
    assert_eq!(harness.rows(MIKAN).await.len(), 1);
    assert!(!harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: format!("discord:{MIKAN}"),
        reason: "logout",
    }));
}

#[tokio::test]
async fn ending_a_device_needs_this_origin_and_the_member_token() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let other = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let list = harness
        .get(
            "/api/public/sessions",
            &[
                (browser.cookie().0, &browser.cookie().1),
                ("CF-Connecting-IP", "198.51.100.10"),
            ],
        )
        .await
        .json();
    let handle = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["current"] == false)
        .unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/api/public/sessions/{handle}");
    let session = browser.cookie();
    for headers in [
        vec![
            (session.0, session.1.as_str()),
            ("X-Kanade-CSRF", browser.csrf.as_str()),
        ],
        vec![(session.0, session.1.as_str()), PUB_ORIGIN],
        vec![
            (session.0, session.1.as_str()),
            ("Origin", "https://kanade.test"),
            ("X-Kanade-CSRF", browser.csrf.as_str()),
        ],
        vec![
            (session.0, session.1.as_str()),
            PUB_ORIGIN,
            ("X-Kanade-CSRF", other.csrf.as_str()),
        ],
    ] {
        let mut headers = headers;
        headers.push(("CF-Connecting-IP", "198.51.100.10"));
        let reply = harness.request("DELETE", &path, &headers).await;
        assert_eq!(reply.api_error(), "csrf", "{headers:?}");
    }
    assert_eq!(harness.rows(MIKAN).await.len(), 2);
}

/// A write that needs a fresh sign-in, behind the real middleware and the
/// harness's clock (no member route uses the guard yet).
async fn fresh_probe(harness: &MemberHarness, fixture: &Fixture) -> std::net::SocketAddr {
    let mut http = fixture.http();
    http.cloudflared_peer = Some([127, 0, 0, 1].into());
    let mut site = Site::public(&http).unwrap();
    site.member = Some(harness.member.clone());
    let site = Arc::new(site);
    let clock = harness.clock();
    let router = Router::new()
        .route(
            "/fresh",
            post(move |session: MemberSession| async move {
                let now = *clock.lock().unwrap();
                session.require_fresh(now).map(|()| "fresh")
            }),
        )
        .route_layer(from_fn_with_state(site.clone(), require_session))
        .with_state(site.clone())
        .layer(from_fn_with_state(site, proxy::sanitize));
    spawn_router(router).await
}

#[tokio::test]
async fn the_fresh_write_window_ends_fifteen_minutes_after_sign_in() {
    let harness = MemberHarness::new().await;
    let fixture = Fixture::new();
    let probe = fresh_probe(&harness, &fixture).await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let write = |browser: Browser| async move {
        send(
            probe,
            "POST",
            PUBLIC_HOST,
            "/fresh",
            &[
                (browser.cookie().0, &browser.cookie().1),
                PUB_ORIGIN,
                ("X-Kanade-CSRF", &browser.csrf),
                ("CF-Connecting-IP", HOME),
                ("Content-Length", "0"),
            ],
            None,
        )
        .await
    };
    harness.advance(TimeDelta::minutes(15) - TimeDelta::seconds(1));
    let fresh = write(browser.clone()).await;
    assert_eq!((fresh.status, fresh.text()), (200, "fresh".into()));
    harness.advance(TimeDelta::seconds(1));
    let stale = write(browser.clone()).await;
    assert_eq!(
        (stale.status, stale.api_error()),
        (401, ApiError::REAUTH_REQUIRED.error.into())
    );
    assert_eq!(
        me(&harness, &browser.id, HOME).await.status,
        200,
        "still signed in"
    );

    // Signing in again from this browser replaces the session: fresh again.
    let (path, login) = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", HOME)
        .await;
    let both = format!("{login}; {}={}", wire::MEMBER_SESSION_COOKIE, browser.id);
    let reply = harness
        .get(&path, &[("Cookie", &both), ("CF-Connecting-IP", HOME)])
        .await;
    assert_eq!(reply.status, 200);
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert_eq!(harness.rows(MIKAN).await.len(), 1, "replaced, not added");
    assert_eq!(me(&harness, &browser.id, HOME).await.status, 401);
    let renewed = Browser {
        csrf: csrf::token(csrf::MEMBER, &id),
        id,
    };
    assert_eq!(write(renewed).await.status, 200);
}
