//! The retained v4 slash commands end to end: `FakeDiscord` interactions
//! through the real dispatcher, a temp SQLite store and the shared scheduler
//! writer, on a pinned clock (Tue 29 Sep 2026 12:00 Kuala Lumpur; boss
//! weeks reset Thursday 00:00). Expected texts are v4's, printed by the
//! rollback tree's `bot.agent.commands`/`formatting` where deterministic.

mod fixed;
mod members;
mod ports;
mod registry;
mod runs;
mod schedule;
mod surface;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use serde_json::{Value, json};
use twilight_model::application::command::CommandOptionChoice;
use twilight_model::application::command::CommandOptionChoiceValue;
use twilight_model::application::interaction::Interaction;

use kanade::api::admin::config::{ConfigDesk, ConfigFacts, ConfigInputs, PersonaFiles};
use kanade::api::rescan::RescanRunner;
use kanade::api::state::GuildAccess;
use kanade::api::write::{ApiClock, SchedulerWriter};
use kanade::bot::commands::{
    AccessPolicy, ChatAllowance, CommandContext, DebugCards, Dispatcher, Disposition,
    GuildChannels, register_retained,
};
use kanade::bot::transport::{Call, FakeDiscord, InteractionReply, Outcome};
use kanade::chat::persona::{PersonaRoot, PersonaSnapshot, PersonaStore};
use kanade::domain::attendance::AttendanceDefault;
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::domain::history::{Actor, ChangeMeta, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Member, MemberProfile, MemberStore, PingLevel};
use kanade::domain::schedule::{
    Change, ChangeSet, FixedRun, ReminderPolicy, Run, RunSource, RunStatus, SchedulePolicy,
};
use kanade::domain::scheduler::{ScheduleStore, SchedulerService, Scope};
use kanade::domain::settings::{MessageStyle, RuntimeSettings};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};

use super::support::{ADMIN_ROLE, BOSSING_ROLE, GUILD, OWNER, guild, parse, role, user};

pub const ALICE: u64 = 1001;
pub const BOB: u64 = 1002;
/// Has the bossing role, on no run.
pub const DAN: u64 = 1004;
/// No bossing role.
pub const CARA: u64 = 1006;
pub const BOTTY: u64 = 1005;
pub const PILOT_ROLE: u64 = 502;
/// Watched party channel and an unwatched one.
pub const KALOS: u64 = 301;
pub const LOUNGE: u64 = 302;

pub const F_KALOS: &str = "ffff0001-0000-4000-8000-000000000001";
/// Tue 29 Sep 22:00 KL, this boss week, from `F_KALOS`.
pub const R_KALOS: &str = "11111111-0000-4000-8000-000000000001";
/// Sat 26 Sep 21:00 KL, this week, a finished one-off in the lounge.
pub const R_DONE: &str = "22222222-0000-4000-8000-000000000002";
/// Tue 6 Oct 22:00 KL, next week, from `F_KALOS`.
pub const R_NEXT: &str = "33333333-0000-4000-8000-000000000003";

pub fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

pub fn now() -> DateTime<Utc> {
    utc(9, 29, 4, 0)
}

/// Thu 24 Sep and Thu 1 Oct 00:00 KL.
fn this_week() -> DateTime<Utc> {
    utc(9, 23, 16, 0)
}

fn next_week() -> DateTime<Utc> {
    utc(9, 30, 16, 0)
}

pub fn catalog() -> BossTable {
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
                guide: None,
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

/// Watched: `KALOS`; known but unwatched: `LOUNGE`; posting is refused in
/// channels listed in `closed`.
pub struct Channels {
    pub closed: Vec<String>,
}

impl GuildChannels for Channels {
    fn is_watched(&self, channel_id: &str) -> bool {
        channel_id == KALOS.to_string()
    }

    fn origin(&self, channel_id: &str) -> String {
        channel_id.to_owned()
    }

    fn name(&self, channel_id: &str) -> Option<String> {
        match channel_id.parse::<u64>().ok()? {
            KALOS => Some("kalos-four".into()),
            LOUNGE => Some("lounge".into()),
            _ => None,
        }
    }

    fn watched(&self) -> Vec<String> {
        vec![KALOS.to_string()]
    }

    fn can_send(&self, channel_id: &str) -> bool {
        self.name(channel_id).is_some() && !self.closed.iter().any(|id| id == channel_id)
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = root.join(format!("kanade-slash-{}", uuid::Uuid::new_v4()));
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

fn profile_config(dir: &TempDir, store: Arc<SqliteStore>, style: MessageStyle) -> Arc<ConfigDesk> {
    let root = dir.0.join("personas");
    std::fs::create_dir_all(root.join("bundles")).unwrap();
    std::fs::create_dir_all(root.join("profiles")).unwrap();
    std::fs::write(
        root.join("bundles/kanade.yaml"),
        "schema_version: 1\nid: kanade\nidentity: |\n  # Persona: Kanade\n\n  Synthetic identity.\nbehaviour:\n  voice: Synthetic voice.\n  prompt: Synthetic prompt.\nstaging:\n  schedule: Synthetic schedule\n  guide: Synthetic guide\n  guide_named: '{boss} synthetic guide'\n  write: Synthetic write\n  generic: Synthetic reply\n",
    )
    .unwrap();
    for (id, label) in [("terse", "Terse"), ("loud", "Loud")] {
        std::fs::write(
            root.join(format!("profiles/{id}.yaml")),
            format!(
                "schema_version: 1\nid: {id}\nlabel: {label}\nvoice: Synthetic {label} voice.\nprompt: Synthetic {label} profile.\n"
            ),
        )
        .unwrap();
    }
    let persona_root = PersonaRoot::open(&root).unwrap();
    let personas = Arc::new(PersonaStore::new(PersonaSnapshot::startup(
        &persona_root,
        None,
    )));
    let mut settings = RuntimeSettings::default();
    settings.persona.profile_visibility = vec!["terse".into()];
    settings.notifications.message_style = style;
    Arc::new(ConfigDesk::new(ConfigInputs {
        settings,
        store,
        models: None,
        facts: ConfigFacts::default(),
        personas: Some(PersonaFiles {
            dir: root,
            store: personas,
        }),
    }))
}

fn profile(id: u64, name: &str, has_role: bool) -> MemberProfile {
    MemberProfile {
        member: Member {
            user_id: id.to_string(),
            display_name: Some(name.into()),
            nickname: None,
            has_role,
            is_bot: false,
            ping_level: PingLevel::Essential,
        },
        ..MemberProfile::default()
    }
}

async fn seed(store: &SqliteStore) {
    let mut bob = profile(BOB, "Bob", true);
    bob.member.nickname = Some("Bobby".into());
    let mut botty = profile(BOTTY, "Botty", true);
    botty.member.is_bot = true;
    let mut alice = profile(ALICE, "Alice", true);
    alice.reply_style = Some("retired".into());
    for member in [
        alice,
        bob,
        profile(DAN, "Dan", true),
        profile(CARA, "Cara", false),
        botty,
    ] {
        store.put_member(member).await.unwrap();
    }
    let run = |id: &str,
               fixed: Option<&str>,
               week: DateTime<Utc>,
               at: DateTime<Utc>,
               party: &[u64],
               status: RunStatus,
               channel: u64| Run {
        id: id.into(),
        fixed_run_id: fixed.map(str::to_owned),
        channel_id: Some(channel.to_string()),
        week_start: week,
        datetime: at,
        bosses: vec!["XKalos".into()],
        participants: party.iter().map(u64::to_string).collect(),
        status,
        source: if fixed.is_some() {
            RunSource::Fixed
        } else {
            RunSource::Amend
        },
        attendance: Vec::new(),
        status_pin: None,
    };
    let changes = vec![
        Change::PutFixedRun(FixedRun {
            id: F_KALOS.into(),
            owner_id: ALICE.to_string(),
            channel_id: Some(KALOS.to_string()),
            bosses: vec!["XKalos".into()],
            weekday: Weekday::Tue,
            time: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            participants: vec![ALICE.to_string(), BOB.to_string()],
            note: None,
            attendance_default: AttendanceDefault::default(),
            standing: Vec::new(),
        }),
        Change::PutRun(run(
            R_KALOS,
            Some(F_KALOS),
            this_week(),
            utc(9, 29, 14, 0),
            &[ALICE, BOB],
            RunStatus::Planned,
            KALOS,
        )),
        Change::PutRun(run(
            R_DONE,
            None,
            this_week(),
            utc(9, 26, 13, 0),
            &[ALICE],
            RunStatus::Done,
            LOUNGE,
        )),
        Change::PutRun(run(
            R_NEXT,
            Some(F_KALOS),
            next_week(),
            utc(10, 6, 14, 0),
            &[ALICE, BOB],
            RunStatus::Planned,
            KALOS,
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

/// Optional ports a test plugs in.
#[derive(Default)]
pub struct Ports {
    pub rescans: Option<Arc<dyn RescanRunner>>,
    pub allowance: Option<Arc<dyn ChatAllowance>>,
    pub debug_cards: Option<Arc<dyn DebugCards>>,
    pub header_rewrite: Option<kanade::api::state::HeaderRewritePort>,
    pub closed: Vec<String>,
    /// The saved message style (classic by default).
    pub style: MessageStyle,
}

pub struct Slash {
    pub store: Arc<SqliteStore>,
    pub discord: Arc<FakeDiscord>,
    pub dispatcher: Dispatcher,
    next_id: AtomicU64,
    /// Every reply, in order, for assertions.
    pub replies: Mutex<Vec<InteractionReply>>,
    _dir: TempDir,
}

impl Slash {
    pub async fn new() -> Self {
        Self::with(Ports::default()).await
    }

    pub async fn with(ports: Ports) -> Self {
        let dir = TempDir::new();
        let store = Arc::new(
            SqliteStore::open(&SqliteStoreConfig {
                db_path: dir.0.join("kanade.sqlite3"),
                owner_lock_dir: dir.0.join("locks"),
            })
            .await
            .unwrap(),
        );
        seed(&store).await;
        let pinned = now();
        let writer = Arc::new(SchedulerWriter::new(SchedulerService::new(
            store.clone(),
            RandomIds,
            ApiClock(Arc::new(move || pinned)),
        )));
        let policy = AccessPolicy {
            bossing_role_id: role(BOSSING_ROLE),
            admin_role_id: Some(role(ADMIN_ROLE)),
            debug_user_ids: vec![user(DAN)],
        };
        let ctx = Arc::new(CommandContext {
            store: store.clone(),
            writer,
            members: store.clone(),
            policy: SchedulePolicy::new(
                ReminderPolicy {
                    zone: chrono_tz::Asia::Kuala_Lumpur,
                    ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    countdowns: vec![60, 15],
                },
                Weekday::Thu,
                NaiveTime::MIN,
            ),
            catalog: Arc::new(catalog()),
            channels: Arc::new(Channels {
                closed: ports.closed,
            }),
            access: Arc::new(GuildAccess::new(
                policy.clone(),
                Some(PILOT_ROLE.to_string()),
            )),
            config: Some(profile_config(&dir, store.clone(), ports.style)),
            rescans: ports.rescans,
            allowance: ports.allowance,
            debug_cards: ports.debug_cards,
            decline_retraction: None,
            header_rewrite: ports.header_rewrite,
            bot_name: Some("Kanade".into()),
            clock: Arc::new(move || pinned),
        });
        let discord = Arc::new(FakeDiscord::new());
        let dispatcher = register_retained(Dispatcher::new(policy), &ctx, discord.clone()).unwrap();
        Self {
            store,
            discord,
            dispatcher,
            next_id: AtomicU64::new(7_000),
            replies: Mutex::new(Vec::new()),
            _dir: dir,
        }
    }

    pub fn interaction(
        &self,
        kind: u8,
        invoker: u64,
        roles: &[u64],
        channel: u64,
        name: &str,
        options: Value,
    ) -> Interaction {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        parse(json!({
            "application_id": "9",
            "authorizing_integration_owners": {},
            "entitlements": [],
            "id": id.to_string(),
            "type": kind,
            "token": "interaction-secret-token",
            "guild_id": GUILD.to_string(),
            "channel": { "id": channel.to_string(), "type": 0 },
            "member": {
                "user": {
                    "id": invoker.to_string(),
                    "username": format!("user{invoker}"),
                    "global_name": null,
                    "discriminator": "0",
                    "avatar": null,
                    "bot": false,
                },
                "nick": null,
                "roles": roles.iter().map(u64::to_string).collect::<Vec<_>>(),
                "joined_at": null,
                "deaf": false,
                "mute": false,
                "flags": 0,
                "permissions": "0",
                "communication_disabled_until": null,
            },
            "data": { "id": "5500", "name": name, "type": 1, "options": options },
        }))
    }

    /// Handle an interaction and return the reply it produced (the direct
    /// response, or the deferred completion).
    pub async fn send(&self, interaction: &Interaction) -> (Disposition, InteractionReply) {
        let before = self.discord.calls().len();
        let (disposition, outcome) = self
            .dispatcher
            .handle(
                self.discord.as_ref(),
                guild(),
                interaction,
                Some(user(OWNER)),
            )
            .await
            .expect("a guild command");
        assert_eq!(outcome, Outcome::Delivered(()), "{disposition:?}");
        let reply = self.discord.calls()[before..]
            .iter()
            .rev()
            .find_map(|call| match call {
                Call::Respond { reply, .. } | Call::CompleteDeferred { reply, .. } => {
                    Some(reply.clone())
                }
                _ => None,
            })
            .expect("a reply");
        self.replies.lock().unwrap().push(reply.clone());
        (disposition, reply)
    }

    /// `/name` by `invoker` in `channel`; the reply text.
    pub async fn run_in(
        &self,
        invoker: u64,
        roles: &[u64],
        channel: u64,
        name: &str,
        options: Value,
    ) -> InteractionReply {
        let interaction = self.interaction(2, invoker, roles, channel, name, options);
        self.send(&interaction).await.1
    }

    /// `/name` by a bossing-role member in the party channel.
    pub async fn run(&self, invoker: u64, name: &str, options: Value) -> String {
        self.run_in(invoker, &[BOSSING_ROLE], KALOS, name, options)
            .await
            .content
    }

    /// Autocomplete for the focused option; `(label, value)` pairs.
    pub async fn suggest(
        &self,
        invoker: u64,
        roles: &[u64],
        name: &str,
        options: Value,
    ) -> Vec<(String, String)> {
        let interaction = self.interaction(4, invoker, roles, KALOS, name, options);
        let before = self.discord.calls().len();
        let (disposition, _) = self
            .dispatcher
            .handle(
                self.discord.as_ref(),
                guild(),
                &interaction,
                Some(user(OWNER)),
            )
            .await
            .expect("a guild command");
        assert_eq!(disposition, Disposition::Suggested);
        let calls = self.discord.calls();
        let Some(Call::Autocomplete { choices, .. }) = calls[before..].last() else {
            panic!("an autocomplete answer: {calls:?}");
        };
        choices.iter().map(pair).collect()
    }

    pub async fn run_row(&self, id: &str) -> Run {
        self.store
            .load(&Scope::All)
            .await
            .unwrap()
            .runs
            .into_iter()
            .find(|run| run.id == id)
            .expect("run")
    }
}

fn pair(choice: &CommandOptionChoice) -> (String, String) {
    let CommandOptionChoiceValue::String(value) = &choice.value else {
        panic!("string choice");
    };
    (choice.name.clone(), value.clone())
}

/// `{name, type, value}` option JSON.
pub fn opt(name: &str, value: impl Into<Value>) -> Value {
    let value = value.into();
    let kind = match &value {
        Value::Bool(_) => 5,
        _ => 3,
    };
    json!({ "name": name, "type": kind, "value": value })
}

/// A user picker option.
pub fn user_opt(name: &str, id: u64) -> Value {
    json!({ "name": name, "type": 6, "value": id.to_string() })
}

/// A focused text option (autocomplete).
pub fn focused(name: &str, text: &str) -> Value {
    json!({ "name": name, "type": 3, "value": text, "focused": true })
}

/// A subcommand wrapping `options`.
pub fn sub(name: &str, options: Value) -> Value {
    json!([{ "name": name, "type": 1, "options": options }])
}
