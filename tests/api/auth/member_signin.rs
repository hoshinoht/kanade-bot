//! Member sign-in on the public origin against fake Discord: state, PKCE,
//! redirect, scope, bots, token handling, `next`, eligibility, rate limits,
//! the Discord cooldown and the closed portal.

use std::time::Duration;

use chrono::TimeDelta;
use kanade::{
    api::auth::{
        audit::{AuditEvent, Realm},
        crypto,
        discord::LOGIN_TTL,
        member::Eligibility,
        rate::{Limits, Route, Rule},
        roster::on_roster_update,
        wire,
    },
    bot::events::RosterUpdate,
};

use super::member_support::{
    MIKAN, MemberHarness, NOROLE, Options, PUBLIC_CLIENT_ID, PUBLIC_REDIRECT, cookie, discord_user,
};
use crate::support::Reply;

const IP: &str = "198.51.100.10";

fn login_error(reply: &Reply) -> Option<String> {
    reply
        .header("location")
        .and_then(|location| location.strip_prefix("/?login_error="))
        .map(str::to_owned)
}

/// No session cookie with a value was set.
fn no_session_cookie(reply: &Reply) -> bool {
    reply
        .cookie(wire::MEMBER_SESSION_COOKIE)
        .is_none_or(|value| value.is_empty())
}

#[tokio::test]
async fn an_eligible_member_signs_in_with_a_strict_host_cookie() {
    let harness = MemberHarness::new().await;
    let start = harness
        .get("/api/public/auth/discord/start?next=%2Faccount", &[])
        .await;
    assert_eq!(start.status, 303);
    assert_eq!(start.header("cache-control"), Some("no-store"));
    assert_eq!(start.header("referrer-policy"), Some("no-referrer"));
    let login = start
        .all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(wire::MEMBER_LOGIN_COOKIE))
        .unwrap()
        .to_owned();
    assert!(login.ends_with("; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=600"));

    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2Faccount", IP)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(reply.destination().as_deref(), Some("/account"));
    assert!(!reply.text().contains("<script"));
    let session = reply
        .all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{}=", wire::MEMBER_SESSION_COOKIE)))
        .unwrap()
        .to_owned();
    assert!(
        session.ends_with("; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=28800"),
        "{session}"
    );
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert!(wire::is_token(&id));
    assert_eq!(reply.cookie(wire::MEMBER_LOGIN_COOKIE).as_deref(), Some(""));

    let rows = harness.rows(MIKAN).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.display, "Mikan");
    assert_eq!(row.expires_at, harness.now() + TimeDelta::hours(8));
    assert!(row.client_tag.as_ref().is_some_and(|tag| tag.len() == 64));
    assert!(!row.id_hash.contains(&id), "only the hash is stored");

    let me = harness
        .get(
            "/api/public/session",
            &[(cookie(&id).0, &cookie(&id).1), ("CF-Connecting-IP", IP)],
        )
        .await;
    assert_eq!(me.status, 200, "{}", me.text());
    let body = me.json();
    assert_eq!(body["member"]["id"], MIKAN.to_string());
    assert_eq!(body["member"]["display"], "Mikan");
    assert!(
        body["member"]["avatar"]
            .as_str()
            .unwrap()
            .starts_with("/api/public/session/avatar?v=")
    );
    assert_eq!(body["fresh_until"], "2026-10-08T04:15:00Z");
    assert!(me.header("x-kanade-csrf").is_some());
    crate::schemas::assert_valid("public.json#/$defs/PublicSession", "session", &body);

    let records = harness.audit.records();
    assert!(records.iter().all(|record| record.realm == Realm::Member));
    let login = records
        .iter()
        .find(|record| {
            matches!(&record.event, AuditEvent::LoginSucceeded { method: "discord", actor, .. }
                if *actor == format!("discord:{MIKAN}"))
        })
        .expect("a login record");
    // Members' records keep only the keyed tag of the address (item 20).
    let client = login.client.as_deref().expect("the client's tag");
    assert!(
        client.len() == 64 && client.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{client}"
    );
}

#[tokio::test]
async fn the_authorize_request_is_the_public_application_identify_only_with_prompt_none() {
    let harness = MemberHarness::new().await;
    let start = harness.get("/api/public/auth/discord/start", &[]).await;
    let location = start.header("location").unwrap();
    assert!(location.starts_with("https://discord.com/oauth2/authorize?"));
    let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
    let value = |key| wire::query_value(&pairs, key).unwrap();
    assert_eq!(value("client_id"), PUBLIC_CLIENT_ID);
    assert_eq!(value("scope"), "identify");
    assert_eq!(value("prompt"), "none");
    assert_eq!(value("redirect_uri"), PUBLIC_REDIRECT);
    assert_eq!(value("code_challenge_method"), "S256");
    assert!(!location.contains("public-discord-client-secret"));

    harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    assert_eq!(harness.discord.redirect_uris(), [PUBLIC_REDIRECT]);
}

/// The verifier is stored with the state: a code issued for another
/// challenge fails the exchange.
#[tokio::test]
async fn the_pkce_verifier_travels_with_the_state() {
    let harness = MemberHarness::new().await;
    let (path, login) = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    let code = path
        .split_once("code=")
        .and_then(|(_, rest)| rest.split_once('&'))
        .map(|(code, _)| code.to_owned())
        .unwrap();
    // Re-approve the same code for a different challenge.
    harness.discord.approve(
        &code,
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        discord_user(MIKAN, "Mikan"),
    );
    let reply = harness.get(&path, &[("Cookie", &login)]).await;
    assert_eq!(login_error(&reply).as_deref(), Some("discord"));
    assert!(harness.rows(MIKAN).await.is_empty());
}

#[tokio::test]
async fn state_is_one_use_bound_to_its_login_and_expires() {
    let harness = MemberHarness::new().await;
    let user = || discord_user(MIKAN, "Mikan");

    // Replayed.
    let (path, login) = harness.approved(user(), "%2F", IP).await;
    assert_eq!(harness.get(&path, &[("Cookie", &login)]).await.status, 200);
    let replay = harness.get(&path, &[("Cookie", &login)]).await;
    assert_eq!(login_error(&replay).as_deref(), Some("state"));
    assert!(no_session_cookie(&replay));

    // Forged state, then the right one: the login was consumed by the forgery.
    let (path, login) = harness.approved(user(), "%2F", IP).await;
    let forged = path.replace("state=", "state=forged");
    assert_eq!(
        login_error(&harness.get(&forged, &[("Cookie", &login)]).await).as_deref(),
        Some("state")
    );
    assert_eq!(
        login_error(&harness.get(&path, &[("Cookie", &login)]).await).as_deref(),
        Some("state")
    );

    // Mismatched: another login's cookie, or none.
    let (path, _) = harness.approved(user(), "%2F", IP).await;
    let (_, other) = harness.approved(user(), "%2F", IP).await;
    assert_eq!(
        login_error(&harness.get(&path, &[("Cookie", &other)]).await).as_deref(),
        Some("state")
    );
    let (path, _) = harness.approved(user(), "%2F", IP).await;
    assert_eq!(
        login_error(&harness.get(&path, &[]).await).as_deref(),
        Some("state")
    );

    // Expired.
    let (path, login) = harness.approved(user(), "%2F", IP).await;
    harness.advance(LOGIN_TTL);
    assert_eq!(
        login_error(&harness.get(&path, &[("Cookie", &login)]).await).as_deref(),
        Some("state")
    );
    assert_eq!(harness.rows(MIKAN).await.len(), 1, "only the first sign-in");
}

fn one_callback(route: Route) -> Limits {
    let rule = |burst| Rule {
        burst,
        per_minute: 1.0,
    };
    match route {
        Route::DiscordCallback => Limits {
            per_ip: rule(1.0),
            global: rule(10.0),
        },
        _ => Limits {
            per_ip: rule(10.0),
            global: rule(10.0),
        },
    }
}

/// A rate-limited callback still consumes the state, so retrying it later
/// is a replay.
#[tokio::test]
async fn a_rate_limited_callback_consumes_its_state() {
    let harness = MemberHarness::with(Options {
        limits: Some(one_callback),
        ..Options::default()
    })
    .await;
    let first = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    let second = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    let ip = ("CF-Connecting-IP", IP);
    assert_eq!(
        harness
            .get(&first.0, &[("Cookie", &first.1), ip])
            .await
            .status,
        200
    );
    let limited = harness.get(&second.0, &[("Cookie", &second.1), ip]).await;
    assert_eq!(login_error(&limited).as_deref(), Some("rate_limited"));
    harness.advance(TimeDelta::minutes(2));
    let retried = harness.get(&second.0, &[("Cookie", &second.1), ip]).await;
    assert_eq!(login_error(&retried).as_deref(), Some("state"));
    assert!(harness.audit.events().contains(&AuditEvent::RateLimited {
        route: "discord_callback"
    }));
}

#[tokio::test]
async fn a_scope_broader_than_identify_is_refused_and_its_token_revoked_unused() {
    let harness = MemberHarness::new().await;
    harness.discord.grant_scope("identify email");
    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    assert_eq!(login_error(&reply).as_deref(), Some("discord"));
    assert!(no_session_cookie(&reply));
    assert_eq!(harness.discord.revoked(), 1);
    assert_eq!(harness.discord.live_tokens(), 0);
    assert!(harness.rows(MIKAN).await.is_empty());
}

#[tokio::test]
async fn bots_and_members_without_the_role_are_not_eligible_and_get_nothing() {
    let harness = MemberHarness::new().await;
    let mut bot = discord_user(MIKAN, "Botty");
    bot.bot = true;
    for user in [
        bot,
        discord_user(NOROLE, "Plain"),
        discord_user(42, "Stranger"),
    ] {
        let id: u64 = user.id.parse().unwrap();
        let reply = harness.callback(user, "%2F", IP).await;
        assert_eq!(reply.status, 303);
        assert_eq!(login_error(&reply).as_deref(), Some("not_eligible"));
        assert!(no_session_cookie(&reply), "no session cookie");
        assert_eq!(reply.cookie(wire::MEMBER_LOGIN_COOKIE).as_deref(), Some(""));
        assert!(harness.rows(id).await.is_empty(), "no session row");
    }
    assert!(harness.audit.events().contains(&AuditEvent::LoginRefused {
        method: "discord",
        reason: "not_eligible",
        user: Some(NOROLE.to_string()),
    }));
    // The token was revoked every time.
    assert_eq!(harness.discord.revoked(), 3);
}

#[tokio::test]
async fn unavailable_member_data_refuses_sign_in_without_denying() {
    let harness = MemberHarness::new().await;
    harness.gate.force(Some(Eligibility::Unavailable));
    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    assert_eq!(login_error(&reply).as_deref(), Some("unavailable"));
    assert!(no_session_cookie(&reply));
}

/// Run `update` through the real roster hook right after the callback's
/// gate check passed, before its session insert.
fn roster_during_sign_in(harness: &MemberHarness, update: RosterUpdate) {
    let admin = harness.admin_auth.clone();
    let member = harness.member.clone();
    let store = harness.store.clone();
    harness.gate.after_next_check(async move {
        on_roster_update(&admin, Some(&member), &*store, None, &update)
            .await
            .unwrap();
    });
}

#[tokio::test]
async fn a_role_loss_or_leave_during_sign_in_leaves_no_live_session() {
    let lost_role = RosterUpdate::Seen {
        user_id: MIKAN.to_string(),
        display_name: "Mikan".into(),
        nickname: None,
        has_role: false,
        roles: Vec::new(),
        is_guild_admin: false,
    };
    let left = RosterUpdate::Left {
        user_id: MIKAN.to_string(),
    };
    for (update, case) in [(lost_role, "role lost"), (left, "left")] {
        let harness = MemberHarness::new().await;
        let earlier = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
        roster_during_sign_in(&harness, update);
        let reply = harness
            .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
            .await;
        assert_eq!(
            login_error(&reply).as_deref(),
            Some("not_eligible"),
            "{case}"
        );
        assert!(no_session_cookie(&reply), "{case}");
        assert!(harness.rows(MIKAN).await.is_empty(), "{case}");
        let (name, value) = earlier.cookie();
        let me = harness
            .get(
                "/api/public/session",
                &[(name, &value), ("CF-Connecting-IP", IP)],
            )
            .await;
        assert_eq!(me.status, 401, "{case}: the earlier session ended too");
        let succeeded = harness
            .audit
            .events()
            .into_iter()
            .filter(|event| matches!(event, AuditEvent::LoginSucceeded { .. }))
            .count();
        assert_eq!(succeeded, 1, "{case}: only the earlier sign-in");
        assert!(
            harness.audit.events().contains(&AuditEvent::LoginRefused {
                method: "discord",
                reason: "not_eligible",
                user: Some(MIKAN.to_string()),
            }),
            "{case}"
        );
    }
}

async fn session_reply(harness: &MemberHarness, id: &str, ip: &str) -> Reply {
    harness
        .get(
            "/api/public/session",
            &[(cookie(id).0, &cookie(id).1), ("CF-Connecting-IP", ip)],
        )
        .await
}

/// The sign-in's second gate check (after the insert) cannot read member data.
fn unavailable_after_insert(harness: &MemberHarness) {
    let gate = harness.gate.clone();
    harness
        .gate
        .after_next_check(async move { gate.force(Some(Eligibility::Unavailable)) });
}

fn capped_events(harness: &MemberHarness) -> usize {
    harness
        .audit
        .events()
        .iter()
        .filter(|event| {
            **event
                == AuditEvent::SessionEnded {
                    actor: format!("discord:{MIKAN}"),
                    reason: "capped",
                }
        })
        .count()
}

/// Unavailable member data never costs a session: the new one is handed out
/// due for its re-check, which the next request runs (503 while unreadable).
#[tokio::test]
async fn member_data_lost_during_sign_in_hands_out_a_session_due_for_recheck() {
    let harness = MemberHarness::new().await;
    unavailable_after_insert(&harness);
    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert!(wire::is_token(&id));
    let checks = harness.gate.checks();
    let next = session_reply(&harness, &id, IP).await;
    assert_eq!(
        (next.status, next.api_error()),
        (503, "auth_unavailable".into())
    );
    assert_eq!(harness.gate.checks(), checks + 1, "re-checked at once");
    assert_eq!(harness.rows(MIKAN).await.len(), 1, "the session is kept");
    harness.gate.force(None);
    assert_eq!(session_reply(&harness, &id, IP).await.status, 200);
}

#[tokio::test]
async fn a_replacing_sign_in_with_unavailable_member_data_keeps_the_browser_signed_in() {
    let harness = MemberHarness::new().await;
    let earlier = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    unavailable_after_insert(&harness);
    let (path, login) = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    let cookies = format!("{login}; {}={}", wire::MEMBER_SESSION_COOKIE, earlier.id);
    let reply = harness
        .get(&path, &[("Cookie", &cookies), ("CF-Connecting-IP", IP)])
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert_ne!(id, earlier.id, "the replacement is handed out");
    let rows = harness.rows(MIKAN).await;
    assert_eq!(rows.len(), 1, "exactly one live row for this browser");
    assert_eq!(rows[0].id_hash, crypto::sha256_hex(id.as_bytes()));
    assert_eq!(session_reply(&harness, &id, IP).await.status, 503);
    harness.gate.force(None);
    assert_eq!(session_reply(&harness, &id, IP).await.status, 200);
    assert_eq!(
        session_reply(&harness, &earlier.id, IP).await.status,
        401,
        "replaced, not lost"
    );
}

#[tokio::test]
async fn a_capped_sign_in_with_unavailable_member_data_hands_out_the_new_session() {
    let harness = MemberHarness::new().await;
    for n in 0..10 {
        harness.advance(TimeDelta::seconds(1));
        harness
            .sign_in_from(
                discord_user(MIKAN, "Mikan"),
                &format!("203.0.113.{}", n + 1),
            )
            .await;
    }
    harness.advance(TimeDelta::seconds(1));
    unavailable_after_insert(&harness);
    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    let hash = crypto::sha256_hex(id.as_bytes());
    let rows = harness.rows(MIKAN).await;
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().any(|row| row.id_hash == hash));
    assert_eq!(capped_events(&harness), 1, "the cap made room for it");
    let next = session_reply(&harness, &id, IP).await;
    assert_eq!(
        (next.status, next.api_error()),
        (503, "auth_unavailable".into())
    );
    let rows = harness.rows(MIKAN).await;
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().any(|row| row.id_hash == hash), "kept");
}

/// A role loss after the second check, before the cookie reaches the
/// browser, ends the new row; that cookie is refused on its first use.
#[tokio::test]
async fn a_role_loss_after_the_second_check_refuses_the_cookie_on_first_use() {
    let harness = MemberHarness::new().await;
    let gate = harness.gate.clone();
    let admin = harness.admin_auth.clone();
    let member = harness.member.clone();
    let store = harness.store.clone();
    harness.gate.after_next_check(async move {
        gate.after_next_check(async move {
            let lost = RosterUpdate::Seen {
                user_id: MIKAN.to_string(),
                display_name: "Mikan".into(),
                nickname: None,
                has_role: false,
                roles: Vec::new(),
                is_guild_admin: false,
            };
            on_roster_update(&admin, Some(&member), &*store, None, &lost)
                .await
                .unwrap();
        });
    });
    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    assert!(harness.rows(MIKAN).await.is_empty());
    let next = session_reply(&harness, &id, IP).await;
    assert_eq!(
        (next.status, next.api_error()),
        (401, "unauthenticated".into())
    );
}

#[tokio::test]
async fn tokens_are_revoked_and_no_secret_or_address_reaches_the_audit() {
    let harness = MemberHarness::new().await;
    let (path, login) = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    harness.discord.fail_revocation(true);
    let reply = harness
        .get(&path, &[("Cookie", &login), ("CF-Connecting-IP", IP)])
        .await;
    assert_eq!(reply.status, 200);
    let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
    harness.discord.fail_revocation(false);
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    assert_eq!(harness.discord.revoked(), 1);

    let state = path.split("state=").nth(1).unwrap();
    let code = path
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let login_value = login.split('=').nth(1).unwrap();
    let lines = harness.audit.lines();
    assert!(lines.iter().any(|line| line.contains("revoke_failed")));
    for line in &lines {
        for secret in [
            state,
            code,
            login_value,
            id.as_str(),
            browser.id.as_str(),
            browser.csrf.as_str(),
            IP,
            "127.0.0.1",
            "public-discord-client-secret",
        ] {
            assert!(!line.contains(secret), "{line} leaks {secret}");
        }
        assert!(line.contains("\"realm\":\"member\""), "{line}");
        // Only the keyed tag of the address (user decision 2026-10-09).
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        let client = record["client"].as_str().unwrap_or_default();
        assert!(
            client.len() == 64 && client.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{line}"
        );
    }
}

#[tokio::test]
async fn next_stays_a_same_origin_path() {
    let harness = MemberHarness::new().await;
    for (next, expected) in [
        ("%2Faccount%3Ftab%3Ddevices", "/account?tab=devices"),
        ("%2F%2Fevil.example%2F", "/"),
        ("%2F%5Cevil.example", "/"),
        ("https%3A%2F%2Fevil.example%2F", "/"),
        ("%2Fapi%2Fpublic%2Fauth%2Flogout", "/"),
    ] {
        let reply = harness
            .callback(discord_user(MIKAN, "Mikan"), next, IP)
            .await;
        assert_eq!(reply.destination().as_deref(), Some(expected), "{next}");
    }
}

#[tokio::test]
async fn discord_errors_are_never_echoed() {
    let harness = MemberHarness::new().await;
    let (path, login) = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    let state = path.split("state=").nth(1).unwrap();
    let denied = format!(
        "/api/public/auth/discord/callback?error=access_denied&error_description=%3Cscript%3Eevil%3C%2Fscript%3E&state={state}"
    );
    let reply = harness.get(&denied, &[("Cookie", &login)]).await;
    assert_eq!(login_error(&reply).as_deref(), Some("denied"));
    let dump = reply.dump();
    assert!(
        !dump.contains("evil") && !dump.contains("access_denied"),
        "{dump}"
    );
    assert!(
        harness
            .audit
            .lines()
            .iter()
            .all(|line| !line.contains("evil"))
    );
}

fn tight(route: Route) -> Limits {
    let rule = |burst| Rule {
        burst,
        per_minute: 1.0,
    };
    match route {
        Route::DiscordStart => Limits {
            per_ip: rule(2.0),
            global: rule(3.0),
        },
        _ => Limits {
            per_ip: rule(10.0),
            global: rule(10.0),
        },
    }
}

#[tokio::test]
async fn start_is_limited_per_client_address_and_globally() {
    let harness = MemberHarness::with(Options {
        limits: Some(tight),
        ..Options::default()
    })
    .await;
    let harness = &harness;
    let start = |ip: &'static str| async move {
        let reply = harness
            .get(
                "/api/public/auth/discord/start",
                &[("CF-Connecting-IP", ip)],
            )
            .await;
        reply.header("location").unwrap().to_owned()
    };
    assert!(
        start("203.0.113.1")
            .await
            .starts_with("https://discord.com/")
    );
    assert!(
        start("203.0.113.1")
            .await
            .starts_with("https://discord.com/")
    );
    assert_eq!(
        start("203.0.113.1").await,
        "/?login_error=rate_limited",
        "per IP"
    );
    assert!(
        start("203.0.113.2")
            .await
            .starts_with("https://discord.com/")
    );
    assert_eq!(
        start("203.0.113.3").await,
        "/?login_error=rate_limited",
        "global"
    );
    harness.advance(TimeDelta::minutes(5));
    assert!(
        start("203.0.113.3")
            .await
            .starts_with("https://discord.com/")
    );
    assert!(harness.audit.events().contains(&AuditEvent::RateLimited {
        route: "discord_start"
    }));
}

#[tokio::test]
async fn a_discord_429_cools_the_member_login_down() {
    let harness = MemberHarness::new().await;
    harness.discord.rate_limit_next(Duration::from_secs(30));
    let reply = harness
        .callback(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    assert_eq!(login_error(&reply).as_deref(), Some("unavailable"));
    let start = harness.get("/api/public/auth/discord/start", &[]).await;
    assert_eq!(login_error(&start).as_deref(), Some("unavailable"));
    let exchanges = harness.discord.exchanges();
    harness.advance(TimeDelta::seconds(31));
    harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    assert_eq!(harness.discord.exchanges(), exchanges + 1);
}

#[tokio::test]
async fn a_closed_portal_answers_closed_and_drops_the_pending_login() {
    let harness = MemberHarness::new().await;
    let browser = harness.sign_in(discord_user(MIKAN, "Mikan")).await;
    let (path, login) = harness
        .approved(discord_user(MIKAN, "Mikan"), "%2F", IP)
        .await;
    harness.set_open(false);

    let status = harness.get("/api/public/status", &[]).await;
    assert_eq!(status.json(), serde_json::json!({"portal": "closed"}));
    let start = harness.get("/api/public/auth/discord/start", &[]).await;
    assert_eq!(login_error(&start).as_deref(), Some("closed"));
    let callback = harness.get(&path, &[("Cookie", &login)]).await;
    assert_eq!(login_error(&callback).as_deref(), Some("closed"));
    assert!(no_session_cookie(&callback));
    for path in [
        "/api/public/session",
        "/api/public/sessions",
        "/api/public/session/avatar",
        "/api/public/week",
        "/api/public/me/allowance",
        "/api/public/timings",
        "/api/public/bosses",
        "/api/public/bosses/events",
        "/api/public/bosses/Carling/knowledge",
        "/api/public/timings/f-1/owner",
        "/api/public/owner-requests/r-1/accept",
        "/api/public/runs/r-1",
        "/api/public/runs/r-1/answer",
        "/api/public/runs/r-1/move",
        "/api/public/requests",
        "/api/public/requests/mine",
        "/api/public/requests/q-1/withdraw",
        "/api/public/anything",
        "/art/entry/Carling",
    ] {
        let reply = harness
            .get(path, &[(browser.cookie().0, &browser.cookie().1)])
            .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (503, "closed".into()),
            "{path}"
        );
    }
    // Closing neither deletes nor touches the session.
    assert_eq!(harness.rows(MIKAN).await.len(), 1);

    harness.set_open(true);
    assert_eq!(
        harness.get("/api/public/status", &[]).await.json(),
        serde_json::json!({"portal": "open"})
    );
    let replay = harness.get(&path, &[("Cookie", &login)]).await;
    assert_eq!(
        login_error(&replay).as_deref(),
        Some("state"),
        "the pending login was dropped while closed"
    );
    let reply = harness.get("/api/public/anything", &[]).await;
    assert_eq!((reply.status, reply.api_error()), (404, "not_found".into()));
    // Data and every art path now ask for the session instead.
    for path in [
        "/art/entry/Carling",
        "/art/anything/at/all",
        "/api/public/week",
        "/api/public/bosses",
        "/api/public/bosses/Carling/knowledge",
        "/api/public/runs/r-1",
        "/api/public/requests/mine",
    ] {
        let reply = harness.get(path, &[]).await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (401, "unauthenticated".into()),
            "{path}"
        );
    }
}

#[tokio::test]
async fn without_the_public_application_the_portal_stays_closed() {
    let harness = MemberHarness::with(Options {
        discord: false,
        ..Options::default()
    })
    .await;
    assert_eq!(
        harness.get("/api/public/status", &[]).await.json(),
        serde_json::json!({"portal": "closed"})
    );
    let start = harness.get("/api/public/auth/discord/start", &[]).await;
    assert_eq!(login_error(&start).as_deref(), Some("closed"));
    let reply = harness.get("/api/public/anything", &[]).await;
    assert_eq!(reply.api_error(), "closed");
}
