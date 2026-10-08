//! Member (public origin) sign-in harness: the real public router with the
//! member realm over the memory store (sessions optionally on SQLite), fake
//! Discord, the store-backed eligibility gate (overridable, with a one-shot
//! hook after a check), a live portal switch and a pinned clock.
//! The admin listener runs beside it with its own realm, so isolation can be
//! checked in both directions.

use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade::{
    api::{
        auth::{
            AdminAuth,
            audit::RecordingAudit,
            discord::{DiscordClient, DiscordLogin, DiscordUser, Secret},
            fake::{FakeDiscord, FakeGuild},
            member::{Eligibility, EligibilityGate, MemberAuth, StoreEligibility},
            rate::{Limits, RateLimits},
            staff::{GateFuture, GuildStaffGate},
            wire,
        },
        listeners::Site,
    },
    bot::commands::AccessPolicy,
    domain::members::{GatewayMember, MemberStore},
    infrastructure::store::{
        MemoryScheduleStore, SqliteStore, SqliteStoreConfig,
        web_sessions::{LoginMethod, SessionOrigin, WebSession, WebSessionStore},
    },
};
use twilight_model::id::Id;

use super::TOKEN;
use crate::reads::TempDir;
use crate::support::{self, ADMIN_HOST, Fixture, PUBLIC_HOST, Reply, send};

pub const PUBLIC_CLIENT_ID: &str = "5151";
pub const PUBLIC_SECRET: &str = "public-discord-client-secret";
pub const PUBLIC_REDIRECT: &str = "https://kanade-pub.test/api/public/auth/discord/callback";
pub const PUB_ORIGIN: (&str, &str) = ("Origin", "https://kanade-pub.test");
/// An eligible member (bossing role) and one without the role.
pub const MIKAN: u64 = 100_000_000_000_000_001;
pub const NOROLE: u64 = 100_000_000_000_000_002;

type Race = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The store-backed gate, overridable to answer one fixed result; counts checks.
pub struct TestGate {
    store: StoreEligibility<Arc<MemoryScheduleStore>>,
    fixed: Mutex<Option<Eligibility>>,
    checks: AtomicU32,
    race: Mutex<Option<Race>>,
}

impl TestGate {
    pub fn force(&self, answer: Option<Eligibility>) {
        *self.fixed.lock().unwrap() = answer;
    }

    pub fn checks(&self) -> u32 {
        self.checks.load(Ordering::SeqCst)
    }

    /// Run `race` once, right after the next check has decided its answer
    /// and before the caller sees it (e.g. a roster event landing between a
    /// sign-in's gate check and its session insert).
    pub fn after_next_check(&self, race: impl Future<Output = ()> + Send + 'static) {
        *self.race.lock().unwrap() = Some(Box::pin(race));
    }
}

impl EligibilityGate for TestGate {
    fn check<'a>(&'a self, user_id: &'a str) -> GateFuture<'a, Eligibility> {
        Box::pin(async move {
            self.checks.fetch_add(1, Ordering::SeqCst);
            let fixed = *self.fixed.lock().unwrap();
            let answer = match fixed {
                Some(answer) => answer,
                None => self.store.check(user_id).await,
            };
            let race = self.race.lock().unwrap().take();
            if let Some(race) = race {
                race.await;
            }
            answer
        })
    }
}

pub struct Options {
    /// The cloudflared peer (the test client is always 127.0.0.1).
    pub cloudflared: Option<IpAddr>,
    pub limits: Option<fn(kanade::api::auth::rate::Route) -> Limits>,
    /// The public application is configured.
    pub discord: bool,
    /// Sessions on a fresh SQLite store instead of the memory store.
    pub sqlite: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cloudflared: Some([127, 0, 0, 1].into()),
            limits: None,
            discord: true,
            sqlite: false,
        }
    }
}

pub struct MemberHarness {
    pub public: SocketAddr,
    pub admin: SocketAddr,
    pub discord: Arc<FakeDiscord>,
    pub audit: Arc<RecordingAudit>,
    pub store: Arc<MemoryScheduleStore>,
    /// The member realm's session store (`store` unless built with `sqlite`).
    pub sessions: Arc<dyn WebSessionStore>,
    pub gate: Arc<TestGate>,
    pub member: Arc<MemberAuth>,
    pub admin_auth: Arc<AdminAuth>,
    open: Arc<AtomicBool>,
    clock: Arc<Mutex<DateTime<Utc>>>,
    _fixture: Fixture,
    _dir: Option<TempDir>,
}

/// A signed-in browser: its session cookie value and CSRF token.
#[derive(Clone, Debug)]
pub struct Browser {
    pub id: String,
    pub csrf: String,
}

impl Browser {
    pub fn cookie(&self) -> (&'static str, String) {
        cookie(&self.id)
    }
}

pub fn cookie(id: &str) -> (&'static str, String) {
    ("Cookie", format!("{}={id}", wire::MEMBER_SESSION_COOKIE))
}

pub fn discord_user(id: u64, name: &str) -> DiscordUser {
    DiscordUser {
        id: id.to_string(),
        username: name.to_lowercase(),
        global_name: Some(name.into()),
        bot: false,
        avatar: Some("0123456789abcdef0123456789abcdef".into()),
    }
}

impl MemberHarness {
    pub async fn new() -> Self {
        Self::with(Options::default()).await
    }

    pub async fn with(options: Options) -> Self {
        let fixture = Fixture::new();
        let mut http = fixture.http();
        http.cloudflared_peer = options.cloudflared;
        let store = Arc::new(MemoryScheduleStore::new());
        let discord = Arc::new(FakeDiscord::default());
        let audit = Arc::new(RecordingAudit::default());
        let clock = Arc::new(Mutex::new(
            Utc.with_ymd_and_hms(2026, 10, 8, 4, 0, 0).unwrap(),
        ));
        let open = Arc::new(AtomicBool::new(true));
        let gate = Arc::new(TestGate {
            store: StoreEligibility::new(store.clone()),
            fixed: Mutex::new(None),
            checks: AtomicU32::new(0),
            race: Mutex::new(None),
        });
        let (sessions, dir): (Arc<dyn WebSessionStore>, _) = if options.sqlite {
            let dir = TempDir::new();
            let sqlite = SqliteStore::open(&SqliteStoreConfig {
                db_path: dir.0.join("kanade.sqlite3"),
                owner_lock_dir: dir.0.join("locks"),
            })
            .await
            .unwrap();
            (Arc::new(sqlite), Some(dir))
        } else {
            (store.clone(), None)
        };
        let now = clock.clone();
        let switch = open.clone();
        let mut member = MemberAuth::new(
            sessions.clone(),
            gate.clone(),
            Arc::new(move || switch.load(Ordering::SeqCst)),
        )
        .with_clock(Arc::new(move || *now.lock().unwrap()))
        .with_audit(audit.clone());
        if options.discord {
            member = member.with_discord(DiscordLogin::new(
                DiscordClient {
                    client_id: PUBLIC_CLIENT_ID.into(),
                    client_secret: Secret::new(PUBLIC_SECRET),
                    redirect_uri: PUBLIC_REDIRECT.into(),
                },
                discord.clone(),
            ));
        }
        if let Some(limits) = options.limits {
            member = member.with_rate_limits(RateLimits::new(limits));
        }
        let member = Arc::new(member);

        // The admin realm beside it, signing in with the break-glass token.
        let guild = Arc::new(FakeGuild::default());
        let staff = Arc::new(GuildStaffGate::new(
            AccessPolicy {
                bossing_role_id: Id::new(10),
                admin_role_id: Some(Id::new(20)),
                debug_user_ids: Vec::new(),
            },
            guild,
        ));
        let now = clock.clone();
        let admin_auth = Arc::new(
            AdminAuth::new(store.clone(), staff)
                .with_breakglass(TOKEN.as_bytes())
                .unwrap()
                .with_clock(Arc::new(move || *now.lock().unwrap())),
        );

        let mut public_site = Site::public(&http).unwrap();
        public_site.member = Some(member.clone());
        // Misassigned admin credentials must mean nothing on the public origin.
        public_site.auth = Some(admin_auth.clone());
        let public = support::spawn(public_site).await;
        let mut admin_site = Site::admin(&http);
        admin_site.auth = Some(admin_auth.clone());
        // Nor a misassigned member realm on the admin origin.
        admin_site.member = Some(member.clone());
        let admin = support::spawn(admin_site).await;

        let harness = Self {
            public,
            admin,
            discord,
            audit,
            store,
            sessions,
            gate,
            member,
            admin_auth,
            open,
            clock,
            _fixture: fixture,
            _dir: dir,
        };
        harness.roster(MIKAN, true).await;
        harness.roster(NOROLE, false).await;
        harness
    }

    pub fn now(&self) -> DateTime<Utc> {
        *self.clock.lock().unwrap()
    }

    /// The realm's pinned clock, for probes that need "now".
    pub fn clock(&self) -> Arc<Mutex<DateTime<Utc>>> {
        self.clock.clone()
    }

    pub fn advance(&self, by: TimeDelta) {
        *self.clock.lock().unwrap() += by;
    }

    pub fn set_open(&self, open: bool) {
        self.open.store(open, Ordering::SeqCst);
    }

    /// Store the member as the gateway reports them.
    pub async fn roster(&self, id: u64, has_role: bool) {
        self.store
            .apply_gateway(GatewayMember {
                user_id: id.to_string(),
                display_name: Some(format!("member-{id}")),
                nickname: None,
                has_role,
                is_bot: false,
                roles: if has_role {
                    vec!["10".into()]
                } else {
                    Vec::new()
                },
                is_guild_admin: false,
            })
            .await
            .unwrap();
    }

    pub async fn request(&self, method: &str, path: &str, extra: &[(&str, &str)]) -> Reply {
        let mut headers = extra.to_vec();
        if method != "GET" && method != "HEAD" {
            headers.push(("Content-Length", "0"));
        }
        send(self.public, method, PUBLIC_HOST, path, &headers, None).await
    }

    pub async fn get(&self, path: &str, extra: &[(&str, &str)]) -> Reply {
        self.request("GET", path, extra).await
    }

    pub async fn admin_get(&self, path: &str, extra: &[(&str, &str)]) -> Reply {
        send(self.admin, "GET", ADMIN_HOST, path, extra, None).await
    }

    /// Start a sign-in and approve it at Discord: the callback path and the
    /// pre-auth cookie header value.
    pub async fn approved(&self, user: DiscordUser, next: &str, ip: &str) -> (String, String) {
        let start = self
            .get(
                &format!("/api/public/auth/discord/start?next={next}"),
                &[("CF-Connecting-IP", ip)],
            )
            .await;
        assert_eq!(start.status, 303, "{}", start.text());
        let login = start.cookie(wire::MEMBER_LOGIN_COOKIE).unwrap();
        let location = start.header("location").unwrap().to_owned();
        let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
        let state = wire::query_value(&pairs, "state").unwrap();
        let challenge = wire::query_value(&pairs, "code_challenge").unwrap();
        let code = format!("code-{}-{}", user.id, &state[..8]);
        self.discord.approve(&code, &challenge, user);
        (
            format!(
                "/api/public/auth/discord/callback?code={code}&state={}",
                wire::encode(&state)
            ),
            format!("{}={login}", wire::MEMBER_LOGIN_COOKIE),
        )
    }

    /// The full sign-in from client address `ip`, answering the callback.
    pub async fn callback(&self, user: DiscordUser, next: &str, ip: &str) -> Reply {
        let (path, login) = self.approved(user, next, ip).await;
        self.get(&path, &[("Cookie", &login), ("CF-Connecting-IP", ip)])
            .await
    }

    /// Sign `user` in from `ip` and read the session's CSRF token.
    pub async fn sign_in_from(&self, user: DiscordUser, ip: &str) -> Browser {
        let reply = self.callback(user, "%2F", ip).await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let id = reply.cookie(wire::MEMBER_SESSION_COOKIE).unwrap();
        let session = self
            .get(
                "/api/public/session",
                &[(cookie(&id).0, &cookie(&id).1), ("CF-Connecting-IP", ip)],
            )
            .await;
        assert_eq!(session.status, 200, "{}", session.text());
        Browser {
            csrf: session.header("x-kanade-csrf").unwrap().to_owned(),
            id,
        }
    }

    pub async fn sign_in(&self, user: DiscordUser) -> Browser {
        self.sign_in_from(user, "198.51.100.10").await
    }

    /// The member's public session rows, superseded ids included.
    pub async fn rows(&self, id: u64) -> Vec<WebSession> {
        self.sessions
            .subject_sessions(SessionOrigin::Public, LoginMethod::Discord, &id.to_string())
            .await
            .unwrap()
    }
}
