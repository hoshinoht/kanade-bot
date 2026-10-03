//! A3 admin reads against a seeded SQLite store and a pinned clock
//! (2026-09-29T04:00:00Z, Tue 12:00 in Kuala Lumpur; boss weeks reset Thursday
//! 00:00). Every response is validated against the frozen A0 schemas.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::{
    api::{
        admin::{config::ConfigDesk, limits::LimitsDesk},
        auth::{
            AdminAuth,
            crypto::SealedSecret,
            discord::{DiscordClient, DiscordLogin, DiscordUser, Secret},
            fake::FakeDiscord,
            staff::{GuildStaffGate, StoreGuildMembers},
            wire,
        },
        listeners::Site,
        rescan::RescanDesk,
        state::{
            ApiState, BackupDir, ChannelEntry, ChannelGrants, ChannelList, DeclineRetraction,
            GuildAccess, ProposalCardRefresh, RoleEntry, StaticChannels,
        },
        write::{ApiClock, SchedulerWriter},
    },
    bot::commands::AccessPolicy,
    chat::{
        driver::{ChatHandle, ChatView},
        pilot::{ChatPilot, GuardLimits, LimitsView, TrafficLimits},
    },
    domain::{
        attendance::AttendanceDefault,
        catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec, GuideSpec},
        history::{Actor, ChangeMeta, Origin, Surface},
        ids::RandomIds,
        members::{Member, MemberProfile, MemberStore, PingLevel},
        schedule::{
            Change, ChangeSet, FixedRun, Reminder, ReminderPolicy, Rsvp, RsvpSource, RsvpState,
            Run, RunSource, RunStatus, SchedulePolicy,
        },
        scheduler::SchedulerService,
        scheduler::{ScheduleStore, Scope},
    },
    infrastructure::store::{SqliteStore, SqliteStoreConfig},
};
use serde_json::Value;
use twilight_model::id::Id;

use crate::{
    logs::fake::FakeRescans,
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, request, send, spawn},
};

type ConfigMaker = Box<dyn FnOnce(Arc<SqliteStore>) -> Arc<ConfigDesk> + Send>;

const TOKEN: &str = "break-glass-token-with-at-least-32-bytes!";
const TAILSCALE_ADMIN: &str = "ops@example.com";
const SECOND_TAILSCALE_ADMIN: &str = "second-ops@example.com";
const EDGE_SECRET: &str = "edge-secret-shared-with-the-caddy-edge!!";
/// What the trusted edge adds to every relayed request.
pub const EDGE_HEADERS: [(&str, &str); 3] = [
    ("X-Kanade-Edge-Auth", EDGE_SECRET),
    ("X-Forwarded-For", "100.64.0.7"),
    ("Tailscale-User-Login", TAILSCALE_ADMIN),
];
const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");

fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

/// Thursday 24 Sep 00:00 KL and Thursday 1 Oct 00:00 KL.
fn this_week() -> DateTime<Utc> {
    utc(9, 23, 16, 0)
}

fn next_week() -> DateTime<Utc> {
    utc(9, 30, 16, 0)
}

fn now() -> DateTime<Utc> {
    utc(9, 29, 4, 0)
}

fn catalog() -> BossTable {
    let difficulty = |prefix: &str, label: &str| DifficultySpec {
        prefix: prefix.into(),
        label: label.into(),
    };
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![
            difficulty("e", "Easy"),
            difficulty("n", "Normal"),
            difficulty("h", "Hard"),
            difficulty("c", "Chaos"),
            difficulty("x", "Extreme"),
        ],
        bosses: vec![
            BossSpec {
                short: "Kalos".into(),
                full: Some("Gatekeeper Kalos".into()),
                level: Some(265),
                difficulties: Some(vec!["e".into(), "n".into(), "c".into(), "x".into()]),
                aliases: vec!["kalos".into()],
                portrait: None,
                guide: Some(GuideSpec {
                    colour: Some(0xF07825),
                }),
            },
            BossSpec {
                short: "MaleficStar".into(),
                full: Some("Radiant Malefic Star".into()),
                level: Some(280),
                difficulties: Some(vec!["n".into(), "h".into()]),
                aliases: vec!["star".into()],
                portrait: None,
                guide: None,
            },
        ],
    })
    .unwrap()
}

fn profile(id: &str, name: &str, has_role: bool) -> MemberProfile {
    MemberProfile {
        member: Member {
            user_id: id.into(),
            display_name: Some(name.into()),
            nickname: None,
            has_role,
            is_bot: false,
            ping_level: PingLevel::Essential,
        },
        ..MemberProfile::default()
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = root.join(format!("kanade-api-store-{}", uuid::Uuid::new_v4()));
        for dir in [path.clone(), path.join("locks")] {
            std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
        }
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Reads {
    pub admin: std::net::SocketAddr,
    pub cookie: String,
    pub csrf: String,
    pub store: Arc<SqliteStore>,
    /// Discord and Tailscale sign-in, when built `with_logins`.
    pub discord: Arc<FakeDiscord>,
    pub rescans: Arc<FakeRescans>,
    /// The post-commit proposal-card refresh port's calls.
    pub proposal_refreshes: Arc<Mutex<Vec<Vec<String>>>>,
    /// The post-commit decline-retraction port's calls.
    pub decline_retractions: Arc<Mutex<Vec<(String, String)>>>,
    pub chat: Arc<FakeChat>,
    pub digest_posts: Arc<Mutex<Vec<kanade::api::state::DigestPostRequest>>>,
    /// `KANADE_BACKUP_DIR`, an empty directory unless built `without_backup_dir`.
    pub backup_dir: Option<PathBuf>,
    _fixture: Fixture,
    _dir: TempDir,
}

pub struct FakeChat {
    pilot: Mutex<ChatPilot>,
    pub resets: Mutex<Vec<String>>,
}

impl Default for FakeChat {
    fn default() -> Self {
        Self {
            pilot: Mutex::new(ChatPilot::new(
                60.0,
                TrafficLimits::default(),
                GuardLimits::default(),
            )),
            resets: Mutex::new(Vec::new()),
        }
    }
}

impl ChatView for FakeChat {
    fn limits(&self) -> LimitsView {
        self.pilot.lock().unwrap().limits(0.0)
    }

    fn reset_allowance(&self, member_id: &str) {
        self.pilot.lock().unwrap().allowance.forget(member_id);
        self.resets.lock().unwrap().push(member_id.to_owned());
    }

    fn status(&self) -> &'static str {
        "idle"
    }
}

fn write(root: &Path, relative: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"art").unwrap();
}

const STAR_WYRM: &str = "boss: StarWyrm
event:
  name: Wyrmfall Trials Season 9
  availability: Invented for tests; never ran.
summary: An invented event boss.
core: [Dodge the wyrm.]
danger: [The tail.]
tips: [Stay central.]
sources: []
";

/// The tracked knowledge documents plus the invented `StarWyrm` event document.
fn event_knowledge(root: &Path) -> PathBuf {
    let tracked = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("boss/knowledge");
    let dir = root.join("knowledge");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(tracked).unwrap().flatten() {
        if entry.path().extension().is_some_and(|ext| ext == "yaml") {
            std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(dir.join("starwyrm.yaml"), STAR_WYRM).unwrap();
    dir
}

/// `reset` past midnight shifts each seeded week start by the same amount.
async fn seed(store: &SqliteStore, reset: NaiveTime) {
    let shift = reset - NaiveTime::MIN;
    let this_week = || this_week() + shift;
    let next_week = || next_week() + shift;
    let mut alice = profile("1001", "Alice", true);
    alice.aliases = vec!["ali".into()];
    alice.reply_style = Some("terse".into());
    let mut bob = profile("1002", "Bob", true);
    bob.reply_style = Some("retired-style".into());
    bob.member.nickname = Some("Bobby".into());
    let mut cara = profile("1003", "Cara", false);
    cara.roles = vec!["20".into()];
    let dan = profile("1004", "Dan", true);
    let mut bot = profile("1005", "Botty", true);
    bot.member.is_bot = true;
    let mut eve = profile("1006", "Eve", false);
    eve.roles = vec!["30".into()];
    eve.member.ping_level = PingLevel::Off;
    for profile in [alice, bob, cara, dan, bot, eve] {
        store.put_member(profile).await.unwrap();
    }

    let fixed = FixedRun {
        id: "f-kalos".into(),
        owner_id: "1001".into(),
        channel_id: Some("kalos-four".into()),
        bosses: vec!["XKalos".into()],
        weekday: Weekday::Tue,
        time: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
        participants: vec!["1001".into(), "1002".into(), "1003".into()],
        note: Some("bring pots".into()),
        attendance_default: AttendanceDefault::default(),
        standing: Vec::new(),
    };
    let run = |id: &str,
               fixed: Option<&str>,
               week: DateTime<Utc>,
               at: DateTime<Utc>,
               bosses: &[&str],
               party: &[&str],
               status: RunStatus,
               source: RunSource| Run {
        id: id.into(),
        fixed_run_id: fixed.map(str::to_owned),
        channel_id: Some(
            if fixed.is_some() {
                "kalos-four"
            } else {
                "star"
            }
            .into(),
        ),
        week_start: week,
        datetime: at,
        bosses: bosses.iter().map(|b| (*b).to_owned()).collect(),
        participants: party.iter().map(|p| (*p).to_owned()).collect(),
        status,
        source,
        attendance: Vec::new(),
        status_pin: None,
    };
    let reminder = |id: &str,
                    run: &str,
                    kind: &str,
                    fire: DateTime<Utc>,
                    sent: Option<DateTime<Utc>>,
                    message: Option<&str>| Reminder {
        id: id.into(),
        run_id: run.into(),
        kind: kind.into(),
        fire_at: fire,
        sent_at: sent,
        message_id: message.map(str::to_owned),
    };
    let rsvp = |run: &str, user: &str, state: RsvpState| Rsvp {
        run_id: run.into(),
        user_id: user.into(),
        state,
        source: RsvpSource::Reaction,
        at: utc(9, 28, 0, 0),
    };
    let changes = vec![
        Change::PutFixedRun(fixed),
        // Tue 29 Sep 22:00 KL, amended roster (Cara out, Dan in).
        Change::PutRun(run(
            "r-kalos",
            Some("f-kalos"),
            this_week(),
            utc(9, 29, 14, 0),
            &["XKalos"],
            &["1001", "1002", "1004"],
            RunStatus::Planned,
            RunSource::Amend,
        )),
        // Sat 26 Sep 21:00 KL, already done.
        Change::PutRun(run(
            "r-star",
            None,
            this_week(),
            utc(9, 26, 13, 0),
            &["HMaleficStar", "HLucid", "Lucid9"],
            &["1001"],
            RunStatus::Done,
            RunSource::Amend,
        )),
        // Next week, own time.
        Change::PutRun(run(
            "n-star",
            None,
            next_week(),
            utc(10, 2, 13, 0),
            &["NMaleficStar"],
            &["1004"],
            RunStatus::Otot,
            RunSource::Amend,
        )),
        Change::PutRun(run(
            "n-kalos",
            Some("f-kalos"),
            next_week(),
            utc(10, 6, 14, 0),
            &["XKalos"],
            &["1001", "1002", "1003"],
            RunStatus::Planned,
            RunSource::Fixed,
        )),
        Change::PutRsvp(rsvp("r-kalos", "1001", RsvpState::Yes)),
        Change::PutRsvp(rsvp("r-kalos", "1002", RsvpState::No)),
        Change::PutRsvp(rsvp("r-star", "1001", RsvpState::Yes)),
        Change::PutReminder(reminder(
            "m-kalos-day",
            "r-kalos",
            "day_of",
            utc(9, 29, 1, 0),
            Some(utc(9, 29, 1, 0)),
            Some("555"),
        )),
        Change::PutReminder(reminder(
            "m-kalos-60",
            "r-kalos",
            "countdown_60",
            utc(9, 29, 13, 0),
            None,
            None,
        )),
        Change::PutReminder(reminder(
            "m-kalos-15",
            "r-kalos",
            "countdown_15",
            utc(9, 29, 13, 45),
            None,
            None,
        )),
        Change::PutReminder(reminder(
            "m-kalos-30",
            "r-kalos",
            "countdown_30",
            utc(9, 29, 13, 30),
            None,
            None,
        )),
        Change::PutReminder(reminder(
            "m-star-day",
            "r-star",
            "day_of",
            utc(9, 26, 1, 0),
            None,
            None,
        )),
    ];
    let revision = store.load(&Scope::All).await.unwrap().revision;
    store
        .commit(
            revision,
            ChangeSet { changes },
            ChangeMeta {
                origin: Origin::new(Actor::admin("seed"), Surface::AdminPortal),
                at: utc(9, 28, 0, 0),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            },
        )
        .await
        .unwrap();
}

impl Reads {
    pub async fn new() -> Self {
        Self::with_reset(NaiveTime::MIN).await
    }

    /// Boss weeks reset Thursday at `reset` (KL) instead of midnight.
    pub async fn with_reset(reset: NaiveTime) -> Self {
        Self::build(reset, false, None, true, true, true).await
    }

    pub async fn with_role_directory_connected(connected: bool) -> Self {
        Self::build(NaiveTime::MIN, false, None, connected, true, true).await
    }

    /// With the config API over the seeded store.
    pub async fn with_config(
        make: impl FnOnce(Arc<SqliteStore>) -> Arc<ConfigDesk> + Send + 'static,
    ) -> Self {
        Self::build(
            NaiveTime::MIN,
            false,
            Some(Box::new(make)),
            true,
            true,
            true,
        )
        .await
    }

    pub async fn with_config_role_directory_connected(
        make: impl FnOnce(Arc<SqliteStore>) -> Arc<ConfigDesk> + Send + 'static,
        connected: bool,
    ) -> Self {
        Self::build(
            NaiveTime::MIN,
            false,
            Some(Box::new(make)),
            connected,
            true,
            true,
        )
        .await
    }

    pub async fn with_config_and_logins(
        make: impl FnOnce(Arc<SqliteStore>) -> Arc<ConfigDesk> + Send + 'static,
    ) -> Self {
        Self::build(NaiveTime::MIN, true, Some(Box::new(make)), true, true, true).await
    }

    /// Also Discord sign-in and Tailscale sign-in through a trusted edge
    /// (the test client, 127.0.0.1, carrying `EDGE_AUTH`).
    pub async fn with_logins() -> Self {
        Self::build(NaiveTime::MIN, true, None, true, true, true).await
    }

    pub async fn without_digest_delivery() -> Self {
        Self::build(NaiveTime::MIN, false, None, true, false, true).await
    }

    /// No `KANADE_BACKUP_DIR`: checkpoints list no backups.
    pub async fn without_backup_dir() -> Self {
        Self::build(NaiveTime::MIN, false, None, true, true, false).await
    }

    async fn build(
        reset: NaiveTime,
        logins: bool,
        config: Option<ConfigMaker>,
        role_directory_connected: bool,
        digest_delivery: bool,
        backup_dir: bool,
    ) -> Self {
        let dir = TempDir::new();
        let backup_dir = backup_dir.then(|| {
            let path = dir.0.join("backups");
            std::fs::create_dir(&path).unwrap();
            path
        });
        let store = Arc::new(
            SqliteStore::open(&SqliteStoreConfig {
                db_path: dir.0.join("kanade.sqlite3"),
                owner_lock_dir: dir.0.join("locks"),
            })
            .await
            .unwrap(),
        );
        seed(&store, reset).await;

        let fixture = Fixture::new();
        // Mixed case on purpose: Linux CI is case-sensitive.
        write(&fixture.root, "boss/portraits/MaleficStar.png");
        write(&fixture.root, "boss/portraits/icon/Kalos.png");
        write(&fixture.root, "boss/artwork/entry/Kalos.webp");
        // An invented event boss outside the catalog, with all three kinds of art.
        let knowledge = event_knowledge(&fixture.root);
        write(&fixture.root, "boss/portraits/StarWyrm.png");
        write(&fixture.root, "boss/portraits/icon/StarWyrm.png");
        write(&fixture.root, "boss/artwork/entry/StarWyrm.png");

        let access = Arc::new(GuildAccess::new(
            AccessPolicy {
                bossing_role_id: Id::new(10),
                admin_role_id: Some(Id::new(20)),
                debug_user_ids: Vec::new(),
            },
            Some("30".into()),
        ));
        let staff = Arc::new(GuildStaffGate::new(
            access.policy.clone(),
            Arc::new(StoreGuildMembers::new(store.clone(), access.clone())),
        ));
        let pinned = now();
        let discord = Arc::new(FakeDiscord::default());
        let mut auth = AdminAuth::new(store.clone(), staff)
            .with_breakglass(TOKEN.as_bytes())
            .unwrap()
            .with_clock(Arc::new(move || pinned));
        if logins {
            auth = auth
                .with_discord(DiscordLogin::new(
                    DiscordClient {
                        client_id: "4242".into(),
                        client_secret: Secret::new("discord-client-secret"),
                        redirect_uri: "https://kanade.test/api/admin/auth/discord/callback".into(),
                    },
                    discord.clone(),
                ))
                .with_tailscale_logins([
                    TAILSCALE_ADMIN.to_owned(),
                    SECOND_TAILSCALE_ADMIN.to_owned(),
                ]);
        }
        let zone = chrono_tz::Asia::Kuala_Lumpur;
        let writer = Arc::new(SchedulerWriter::new(SchedulerService::new(
            store.clone(),
            RandomIds,
            ApiClock(Arc::new(move || pinned)),
        )));
        let rescans = FakeRescans::new(pinned);
        let proposal_refreshes = Arc::new(Mutex::new(Vec::new()));
        let proposal_refresh: ProposalCardRefresh = {
            let refreshes = Arc::clone(&proposal_refreshes);
            Arc::new(move |proposal_ids| {
                let refreshes = Arc::clone(&refreshes);
                Box::pin(async move {
                    refreshes.lock().unwrap().push(proposal_ids);
                })
            })
        };
        let decline_retractions = Arc::new(Mutex::new(Vec::new()));
        let decline_retraction: DeclineRetraction = {
            let calls = Arc::clone(&decline_retractions);
            Arc::new(move |run_id, user_id, _| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.lock().unwrap().push((run_id, user_id));
                })
            })
        };
        let chat = Arc::new(FakeChat::default());
        let chat_handle = Arc::new(ChatHandle::default());
        chat_handle.set(chat.clone());
        let digest_posts = Arc::new(Mutex::new(Vec::new()));
        let digest_post: kanade::api::state::DigestPost = {
            let posts = Arc::clone(&digest_posts);
            Arc::new(move |request| {
                let posts = Arc::clone(&posts);
                Box::pin(async move {
                    posts.lock().unwrap().push(request);
                    kanade::api::state::DigestPostResult::Completed
                })
            })
        };
        let state = ApiState {
            store: store.clone(),
            writer,
            policy: SchedulePolicy::new(
                ReminderPolicy {
                    zone,
                    ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    countdowns: vec![60, 15],
                },
                Weekday::Thu,
                reset,
            ),
            catalog: Arc::new(catalog()),
            channels: Arc::new(ReadyGuild(
                StaticChannels(vec![
                    ChannelEntry {
                        id: "kalos-four".into(),
                        name: "#kalos-four".into(),
                        watched: true,
                    },
                    ChannelEntry {
                        id: "star".into(),
                        name: "#star".into(),
                        watched: false,
                    },
                    ChannelEntry {
                        id: "limbo-trio".into(),
                        name: "#limbo-trio".into(),
                        watched: true,
                    },
                ]),
                role_directory_connected,
            )),
            access,
            knowledge_dir: Some(knowledge),
            guild_id: Some("900".into()),
            clock: Arc::new(move || pinned),
            rescans: Some(Arc::new(RescanDesk::new(rescans.clone()))),
            config: config.map(|make| make(store.clone())),
            chat: Some(chat_handle),
            model_limits: None,
            limits: Arc::new(LimitsDesk::default()),
            proposal_refresh: Some(proposal_refresh),
            decline_retraction: Some(decline_retraction),
            digest_post: digest_delivery.then_some(digest_post),
            backups: BackupDir {
                dir: backup_dir.clone(),
                schema_version: store.schema_version().await.unwrap(),
            },
        };
        let mut http = fixture.http();
        if logins {
            http.trusted_proxy = Some([127, 0, 0, 1].into());
        }
        let mut site = Site::admin(&http);
        site.auth = Some(Arc::new(auth));
        if logins {
            site.edge_secret = Some(Arc::new(SealedSecret::new(EDGE_SECRET.as_bytes()).unwrap()));
        }
        site.state = Some(Arc::new(state));
        let admin = spawn(site).await;
        let login = send(
            admin,
            "POST",
            ADMIN_HOST,
            "/api/admin/auth/token",
            &[ORIGIN],
            Some(&format!(r#"{{"token":"{TOKEN}"}}"#)),
        )
        .await;
        let cookie = login
            .cookie(kanade::api::auth::wire::SESSION_COOKIE)
            .expect("signed in");
        let csrf = login.header("x-kanade-csrf").expect("csrf").to_owned();
        Self {
            admin,
            cookie: format!("{}={cookie}", kanade::api::auth::wire::SESSION_COOKIE),
            csrf,
            store,
            discord,
            rescans,
            proposal_refreshes,
            decline_retractions,
            chat,
            digest_posts,
            backup_dir,
            _fixture: fixture,
            _dir: dir,
        }
    }

    /// A Discord session for `user` (staff by the stored member rows):
    /// `(Cookie header, CSRF token)`.
    pub async fn discord_session(&self, id: u64, name: &str) -> (String, String) {
        let start = request(
            self.admin,
            "GET",
            ADMIN_HOST,
            "/api/admin/auth/discord/start?next=%2F",
            &[],
        )
        .await;
        assert_eq!(start.status, 303, "{}", start.text());
        let login = start.cookie(wire::LOGIN_COOKIE).unwrap();
        let location = start.header("location").unwrap().to_owned();
        let pairs = wire::query_pairs(location.split_once('?').map(|(_, query)| query));
        let state = wire::query_value(&pairs, "state").unwrap();
        let challenge = wire::query_value(&pairs, "code_challenge").unwrap();
        let code = format!("code-{id}");
        self.discord.approve(
            &code,
            &challenge,
            DiscordUser {
                id: id.to_string(),
                username: name.to_lowercase(),
                global_name: Some(name.into()),
                bot: false,
            },
        );
        let login_cookie = format!("{}={login}", wire::LOGIN_COOKIE);
        let callback = request(
            self.admin,
            "GET",
            ADMIN_HOST,
            &format!(
                "/api/admin/auth/discord/callback?code={code}&state={}",
                wire::encode(&state)
            ),
            &[("Cookie", &login_cookie)],
        )
        .await;
        let session = callback
            .cookie(wire::SESSION_COOKIE)
            .unwrap_or_else(|| panic!("signed in: {}", callback.dump()));
        let cookie = format!("{}={session}", wire::SESSION_COOKIE);
        let me = request(
            self.admin,
            "GET",
            ADMIN_HOST,
            "/api/admin/session",
            &[("Cookie", &cookie)],
        )
        .await;
        (cookie, me.header("x-kanade-csrf").unwrap().to_owned())
    }

    /// A Tailscale session through the trusted edge: `(Cookie header, CSRF
    /// token)`; every later request must carry [`EDGE_HEADERS`] too.
    pub async fn tailscale_session(&self) -> (String, String) {
        self.tailscale_session_as(TAILSCALE_ADMIN).await
    }

    pub async fn tailscale_session_as(&self, login_name: &str) -> (String, String) {
        let mut headers = vec![ORIGIN];
        let mut edge_headers = EDGE_HEADERS;
        edge_headers[2] = ("Tailscale-User-Login", login_name);
        headers.extend_from_slice(&edge_headers);
        let login = send(
            self.admin,
            "POST",
            ADMIN_HOST,
            "/api/admin/auth/tailscale",
            &headers,
            None,
        )
        .await;
        assert_eq!(login.status, 200, "{}", login.text());
        (
            format!(
                "{}={}",
                wire::SESSION_COOKIE,
                login.cookie(wire::SESSION_COOKIE).unwrap()
            ),
            login.header("x-kanade-csrf").unwrap().to_owned(),
        )
    }

    /// GET as the signed-in admin; asserts 200 and the schema.
    pub async fn read(&self, path: &str, target: &str) -> Value {
        let reply = request(
            self.admin,
            "GET",
            ADMIN_HOST,
            path,
            &[("Cookie", &self.cookie)],
        )
        .await;
        assert_eq!(reply.status, 200, "{path}: {}", reply.text());
        let value = reply.json();
        assert_valid(target, path, &value);
        value
    }

    pub async fn status(&self, path: &str, cookie: bool) -> (u16, String) {
        let extra: Vec<(&str, &str)> = if cookie {
            vec![("Cookie", self.cookie.as_str())]
        } else {
            Vec::new()
        };
        let reply = request(self.admin, "GET", ADMIN_HOST, path, &extra).await;
        assert_valid("error.json#/$defs/ApiError", path, &reply.json());
        (reply.status, reply.api_error())
    }
}

fn run<'a>(week: &'a Value, id: &str) -> &'a Value {
    week["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["id"] == id)
        .unwrap_or_else(|| panic!("{id} in {week:#}"))
}

#[tokio::test]
async fn week_projects_runs_answers_cards_and_the_history_version() {
    let reads = Reads::new().await;
    let week = reads.read("/api/admin/week", "week.json#/$defs/Week").await;
    assert_eq!(week["starts"], "2026-09-24");
    assert_eq!(week["timezone"], "Asia/Kuala_Lumpur");
    assert_eq!(week["reset"], "Thu 00:00");
    assert_eq!(week["generated_at"], "2026-09-29T04:00:00Z");
    assert_eq!(week["version"], 1, "one seed commit in history");
    assert_eq!(week["days"][5]["date"], "2026-09-29");
    assert_eq!(week["days"][5]["is_today"], true);
    assert_eq!(week["days"][0]["dow"], "Thu");
    assert_eq!(week["runs"].as_array().unwrap().len(), 2, "this week only");

    let kalos = run(&week, "r-kalos");
    assert_eq!(
        (kalos["day"].clone(), kalos["time"].clone()),
        (5.into(), "22:00".into())
    );
    assert_eq!(kalos["tally"], serde_json::json!({"on": 1, "total": 3}));
    let answers: Vec<_> = kalos["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["answer"].as_str().unwrap()))
        .collect();
    assert_eq!(
        answers,
        [("Alice", "yes"), ("Bobby", "no"), ("Dan", "waiting")]
    );
    assert_eq!(kalos["channel"], "#kalos-four");
    assert_eq!(kalos["amended"], true);
    assert_eq!(kalos["roster_change"]["out"][0]["name"], "Cara");
    assert_eq!(kalos["roster_change"]["in"][0]["name"], "Dan");
    let cards: Vec<_> = kalos["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["label"].as_str().unwrap(),
                c["state"].as_str().unwrap(),
                c["at"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        cards,
        [
            ("morning", "posted", "09:00"),
            ("T-1h", "queued", "21:00"),
            ("T-15m", "queued", "21:45")
        ],
        "an unlabelled countdown (30 min) is left out"
    );
    assert_eq!(
        kalos["cards"][0]["url"],
        "https://discord.com/channels/900/kalos-four/555"
    );
    let boss = &kalos["bosses"][0];
    assert_eq!(
        (boss["key"].clone(), boss["name"].clone()),
        ("Kalos".into(), "Gatekeeper Kalos".into())
    );
    assert_eq!(boss["portrait"], Value::Null, "no portrait file");
    assert_eq!(boss["portrait_sm"], "/art/icons/Kalos");
    assert_eq!(boss["art"], "/art/entry/Kalos");
    assert_eq!(boss["hue"], 25);

    let star = run(&week, "r-star");
    assert_eq!(
        star["bosses"].as_array().unwrap().len(),
        2,
        "an unknown token keeps its text if it has a difficulty letter, else it is dropped"
    );
    assert_eq!(star["bosses"][0]["portrait"], "/art/portraits/MaleficStar");
    assert_eq!(star["bosses"][1]["token"], "HLucid");
    assert_eq!(
        star["cards"][0]["state"], "skipped",
        "never posted and too late"
    );

    let next = reads
        .read("/api/admin/week?week=next", "week.json#/$defs/Week")
        .await;
    assert_eq!(next["starts"], "2026-10-01");
    assert_eq!(run(&next, "n-star")["time"], Value::Null, "own time");
    assert_eq!(
        reads.status("/api/admin/week?week=later", true).await,
        (422, "invalid_query".into())
    );
}

#[tokio::test]
async fn stats_summary_and_reminders() {
    let reads = Reads::new().await;
    let stats = reads
        .read("/api/admin/stats", "week.json#/$defs/Stats")
        .await;
    assert_eq!(
        stats["per_day"][5],
        serde_json::json!({"day": 5, "answered": 2, "waiting": 1})
    );
    assert_eq!(
        stats["per_day"][2],
        serde_json::json!({"day": 2, "answered": 1, "waiting": 0})
    );

    let summary = reads
        .read("/api/admin/summary", "week.json#/$defs/Summary")
        .await;
    assert_eq!(summary["next"]["run_id"], "r-kalos");
    assert_eq!(summary["next"]["when"], "Tue 29 Sep 22:00");
    assert_eq!(summary["next"]["countdown"], "in 10 h");
    assert_eq!(summary["next"]["bosses"], "XKalos");
    assert_eq!(
        summary["unanswered"],
        1 + 3,
        "Dan this week, all of n-kalos"
    );
    assert_eq!(summary["inbox"], 0);
    assert_eq!(summary["members"], 3, "Alice, Bob, Dan; never the bot");

    let reminders = reads
        .read("/api/admin/reminders", "reminders.json#/$defs/Reminders")
        .await;
    let ids = |list: &str| -> Vec<String> {
        reminders[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                format!(
                    "{}:{}",
                    row["id"].as_str().unwrap(),
                    row["state"].as_str().unwrap()
                )
            })
            .collect()
    };
    assert_eq!(ids("upcoming"), ["m-kalos-60:queued", "m-kalos-15:queued"]);
    assert_eq!(summary["reminders"], 2, "the upcoming rows above");
    assert_eq!(ids("sent"), ["m-kalos-day:sent", "m-star-day:stale"]);
    assert_eq!(
        reminders["sent"][0]["party"],
        serde_json::json!(["Alice", "Bobby", "Dan"])
    );
    assert_eq!(reminders["sent"][0]["at"], "Tue 29 Sep 09:00");
}

#[tokio::test]
async fn members_channels_personas_and_fixed() {
    let reads = Reads::new().await;
    let members = reads
        .read("/api/admin/members", "members.json#/$defs/MemberRows")
        .await;
    let rows: Vec<_> = members
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["access"].as_str().unwrap(),
                row["bossing"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("1001", "none", true),
            ("1002", "none", true),
            ("1003", "staff", false),
            ("1004", "none", true),
            ("1006", "pilot", false),
        ],
        "bots never, non-bossers only with access; by name"
    );
    let alice = &members[0];
    assert_eq!(alice["aliases"], serde_json::json!(["ali"]));
    assert_eq!(alice["runs_this_week"], 2);
    assert_eq!(alice["persona"], "terse");
    assert_eq!(
        alice["persona_available"], false,
        "no visibility setting is private"
    );
    assert_eq!(members[1]["name"], "Bobby");
    assert_eq!(members[1]["persona_available"], false);
    assert_eq!(members[4]["ping_level"], "off");

    let channels = reads
        .read("/api/admin/channels", "common.json#/$defs/Channels")
        .await;
    assert_eq!(
        channels[0],
        serde_json::json!({"id": "kalos-four", "name": "#kalos-four"})
    );
    let roles = reads
        .read("/api/admin/roles", "common.json#/$defs/Roles")
        .await;
    assert_eq!(
        roles,
        serde_json::json!([
            {"id": "700", "name": "Officer", "color": "#0a0bff"},
            {"id": "701", "name": "Bossing"},
            {"id": "702", "name": "Integration"},
        ])
    );
    let identity = reads
        .read("/api/identity", "identity.json#/$defs/Identity")
        .await;
    assert_eq!(identity["bot_user_id"], "42");
    assert_eq!(identity["name"], "mikan", "the live name once READY");
    let monogram = request(reads.admin, "GET", ADMIN_HOST, "/identity/avatar", &[]).await;
    assert!(monogram.text().contains(">M</text>"), "live initial");
    let personas = reads
        .read("/api/admin/personas", "members.json#/$defs/Personas")
        .await;
    assert_eq!(
        personas,
        serde_json::json!([]),
        "missing settings are private"
    );

    let fixed = reads
        .read("/api/admin/fixed", "fixed.json#/$defs/FixedRows")
        .await;
    let row = &fixed[0];
    assert_eq!(
        (row["weekday"].clone(), row["weekday_name"].clone()),
        (1.into(), "Tuesday".into())
    );
    assert_eq!(row["time"], "22:00");
    assert_eq!(row["owner"], "Alice");
    assert_eq!(row["channel_watched"], true);
    let runs: Vec<_> = row["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| {
            (
                link["run_id"].as_str().unwrap(),
                link["week"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(runs, [("r-kalos", "this"), ("n-kalos", "next")]);
}

#[tokio::test]
async fn roles_are_unavailable_when_the_guild_directory_is_disconnected() {
    let reads = Reads::with_role_directory_connected(false).await;
    assert_eq!(
        reads.status("/api/admin/roles", true).await,
        (503, "unavailable".into())
    );
}

#[tokio::test]
async fn bosses_events_and_knowledge() {
    let reads = Reads::new().await;
    let bosses = reads
        .read("/api/admin/bosses", "bosses.json#/$defs/BossRows")
        .await;
    assert_eq!(bosses[0]["key"], "Kalos");
    let in_use: Vec<_> = bosses[0]["difficulties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["token"].as_str().unwrap(), d["in_use"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        in_use,
        [
            ("EKalos", false),
            ("NKalos", false),
            ("CKalos", false),
            ("XKalos", true)
        ]
    );
    assert_eq!(bosses[1]["portrait"], "/art/portraits/MaleficStar");

    let events = reads
        .read("/api/admin/bosses/events", "bosses.json#/$defs/EventBosses")
        .await;
    assert!(
        events
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["key"] == "Kai")
    );

    let star = reads
        .read(
            "/api/admin/bosses/MaleficStar/knowledge",
            "bosses.json#/$defs/Knowledge",
        )
        .await;
    assert_eq!(star["name"], "Radiant Malefic Star");
    assert_eq!(star["path"], "boss/knowledge/maleficstar.yaml");
    let kalos = reads
        .read(
            "/api/admin/bosses/Kalos/knowledge",
            "bosses.json#/$defs/Knowledge",
        )
        .await;
    assert_eq!(kalos["in_use"], serde_json::json!(["x"]));
    for path in [
        "/api/admin/bosses/Nobody/knowledge",
        "/api/admin/bosses/..%2F..%2Fetc/knowledge",
        "/api/admin/bosses/_meta/knowledge",
    ] {
        assert_eq!(
            reads.status(path, true).await,
            (404, "not_found".into()),
            "{path}"
        );
    }
}

#[tokio::test]
async fn event_bosses_carry_and_serve_their_art() {
    let reads = Reads::new().await;
    let events = reads
        .read("/api/admin/bosses/events", "bosses.json#/$defs/EventBosses")
        .await;
    let wyrm = events
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["key"] == "StarWyrm")
        .expect("the fixture event");
    assert_eq!(wyrm["event"]["name"], "Wyrmfall Trials Season 9");
    assert_eq!(wyrm["portrait"], "/art/portraits/StarWyrm");
    assert_eq!(wyrm["portrait_sm"], "/art/icons/StarWyrm");
    assert_eq!(wyrm["art"], "/art/entry/StarWyrm");
    // Absent art stays null.
    let kai = events
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["key"] == "Kai")
        .expect("tracked event");
    assert_eq!(
        (&kai["portrait"], &kai["portrait_sm"], &kai["art"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );

    let knowledge = reads
        .read(
            "/api/admin/bosses/StarWyrm/knowledge",
            "bosses.json#/$defs/Knowledge",
        )
        .await;
    assert_eq!(knowledge["portrait"], "/art/portraits/StarWyrm");

    for path in [
        "/art/portraits/StarWyrm",
        "/art/icons/StarWyrm",
        "/art/entry/StarWyrm",
    ] {
        let reply = request(reads.admin, "GET", ADMIN_HOST, path, &[]).await;
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(reply.body, b"art", "{path}");
    }
    for path in [
        // Exact case only (a case-insensitive filesystem would otherwise find the file).
        "/art/portraits/starwyrm",
        "/art/portraits/STARWYRM",
        "/art/icons/starWyrm",
        // Unknown key.
        "/art/portraits/MoonWyrm",
        // A document without `event`, outside the catalog, whose art file exists.
        "/art/portraits/Carling",
        "/art/entry/Carling",
    ] {
        let reply = request(reads.admin, "GET", ADMIN_HOST, path, &[]).await;
        assert_eq!(reply.status, 404, "{path}");
    }
}

#[tokio::test]
async fn every_read_needs_a_session_and_art_uses_catalog_keys() {
    let reads = Reads::new().await;
    for path in [
        "/api/admin/week",
        "/api/admin/stats",
        "/api/admin/summary",
        "/api/admin/fixed",
        "/api/admin/reminders",
        "/api/admin/members",
        "/api/admin/channels",
        "/api/admin/roles",
        "/api/admin/personas",
        "/api/admin/bosses",
        "/api/admin/bosses/events",
        "/api/admin/bosses/Kalos/knowledge",
    ] {
        assert_eq!(
            reads.status(path, false).await,
            (401, "unauthenticated".into()),
            "{path}"
        );
    }
    let art = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        "/art/portraits/MaleficStar",
        &[],
    )
    .await;
    assert_eq!(art.status, 200);
    for path in ["/art/portraits/maleficstar", "/art/portraits/Lucid"] {
        let reply = request(reads.admin, "GET", ADMIN_HOST, path, &[]).await;
        assert_eq!(reply.status, 404, "{path}: exact catalog keys only");
    }
}

#[tokio::test]
async fn staff_sign_in_reads_the_persisted_member_rows() {
    use kanade::api::auth::staff::{StaffCheck, StaffGate};
    let reads = Reads::new().await;
    let access = Arc::new(GuildAccess::new(
        AccessPolicy {
            bossing_role_id: Id::new(10),
            admin_role_id: Some(Id::new(20)),
            debug_user_ids: Vec::new(),
        },
        None,
    ));
    let gate = GuildStaffGate::new(
        access.policy.clone(),
        Arc::new(StoreGuildMembers::new(reads.store.clone(), access.clone())),
    );
    assert_eq!(gate.check("1003").await, StaffCheck::Staff, "admin role");
    assert_eq!(gate.check("1001").await, StaffCheck::NotStaff);
    assert_eq!(
        gate.check("7777").await,
        StaffCheck::NotStaff,
        "not a member"
    );
    access.set_owner(Some(Id::new(1001)));
    assert_eq!(gate.check("1001").await, StaffCheck::Staff, "guild owner");
    let mut admin = reads.store.load_member("1004").await.unwrap().unwrap();
    admin.is_guild_admin = true;
    reads.store.put_member(admin).await.unwrap();
    assert_eq!(
        gate.check("1004").await,
        StaffCheck::Staff,
        "Administrator permission"
    );
    let mut bot = reads.store.load_member("1005").await.unwrap().unwrap();
    bot.is_guild_admin = true;
    reads.store.put_member(bot).await.unwrap();
    assert_eq!(
        gate.check("1005").await,
        StaffCheck::NotStaff,
        "never a bot"
    );
}

/// A gateway-ready guild: the fixed channels plus roles and the bot's id.
struct ReadyGuild(StaticChannels, bool);

impl ChannelList for ReadyGuild {
    fn channels(&self) -> Vec<ChannelEntry> {
        self.0.channels()
    }

    fn roles(&self) -> Vec<RoleEntry> {
        vec![
            RoleEntry {
                id: "700".into(),
                name: "Officer".into(),
                color: Some(0x0a0bff),
            },
            RoleEntry {
                id: "701".into(),
                name: "Bossing".into(),
                color: None,
            },
            // A current managed role stays selectable just like any other role.
            RoleEntry {
                id: "702".into(),
                name: "Integration".into(),
                color: None,
            },
        ]
    }

    fn bot_user_id(&self) -> Option<String> {
        Some("42".into())
    }

    fn bot_name(&self) -> Option<String> {
        Some("mikan".into())
    }

    fn connected(&self) -> bool {
        self.1
    }

    /// `kalos-four` is not known yet; `limbo-trio` may not post or tidy.
    fn grants(&self, id: &str) -> Option<ChannelGrants> {
        match id {
            "kalos-four" => None,
            "limbo-trio" => Some(ChannelGrants {
                send: false,
                manage_messages: false,
                ..ChannelGrants::UNKNOWN
            }),
            _ => Some(ChannelGrants::UNKNOWN),
        }
    }
}
