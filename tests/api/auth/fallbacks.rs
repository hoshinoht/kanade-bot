//! Tailscale identity from the edge, the break-glass token, and secrets
//! staying out of logs and responses.

use kanade::{
    api::auth::{audit::AuditEvent, crypto, wire},
    infrastructure::store::web_sessions::{LoginMethod, WebSessionStore},
};

use super::{
    ADMIN_ROLE, CLIENT_SECRET, EDGE_AUTH, EDGE_XFF, Harness, ORIGIN, TAILSCALE_ADMIN, TOKEN,
    cookie, member, user,
};

const EDGE: [u8; 4] = [127, 0, 0, 1];

#[tokio::test]
async fn tailscale_sign_in_needs_the_trusted_edge_and_an_allow_listed_login() {
    let harness = Harness::with(Some(EDGE.into()), TOKEN).await;
    let methods = harness
        .get(
            "/api/admin/auth/methods",
            &[
                EDGE_AUTH,
                EDGE_XFF,
                ("Tailscale-User-Login", TAILSCALE_ADMIN),
            ],
        )
        .await;
    assert_eq!(
        methods.json(),
        serde_json::json!({"discord": true, "tailscale": true, "token": true})
    );

    let refused = harness
        .post(
            "/api/admin/auth/tailscale",
            &[
                ORIGIN,
                EDGE_AUTH,
                EDGE_XFF,
                ("Tailscale-User-Login", "intruder@example.com"),
            ],
            None,
        )
        .await;
    assert_eq!(refused.status, 401);

    let login = harness
        .post(
            "/api/admin/auth/tailscale",
            &[
                ORIGIN,
                EDGE_AUTH,
                EDGE_XFF,
                ("Tailscale-User-Login", TAILSCALE_ADMIN),
                ("Tailscale-User-Name", "Ops Person"),
            ],
            None,
        )
        .await;
    assert_eq!(login.status, 200);
    assert_eq!(
        login.json(),
        serde_json::json!({"display": "Ops Person", "method": "tailscale"})
    );
    let id = login.cookie(wire::SESSION_COOKIE).unwrap();
    let (name, value) = cookie(&id);
    let with_identity = [
        (name, value.as_str()),
        EDGE_AUTH,
        EDGE_XFF,
        ("Tailscale-User-Login", TAILSCALE_ADMIN),
    ];
    assert_eq!(
        harness
            .get("/api/admin/session", &with_identity)
            .await
            .status,
        200
    );

    // A different identity on the same cookie ends the session.
    let swapped = harness
        .get(
            "/api/admin/session",
            &[
                (name, &value),
                EDGE_AUTH,
                EDGE_XFF,
                ("Tailscale-User-Login", "other@example.com"),
            ],
        )
        .await;
    assert_eq!(swapped.status, 401);
    assert_eq!(
        harness
            .get("/api/admin/session", &with_identity)
            .await
            .status,
        401
    );

    // Login CSRF: a cross-site page cannot sign the browser in.
    let cross = harness
        .post(
            "/api/admin/auth/tailscale",
            &[
                ("Origin", "https://evil.example"),
                EDGE_AUTH,
                EDGE_XFF,
                ("Tailscale-User-Login", TAILSCALE_ADMIN),
            ],
            None,
        )
        .await;
    assert_eq!((cross.status, cross.api_error()), (403, "csrf".into()));
}

#[tokio::test]
async fn tailscale_headers_need_both_the_edge_peer_and_the_edge_secret() {
    // Same address as the edge but no or a wrong secret: any local process looks like this.
    let same_peer = Harness::with(Some(EDGE.into()), TOKEN).await;
    // Right secret from the wrong peer.
    let other_peer = Harness::with(Some([127, 0, 0, 2].into()), TOKEN).await;
    for (case, harness, secret) in [
        ("no secret", &same_peer, None),
        (
            "wrong secret",
            &same_peer,
            Some("not-the-edge-secret-at-all-no-no-no!!"),
        ),
        ("edge secret twice", &same_peer, Some("twice")),
        ("wrong peer", &other_peer, Some(EDGE_AUTH.1)),
    ] {
        let mut extra = vec![ORIGIN, ("Tailscale-User-Login", TAILSCALE_ADMIN)];
        match secret {
            Some("twice") => extra.extend([EDGE_AUTH, EDGE_AUTH]),
            Some(value) => extra.push((EDGE_AUTH.0, value)),
            None => {}
        }
        let methods = harness.get("/api/admin/auth/methods", &extra).await;
        assert_eq!(methods.json()["tailscale"], false, "{case}");
        let reply = harness
            .post("/api/admin/auth/tailscale", &extra, None)
            .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (401, "unauthenticated".into()),
            "{case}"
        );
        assert!(reply.cookie(wire::SESSION_COOKIE).is_none(), "{case}");
    }
    let refused = same_peer.audit.records();
    let refusal = refused
        .iter()
        .find(|record| {
            record.event
                == AuditEvent::LoginRefused {
                    method: "tailscale",
                    reason: "no_identity",
                    user: None,
                }
        })
        .unwrap();
    assert_eq!(refusal.client.as_deref(), Some("127.0.0.1"));
    assert_eq!(refusal.request_id.len(), 16);
}

#[tokio::test]
async fn the_authenticated_edge_must_name_the_client() {
    let harness = Harness::with(Some(EDGE.into()), TOKEN).await;
    for xff in [None, Some("not-an-ip"), Some("")] {
        let mut extra = vec![EDGE_AUTH, ("Tailscale-User-Login", TAILSCALE_ADMIN)];
        if let Some(value) = xff {
            extra.push(("X-Forwarded-For", value));
        }
        let reply = harness.get("/api/admin/auth/methods", &extra).await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (400, "bad_forwarding".into()),
            "{xff:?}"
        );
    }
    // Without the secret nothing is vouched for, so no forwarding is needed.
    assert_eq!(
        harness.get("/api/admin/auth/methods", &[]).await.status,
        200
    );
    // The named client is the one rate limits and audit see.
    let reply = harness
        .post(
            "/api/admin/auth/tailscale",
            &[
                ORIGIN,
                EDGE_AUTH,
                EDGE_XFF,
                ("Tailscale-User-Login", "intruder@example.com"),
            ],
            None,
        )
        .await;
    assert_eq!(reply.status, 401);
    let refusal = harness.audit.records().pop().unwrap();
    assert_eq!(refusal.client.as_deref(), Some(EDGE_XFF.1));
}

#[tokio::test]
async fn the_edge_secret_never_reaches_handlers() {
    let harness = Harness::with(Some(EDGE.into()), TOKEN).await;
    let probe = super::header_probe(harness.site.clone()).await;
    let seen = crate::support::request(
        probe,
        "GET",
        crate::support::ADMIN_HOST,
        "/headers",
        &[
            EDGE_AUTH,
            EDGE_XFF,
            ("Tailscale-User-Login", TAILSCALE_ADMIN),
        ],
    )
    .await
    .text();
    assert!(!seen.contains("x-kanade-edge-auth"), "{seen}");
    assert!(!seen.contains(EDGE_AUTH.1), "{seen}");
    assert!(seen.contains("tailscale-user-login"), "{seen}");
}

#[tokio::test]
async fn break_glass_works_by_login_and_bearer_and_is_logged_loudly() {
    let harness = Harness::new().await;
    for (case, extra, body) in [
        (
            "wrong token",
            vec![ORIGIN],
            r#"{"token":"guess"}"#.to_owned(),
        ),
        (
            "no origin markers",
            vec![],
            format!(r#"{{"token":"{TOKEN}"}}"#),
        ),
        (
            "unknown field",
            vec![ORIGIN],
            format!(r#"{{"token":"{TOKEN}","x":1}}"#),
        ),
    ] {
        let reply = harness
            .post("/api/admin/auth/token", &extra, Some(&body))
            .await;
        assert!(
            matches!(reply.status, 400 | 401 | 403),
            "{case}: {}",
            reply.status
        );
        assert!(reply.cookie(wire::SESSION_COOKIE).is_none(), "{case}");
    }
    let (id, _) = harness.token_login().await;
    let (name, value) = cookie(&id);
    let me = harness.get("/api/admin/session", &[(name, &value)]).await;
    assert_eq!(
        me.json(),
        serde_json::json!({"display": "Break-glass token", "method": "token"})
    );
    // Only the SHA-256 of the cookie is stored.
    let stored = harness
        .store
        .load_session(&crypto::sha256_hex(id.as_bytes()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.method, LoginMethod::Token);
    assert_ne!(stored.subject, TOKEN);
    assert_eq!(harness.store.load_session(&id).await.unwrap(), None);

    let bearer = format!("Bearer {TOKEN}");
    let me = harness
        .get("/api/admin/session", &[("Authorization", &bearer)])
        .await;
    assert_eq!(me.status, 200);
    assert_eq!(
        me.header("x-kanade-csrf"),
        None,
        "bearer calls have no session to bind"
    );
    let wrong = harness
        .get(
            "/api/admin/session",
            &[("Authorization", "Bearer nope"), (name, &value)],
        )
        .await;
    assert_eq!(
        wrong.status, 401,
        "a bad bearer never falls back to the cookie"
    );

    let events = harness.audit.events();
    let loud = events
        .iter()
        .filter(|event| matches!(event, AuditEvent::BreakGlassUsed { .. }))
        .count();
    assert_eq!(loud, 2, "{events:?}");
}

#[tokio::test]
async fn tokens_secrets_codes_and_session_ids_never_reach_logs_or_bodies() {
    let harness = Harness::new().await;
    harness.guild.put(member(111, &[ADMIN_ROLE], false));
    let mut seen = Vec::new();

    let discord = harness.discord_login(user(111, "Alice"), "%2F").await;
    let discord_id = discord.cookie(wire::SESSION_COOKIE).unwrap();
    seen.push(discord.text());
    seen.push(discord.header("location").unwrap_or_default().to_owned());
    let refused = harness.discord_login(user(222, "Stranger"), "%2F").await;
    seen.push(refused.dump());

    let bad = harness
        .post(
            "/api/admin/auth/token",
            &[ORIGIN],
            Some(r#"{"token":"guess-guess"}"#),
        )
        .await;
    seen.push(bad.dump());
    let (token_id, _) = harness.token_login().await;
    let bearer = format!("Bearer {TOKEN}");
    seen.push(
        harness
            .get("/api/admin/session", &[("Authorization", &bearer)])
            .await
            .dump(),
    );

    let lines = harness.audit.lines();
    assert!(!lines.is_empty());
    for text in lines.iter().chain(&seen) {
        for secret in [
            TOKEN,
            CLIENT_SECRET,
            "guess-guess",
            "code-111",
            "code-222",
            discord_id.as_str(),
            token_id.as_str(),
        ] {
            assert!(!text.contains(secret), "{secret:?} leaked into {text}");
        }
    }
    assert!(lines.iter().any(|line| line.contains("discord:111")));
}
