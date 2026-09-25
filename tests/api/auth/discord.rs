//! Discord OAuth: success, staff denial, state and code replay, redirects.

use kanade::api::auth::wire;

use super::{ADMIN_ROLE, CLIENT_SECRET, Harness, REDIRECT, cookie, member, user};

#[tokio::test]
async fn staff_sign_in_with_pkce_uses_the_token_once_and_lands_on_next() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));

    let start = harness
        .get("/api/admin/auth/discord/start?next=%2Fweek", &[])
        .await;
    assert_eq!(start.status, 303);
    let login_cookie = start
        .all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with("__Host-kanade_admin_login="))
        .unwrap()
        .to_owned();
    assert!(login_cookie.ends_with("; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=600"));
    let location = start.header("location").unwrap();
    assert!(location.starts_with("https://discord.com/oauth2/authorize?"));
    assert!(location.contains("scope=identify&"));
    assert!(location.contains("code_challenge_method=S256"));
    assert!(!location.contains(CLIENT_SECRET));

    let callback = harness.discord_login(user(111, "Alice"), "%2Fweek").await;
    // A same-origin landing page, so the Strict cookie rides the next navigation.
    assert_eq!(callback.status, 200, "{}", callback.dump());
    assert_eq!(
        callback.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(callback.header("cache-control"), Some("no-store"));
    assert_eq!(callback.header("referrer-policy"), Some("no-referrer"));
    assert!(!callback.text().contains("<script"));
    assert_eq!(callback.destination().as_deref(), Some("/week"));
    let session_line = callback
        .all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with("__Host-kanade_admin="))
        .unwrap()
        .to_owned();
    assert!(session_line.ends_with("; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=43200"));
    assert_eq!(
        callback.cookie(wire::LOGIN_COOKIE).as_deref(),
        Some(""),
        "pre-auth cookie cleared"
    );

    // Two starts, one exchange; the token was used for one call and revoked.
    assert_eq!(harness.discord.exchanges(), 1);
    assert_eq!(harness.discord.revoked(), 1);
    assert_eq!(harness.discord.live_tokens(), 0);
    assert_eq!(harness.discord.redirect_uris(), [REDIRECT]);

    let session_id = callback.cookie(wire::SESSION_COOKIE).unwrap();
    let (name, value) = cookie(&session_id);
    let me = harness.get("/api/admin/session", &[(name, &value)]).await;
    assert_eq!(me.status, 200);
    assert_eq!(me.json(), serde_json::json!({"display": "Alice"}));
    assert_eq!(me.header("x-kanade-csrf").map(str::len), Some(43));
}

#[tokio::test]
async fn members_without_a_staff_grant_are_refused() {
    let harness = Harness::new().await;
    harness.guild.put(member(333, &[10], false));
    // 222 is not in the guild at all.
    for (id, name) in [(222, "Stranger"), (333, "Bosser")] {
        let reply = harness.discord_login(user(id, name), "%2F").await;
        assert_eq!(
            reply.destination().as_deref(),
            Some("/?login_error=forbidden"),
            "{name}"
        );
        assert_eq!(reply.cookie(wire::SESSION_COOKIE), None);
    }
    assert_eq!(
        harness.discord.revoked(),
        2,
        "tokens are revoked even when refused"
    );

    // The bot's staff rule: the guild owner and Administrator also qualify.
    harness.guild.put(member(444, &[], false));
    harness
        .guild
        .set_owner(Some(twilight_model::id::Id::new(444)));
    harness.guild.put(member(555, &[], true));
    for (id, name) in [(444, "Owner"), (555, "Administrator")] {
        let reply = harness.discord_login(user(id, name), "%2F").await;
        assert_eq!(reply.destination().as_deref(), Some("/"), "{name}");
        assert!(reply.cookie(wire::SESSION_COOKIE).is_some());
    }

    harness.guild.set_unavailable(true);
    harness.guild.put(member(666, &[ADMIN_ROLE], false));
    let reply = harness.discord_login(user(666, "Later"), "%2F").await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=unavailable")
    );
}

#[tokio::test]
async fn bad_missing_or_replayed_state_and_codes_are_refused() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));

    let start = harness.get("/api/admin/auth/discord/start", &[]).await;
    let login = start.cookie(wire::LOGIN_COOKIE).unwrap();
    let location = start.header("location").unwrap().to_owned();
    let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
    let state = wire::encode(&wire::query_value(&pairs, "state").unwrap());
    let challenge = wire::query_value(&pairs, "code_challenge").unwrap();
    harness
        .discord
        .approve("code-a", &challenge, user(111, "Alice"));
    let login_cookie = format!("{}={login}", wire::LOGIN_COOKIE);

    // No pre-auth cookie: the state cannot be bound to this browser.
    let reply = harness
        .get(
            &format!("/api/admin/auth/discord/callback?code=code-a&state={state}"),
            &[],
        )
        .await;
    assert_eq!(reply.destination().as_deref(), Some("/?login_error=state"));

    // Missing or forged state consumes the pending login.
    for query in ["code=code-a", "code=code-a&state=forged"] {
        let reply = harness
            .get(
                &format!("/api/admin/auth/discord/callback?{query}"),
                &[("Cookie", &login_cookie)],
            )
            .await;
        assert_eq!(
            reply.destination().as_deref(),
            Some("/?login_error=state"),
            "{query}"
        );
    }
    let reply = harness
        .get(
            &format!("/api/admin/auth/discord/callback?code=code-a&state={state}"),
            &[("Cookie", &login_cookie)],
        )
        .await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=state"),
        "one-time"
    );
    assert_eq!(harness.discord.exchanges(), 0, "no code was exchanged");

    // A successful callback replayed verbatim is refused before Discord is asked again.
    let (path, login_cookie) = harness.discord_approved(user(111, "Alice"), "%2F").await;
    let first = harness.get(&path, &[("Cookie", &login_cookie)]).await;
    assert_eq!(first.destination().as_deref(), Some("/"));
    assert_eq!(harness.discord.exchanges(), 1);
    let replay = harness.get(&path, &[("Cookie", &login_cookie)]).await;
    assert_eq!(replay.destination().as_deref(), Some("/?login_error=state"));
    assert_eq!(harness.discord.exchanges(), 1);

    // A code issued for another login's PKCE challenge is refused by Discord; generic failure.
    let start = harness.get("/api/admin/auth/discord/start", &[]).await;
    let login = start.cookie(wire::LOGIN_COOKIE).unwrap();
    let location = start.header("location").unwrap().to_owned();
    let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
    let state = wire::encode(&wire::query_value(&pairs, "state").unwrap());
    let reply = harness
        .get(
            &format!("/api/admin/auth/discord/callback?code=code-a&state={state}"),
            &[("Cookie", &format!("{}={login}", wire::LOGIN_COOKIE))],
        )
        .await;
    assert_eq!(
        reply.destination().as_deref(),
        Some("/?login_error=discord")
    );

    // The user cancelled on Discord.
    let start = harness.get("/api/admin/auth/discord/start", &[]).await;
    let login = start.cookie(wire::LOGIN_COOKIE).unwrap();
    let location = start.header("location").unwrap().to_owned();
    let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
    let state = wire::encode(&wire::query_value(&pairs, "state").unwrap());
    let reply = harness
        .get(
            &format!("/api/admin/auth/discord/callback?error=access_denied&state={state}"),
            &[("Cookie", &format!("{}={login}", wire::LOGIN_COOKIE))],
        )
        .await;
    assert_eq!(reply.destination().as_deref(), Some("/?login_error=denied"));
}

#[tokio::test]
async fn next_is_restricted_to_same_origin_paths() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    for next in [
        "https%3A%2F%2Fevil.example%2F",
        "%2F%2Fevil.example",
        "%2F%5Cevil.example",
        "%2Fapi%2Fadmin%2Fauth%2Flogout",
    ] {
        let reply = harness.discord_login(user(111, "Alice"), next).await;
        assert_eq!(reply.destination().as_deref(), Some("/"), "{next}");
    }
}
