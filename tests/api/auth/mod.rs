//! Admin sign-in and sessions against fake Discord, a fake guild, the memory
//! session store and a pinned clock.

mod discord;
mod fallbacks;
mod hardening;
mod own_sessions;
mod roster;
mod sessions;

use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};

use axum::{Router, middleware::from_fn_with_state, routing::post};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade::{
    api::{
        auth::{
            AdminAuth, AdminSession,
            audit::RecordingAudit,
            crypto::SealedSecret,
            discord::{DiscordClient, DiscordLogin, DiscordUser, Secret},
            fake::{FakeDiscord, FakeGuild},
            rate::{Limits, RateLimits, Route},
            staff::{GuildMembers, GuildStaffGate, StoreGuildMembers},
            wire,
        },
        guard::proxy,
        listeners::Site,
        state::GuildAccess,
    },
    bot::commands::{AccessPolicy, Invoker},
    infrastructure::store::MemoryScheduleStore,
};
use twilight_model::id::Id;

use crate::support::{self, ADMIN_HOST, Fixture, Reply, request, send, spawn_router};

pub const TOKEN: &str = "break-glass-token-with-at-least-32-bytes!";
pub const CLIENT_SECRET: &str = "discord-client-secret";
pub const REDIRECT: &str = "https://kanade.test/api/admin/auth/discord/callback";
pub const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
pub const ADMIN_ROLE: u64 = 20;
pub const TAILSCALE_ADMIN: &str = "ops@example.com";
pub const EDGE_SECRET: &str = "edge-secret-shared-with-the-caddy-edge!!";
/// What the authenticated edge adds to every request it relays.
pub const EDGE_AUTH: (&str, &str) = ("X-Kanade-Edge-Auth", EDGE_SECRET);
/// The client address the edge saw (a tailnet IP).
pub const EDGE_XFF: (&str, &str) = ("X-Forwarded-For", "100.64.0.7");

pub struct Harness {
    pub admin: SocketAddr,
    pub public: SocketAddr,
    pub discord: Arc<FakeDiscord>,
    pub guild: Arc<FakeGuild>,
    pub audit: Arc<RecordingAudit>,
    pub store: Arc<MemoryScheduleStore>,
    /// Owner and policy the store-backed staff gate reads (`store_gated`).
    pub access: Arc<GuildAccess>,
    clock: Arc<Mutex<DateTime<Utc>>>,
    pub site: Site,
    _fixture: Fixture,
}

pub fn user(id: u64, name: &str) -> DiscordUser {
    DiscordUser {
        id: id.to_string(),
        username: name.to_lowercase(),
        global_name: Some(name.into()),
        bot: false,
        avatar: None,
    }
}

pub fn member(id: u64, roles: &[u64], admin: bool) -> Invoker {
    Invoker {
        user_id: Id::new(id),
        roles: roles.iter().map(|role| Id::new(*role)).collect(),
        is_guild_admin: admin,
    }
}

impl Harness {
    pub async fn new() -> Self {
        Self::with(None, TOKEN).await
    }

    /// `edge`: the trusted proxy peer (the test client is always 127.0.0.1).
    pub async fn with(edge: Option<IpAddr>, token: &str) -> Self {
        Self::sharing(edge, token, Arc::new(MemoryScheduleStore::new())).await
    }

    pub async fn sharing(
        edge: Option<IpAddr>,
        token: &str,
        store: Arc<MemoryScheduleStore>,
    ) -> Self {
        Self::build(edge, token, store, None, false).await
    }

    /// The staff gate reads the persisted member rows, as in production.
    pub async fn store_gated() -> Self {
        Self::build(
            None,
            TOKEN,
            Arc::new(MemoryScheduleStore::new()),
            None,
            true,
        )
        .await
    }

    /// With custom rate limits.
    pub async fn limited(limits: fn(Route) -> Limits) -> Self {
        Self::build(
            None,
            TOKEN,
            Arc::new(MemoryScheduleStore::new()),
            Some(limits),
            false,
        )
        .await
    }

    async fn build(
        edge: Option<IpAddr>,
        token: &str,
        store: Arc<MemoryScheduleStore>,
        limits: Option<fn(Route) -> Limits>,
        store_gate: bool,
    ) -> Self {
        let fixture = Fixture::new();
        let mut http = fixture.http();
        http.trusted_proxy = edge;
        let discord = Arc::new(FakeDiscord::default());
        let guild = Arc::new(FakeGuild::default());
        let audit = Arc::new(RecordingAudit::default());
        let clock = Arc::new(Mutex::new(
            Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap(),
        ));
        let access = Arc::new(GuildAccess::new(
            AccessPolicy {
                bossing_role_id: Id::new(10),
                admin_role_id: Some(Id::new(ADMIN_ROLE)),
                debug_user_ids: Vec::new(),
            },
            None,
        ));
        let members: Arc<dyn GuildMembers> = if store_gate {
            Arc::new(StoreGuildMembers::new(store.clone(), access.clone()))
        } else {
            guild.clone()
        };
        let staff = Arc::new(GuildStaffGate::new(access.policy.clone(), members));
        let now = clock.clone();
        let auth = AdminAuth::new(store.clone(), staff)
            .with_discord(DiscordLogin::new(
                DiscordClient {
                    client_id: "4242".into(),
                    client_secret: Secret::new(CLIENT_SECRET),
                    redirect_uri: REDIRECT.into(),
                },
                discord.clone(),
            ))
            .with_tailscale_logins([TAILSCALE_ADMIN.to_owned()])
            .with_breakglass(token.as_bytes())
            .unwrap()
            .with_clock(Arc::new(move || *now.lock().unwrap()))
            .with_audit(audit.clone());
        let auth = match limits {
            Some(limits) => auth.with_rate_limits(RateLimits::new(limits)),
            None => auth,
        };
        let mut site = Site::admin(&http);
        site.auth = Some(Arc::new(auth));
        if edge.is_some() {
            site.edge_secret = Some(Arc::new(SealedSecret::new(EDGE_SECRET.as_bytes()).unwrap()));
        }
        let admin = support::spawn(site.clone()).await;
        let mut public_site = Site::public(&http).unwrap();
        // Even a misassigned auth must mean nothing on the public origin.
        public_site.auth = site.auth.clone();
        let public = support::spawn(public_site).await;
        Self {
            admin,
            public,
            discord,
            guild,
            audit,
            store,
            access,
            clock,
            site,
            _fixture: fixture,
        }
    }

    pub fn advance(&self, by: TimeDelta) {
        *self.clock.lock().unwrap() += by;
    }

    pub async fn get(&self, path: &str, extra: &[(&str, &str)]) -> Reply {
        request(self.admin, "GET", ADMIN_HOST, path, extra).await
    }

    pub async fn post(&self, path: &str, extra: &[(&str, &str)], json: Option<&str>) -> Reply {
        send(self.admin, "POST", ADMIN_HOST, path, extra, json).await
    }

    /// Break-glass login; returns the session cookie value and CSRF token.
    pub async fn token_login(&self) -> (String, String) {
        let reply = self
            .post(
                "/api/admin/auth/token",
                &[ORIGIN],
                Some(&format!(r#"{{"token":"{TOKEN}"}}"#)),
            )
            .await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        (
            reply.cookie(wire::SESSION_COOKIE).unwrap(),
            reply.header("x-kanade-csrf").unwrap().to_owned(),
        )
    }

    /// Full Discord login for `user`.
    pub async fn discord_login(&self, user: DiscordUser, next: &str) -> Reply {
        let (path, login_cookie) = self.discord_approved(user, next).await;
        self.get(&path, &[("Cookie", &login_cookie)]).await
    }

    /// Start a login and approve it on Discord: the callback path and pre-auth cookie.
    pub async fn discord_approved(&self, user: DiscordUser, next: &str) -> (String, String) {
        let start = self
            .get(&format!("/api/admin/auth/discord/start?next={next}"), &[])
            .await;
        assert_eq!(start.status, 303);
        let login = start.cookie(wire::LOGIN_COOKIE).unwrap();
        let location = start.header("location").unwrap().to_owned();
        let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
        let state = wire::query_value(&pairs, "state").unwrap();
        let challenge = wire::query_value(&pairs, "code_challenge").unwrap();
        let code = format!("code-{}", user.id);
        self.discord.approve(&code, &challenge, user);
        (
            format!(
                "/api/admin/auth/discord/callback?code={code}&state={}",
                wire::encode(&state)
            ),
            format!("{}={login}", wire::LOGIN_COOKIE),
        )
    }
}

pub fn cookie(value: &str) -> (&'static str, String) {
    ("Cookie", format!("{}={value}", wire::SESSION_COOKIE))
}

/// Echoes every header a handler behind the proxy guard can see.
pub async fn header_probe(site: Site) -> SocketAddr {
    let site = Arc::new(site);
    let router = Router::new()
        .route(
            "/headers",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                headers
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.to_str().unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        )
        .layer(from_fn_with_state(site, proxy::sanitize));
    spawn_router(router).await
}

/// A mutation route standing in for later admin endpoints: returns the actor id.
pub async fn actor_probe(site: Site) -> SocketAddr {
    let site = Arc::new(site);
    let router = Router::new()
        .route(
            "/probe",
            post(|session: AdminSession| async move { session.actor.id().to_owned() }),
        )
        .with_state(site.clone())
        .layer(from_fn_with_state(site, proxy::sanitize));
    spawn_router(router).await
}
