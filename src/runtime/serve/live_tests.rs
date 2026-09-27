//! Live serve with the Discord side wired: a scripted gateway, the fake
//! transport and a temp SQLite store. Nothing touches the network.

use std::{
    sync::{Arc, RwLock, atomic::AtomicBool},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep};
use twilight_gateway::Event;
use twilight_model::gateway::CloseFrame;
use twilight_model::gateway::payload::incoming::{
    GuildCreate, InteractionCreate, MemberUpdate, ReactionAdd, Ready, RoleDelete,
};
use twilight_model::id::Id;

use super::{
    api::{self, Composition},
    discord::{self, Discord, Wiring},
    extract,
    health::LiveHealth,
    serve_until, serve_with, store,
    tests::Temp,
};
use crate::{
    api::{
        auth::{
            self,
            staff::{GuildStaffGate, StaffCheck, StaffGate, StoreGuildMembers},
        },
        state::GuildAccess,
    },
    bot::{
        cards::{Authority, CardDesk, CardSettings, DeskDeps},
        delivery::{FixedClock, LogAlerts, StoreRef},
        gateway::{Connection, ConnectionStatus, DeliveryEligibility, EventSource, GatewayError},
        guild_cache::GuildCache,
        roster::LiveRoster,
        transport::{Call, FakeDiscord, Op, RejectionKind, Step},
    },
    domain::{
        drafts::{DraftStatus, ProposalSource, ProposalStore},
        history::Origin,
        ids::RandomIds,
        members::{GatewayMember, MemberStore, Roster},
        notify::{Claim, DeliveryJournal, DeliverySettings, NoticeOutbox, WeekReset, plan_notice},
        proposals::{Approver, ChangeKind, ProposedChange},
        schedule::{NewRun, RsvpState, RunSource, RunStatus, SchedulePolicy, StatusChange},
        scheduler::{ProposalRequest, ScheduleStore, SchedulerService, Scope, Supersede},
    },
    extract::{
        AmendmentKind,
        pipeline::{Card, CardEntry, PostResult},
    },
    infrastructure::store::SqliteStore,
    runtime::{application::HealthProbe, config::ServeConfig},
};

const GUILD: u64 = 900;
const OTHER_GUILD: u64 = 901;
const BOSSING: u64 = 10;
const ADMIN_ROLE: u64 = 20;
const SELF: u64 = 800;
const OWNER: u64 = 1003;
const ALICE: u64 = 1001;
const BOB: u64 = 1002;
const GONE: u64 = 3003;
const BOT: u64 = 1500;
const HOME_A: u64 = 301;
const HOME_B: u64 = 302;
const HOME_C: u64 = 303;

// ---- Gateway fixtures (Discord's JSON shapes) ----

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("twilight fixture")
}

fn user_json(id: u64, name: &str, bot: bool) -> Value {
    json!({
        "id": id.to_string(), "username": name, "global_name": null,
        "discriminator": "0", "avatar": null, "bot": bot,
    })
}

fn member_json(id: u64, name: &str, bot: bool, roles: &[u64]) -> Value {
    json!({
        "user": user_json(id, name, bot),
        "nick": null,
        "roles": roles.iter().map(u64::to_string).collect::<Vec<_>>(),
        "joined_at": null, "deaf": false, "mute": false, "flags": 0,
        "communication_disabled_until": null,
    })
}

fn guild_member(id: u64, name: &str, bot: bool, roles: &[u64]) -> twilight_model::guild::Member {
    parse(member_json(id, name, bot, roles))
}

fn role_json(id: u64) -> Value {
    json!({
        "color": 0,
        "colors": { "primary_color": 0, "secondary_color": null, "tertiary_color": null },
        "hoist": false, "id": id.to_string(), "managed": false, "mentionable": false,
        "name": format!("role-{id}"), "permissions": "0", "position": 1, "flags": 0,
    })
}

fn channel_json(id: u64) -> Value {
    json!({
        "id": id.to_string(), "type": 0, "name": format!("home-{id}"),
        "parent_id": null, "position": 0, "permission_overwrites": [],
    })
}

fn ready() -> Event {
    Event::Ready(parse::<Ready>(json!({
        "application": { "id": "9", "flags": 0 },
        "guilds": [],
        "resume_gateway_url": "wss://gateway.invalid",
        "session_id": "session",
        "user": {
            "id": SELF.to_string(), "username": "kanade", "discriminator": "0",
            "avatar": null, "bot": true, "mfa_enabled": false,
        },
        "v": 10,
    })))
}

fn guild_create(channels: &[u64]) -> Event {
    guild_create_with(
        channels.iter().map(|id| channel_json(*id)).collect(),
        Vec::new(),
    )
}

fn guild_create_with(channels: Vec<Value>, threads: Vec<Value>) -> Event {
    let mut guild = json!({
        "afk_channel_id": null, "afk_timeout": 300, "application_id": null, "banner": null,
        "default_message_notifications": 0, "description": null, "discovery_splash": null,
        "emojis": [], "explicit_content_filter": 0, "features": [], "icon": null,
        "id": GUILD.to_string(), "mfa_level": 0, "name": "guild", "nsfw_level": 0,
        "owner_id": OWNER.to_string(), "preferred_locale": "en-US",
        "premium_progress_bar_enabled": false, "premium_tier": 0,
        "public_updates_channel_id": null,
        "roles": [role_json(GUILD), role_json(BOSSING), role_json(ADMIN_ROLE)],
        "rules_channel_id": null, "splash": null, "system_channel_flags": 0,
        "system_channel_id": null, "verification_level": 0, "vanity_url_code": null,
    });
    for list in [
        "presences",
        "stickers",
        "voice_states",
        "threads",
        "members",
        "guild_scheduled_events",
        "stage_instances",
    ] {
        guild[list] = json!([]);
    }
    guild["channels"] = json!(channels);
    guild["threads"] = json!(threads);
    Event::GuildCreate(Box::new(parse::<GuildCreate>(guild)))
}

fn member_update(guild: u64, id: u64, name: &str, roles: &[u64]) -> Event {
    Event::MemberUpdate(Box::new(parse::<MemberUpdate>(json!({
        "guild_id": guild.to_string(),
        "user": user_json(id, name, false),
        "nick": null,
        "roles": roles.iter().map(u64::to_string).collect::<Vec<_>>(),
        "joined_at": null, "premium_since": null, "avatar": null,
        "communication_disabled_until": null,
    }))))
}

fn role_delete(role: u64) -> Event {
    Event::RoleDelete(parse::<RoleDelete>(
        json!({ "guild_id": GUILD.to_string(), "role_id": role.to_string() }),
    ))
}

fn reaction(guild: Option<u64>, channel: u64, message: &str, user: u64, emoji: &str) -> Event {
    Event::ReactionAdd(Box::new(ReactionAdd(parse(json!({
        "burst": false,
        "channel_id": channel.to_string(),
        "emoji": { "id": null, "name": emoji },
        "guild_id": guild.map(|id| id.to_string()),
        "member": guild.map(|_| member_json(user, "someone", false, &[BOSSING])),
        "message_id": message,
        "user_id": user.to_string(),
    })))))
}

const COMMAND: u8 = 2;
const AUTOCOMPLETE: u8 = 4;

/// A slash interaction (`kind` 2) or its autocomplete (4) by `user` holding
/// the bossing and admin roles, for a command registered to `registered`.
fn slash(
    kind: u8,
    id: u64,
    guild: u64,
    registered: Option<u64>,
    user: u64,
    name: &str,
    options: Value,
) -> Event {
    let mut member = member_json(user, "someone", false, &[BOSSING, ADMIN_ROLE]);
    member["permissions"] = json!("0");
    let interaction = parse(json!({
        "application_id": "9",
        "authorizing_integration_owners": {},
        "entitlements": [],
        "id": id.to_string(),
        "type": kind,
        "token": "interaction-secret-token",
        "guild_id": guild.to_string(),
        "channel": { "id": HOME_A.to_string(), "type": 0 },
        "member": member,
        "data": {
            "id": "5500", "name": name, "type": 1,
            "guild_id": registered.map(|id| id.to_string()),
            "options": options,
        },
    }));
    Event::InteractionCreate(Box::new(InteractionCreate(interaction)))
}

/// What S11's `register_retained` registers, in order.
const RETAINED: [&str; 13] = [
    "fixed", "debug", "schedule", "amend", "swap", "status", "rsvp", "nick", "pings", "style",
    "limits", "rescan", "say",
];

// ---- Harness ----

/// Gateway events from the test; closing ends the stream.
struct Script {
    events: mpsc::UnboundedReceiver<Event>,
    closing: bool,
}

impl EventSource for Script {
    async fn next_event(&mut self) -> Option<Result<Event, GatewayError>> {
        if self.closing {
            return None;
        }
        let event = self.events.recv().await?;
        // Twilight ends the stream after a fatal close frame.
        if let Event::GatewayClose(Some(frame)) = &event {
            self.closing = frame.code >= 4000;
        }
        Some(Ok(event))
    }

    fn close(&mut self) {
        self.closing = true;
    }
}

fn script() -> (mpsc::UnboundedSender<Event>, Script) {
    let (sender, events) = mpsc::unbounded_channel();
    (
        sender,
        Script {
            events,
            closing: false,
        },
    )
}

struct Harness {
    /// Keeps the temp directory alive.
    _temp: Temp,
    config: ServeConfig,
    fake: Arc<FakeDiscord>,
    timing: extract::Timing,
}

struct Ctx {
    store: Arc<SqliteStore>,
    events: mpsc::UnboundedSender<Event>,
    connection: ConnectionStatus,
    access: Arc<GuildAccess>,
    health: Arc<dyn HealthProbe>,
    composition: Composition,
}

const TICK: Duration = Duration::from_millis(50);

impl Harness {
    fn new() -> Self {
        Self::with(&[])
    }

    fn with(extra: &[(&str, &str)]) -> Self {
        let temp = Temp::new();
        let mut values = vec![
            ("KANADE_DISCORD_GATEWAY", "1"),
            ("KANADE_EXPECT_V4_STOPPED", "1"),
            ("KANADE_ADMIN_ROLE_ID", "20"),
        ];
        values.extend_from_slice(extra);
        let config = temp.config(&values);
        Self {
            _temp: temp,
            config,
            fake: Arc::new(FakeDiscord::new()),
            timing: extract::Timing::default(),
        }
    }

    async fn seed(&self, work: impl AsyncFnOnce(&SqliteStore)) {
        let store = store::open(&self.config.store).await.unwrap();
        work(&store).await;
        store::close(store, Duration::ZERO).await;
    }

    async fn start(&self) -> (Discord, Ctx) {
        let store = store::open(&self.config.store).await.unwrap();
        let prepared = discord::prepare(&self.config, TICK);
        let connection = prepared.probe.connection.clone();
        let health = LiveHealth::new(store.clone())
            .with_discord(prepared.probe.clone(), prepared.tick_status.clone())
            .with_extraction(prepared.extraction.clone());
        let mut composition =
            api::compose(&self.config, store.clone(), prepared.cache.clone(), health)
                .await
                .unwrap();
        let (events, source) = script();
        let wiring = Wiring {
            source,
            transport: Arc::clone(&self.fake),
            clock: Arc::new(auth::system_now),
            tick: TICK,
            extraction: self.timing,
        };
        let discord = discord::start(
            &self.config,
            store.clone(),
            &mut composition,
            prepared,
            wiring,
        )
        .await
        .unwrap();
        let ctx = Ctx {
            store,
            events,
            connection,
            access: Arc::clone(&composition.access),
            health: Arc::clone(&composition.admin.health),
            composition,
        };
        (discord, ctx)
    }

    fn creates_in(&self, channel: u64) -> Vec<(String, Option<u64>)> {
        self.fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Create {
                    channel: to,
                    message,
                    outcome,
                } if to.get() == channel => Some((
                    message.content.unwrap_or_default(),
                    match outcome {
                        crate::bot::transport::Outcome::Delivered(id) => Some(id.get()),
                        _ => None,
                    },
                )),
                _ => None,
            })
            .collect()
    }
}

/// Run the Discord side while `script` runs, then stop it in order.
async fn drive(discord: &mut Discord, script: impl Future<Output = ()>) {
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::join!(
        discord.until(async {
            let _ = stopped.await;
        }),
        async {
            script.await;
            let _ = stop.send(());
        }
    );
}

/// Release every store handle and close the store; it must open again.
async fn finish(harness: &Harness, ctx: Ctx) {
    let Ctx {
        store,
        events,
        connection,
        access,
        health,
        composition,
    } = ctx;
    drop((events, connection, access, health, composition));
    store::close(store, Duration::ZERO).await;
    // Closed, so ownership was released: it opens again.
    let reopened = store::open(&harness.config.store)
        .await
        .expect("every store handle was released");
    store::close(reopened, Duration::ZERO).await;
}

macro_rules! eventually {
    ($what:expr, $check:expr) => {{
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if $check {
                break;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {}", $what);
            sleep(Duration::from_millis(20)).await;
        }
    }};
}

async fn staff(ctx: &Ctx, user: u64) -> StaffCheck {
    GuildStaffGate::new(
        ctx.access.policy.clone(),
        Arc::new(StoreGuildMembers::new(
            Arc::clone(&ctx.store),
            Arc::clone(&ctx.access),
        )),
    )
    .check(&user.to_string())
    .await
}

async fn roles_of(store: &SqliteStore, user: u64) -> Option<Vec<String>> {
    store
        .load_member(&user.to_string())
        .await
        .unwrap()
        .map(|row| row.roles)
}

fn gateway_member(id: u64, name: &str, roles: &[u64]) -> GatewayMember {
    GatewayMember {
        user_id: id.to_string(),
        display_name: Some(name.into()),
        has_role: roles.contains(&BOSSING),
        roles: roles.iter().map(u64::to_string).collect(),
        ..GatewayMember::default()
    }
}

// ---- Gateway, commands and roster ----

#[tokio::test]
async fn guild_create_registers_guild_commands_reconciles_and_drops_other_guilds() {
    let harness = Harness::new();
    harness
        .seed(async |store| {
            store
                .apply_gateway(gateway_member(GONE, "gone", &[BOSSING]))
                .await
                .unwrap();
            store
                .apply_gateway(gateway_member(BOB, "old bob", &[BOSSING]))
                .await
                .unwrap();
        })
        .await;
    harness.fake.seed_members(
        Id::new(GUILD),
        vec![
            guild_member(ALICE, "alice", false, &[BOSSING, ADMIN_ROLE]),
            guild_member(BOB, "bob", false, &[BOSSING]),
            guild_member(BOT, "helper", true, &[BOSSING]),
        ],
    );
    let fake = &harness.fake;
    let (mut discord, ctx) = harness.start().await;
    drive(&mut discord, async {
        let before = ctx.health.health().await;
        assert_eq!(before.discord, "connecting");
        assert_eq!(before.scheduler, "starting");
        assert_eq!(before.status, "degraded");
        assert_eq!(
            staff(&ctx, ALICE).await,
            StaffCheck::NotStaff,
            "no member data before reconciliation"
        );

        for event in [
            ready(),
            // Noise: another guild and a DM.
            member_update(OTHER_GUILD, ALICE, "alice", &[BOSSING]),
            slash(
                COMMAND,
                1,
                OTHER_GUILD,
                Some(OTHER_GUILD),
                ALICE,
                "schedule",
                json!([]),
            ),
            reaction(None, HOME_A, "4242", ALICE, "✅"),
            guild_create(&[HOME_A]),
            // A global command is not ours even in this guild.
            slash(COMMAND, 2, GUILD, None, ALICE, "schedule", json!([])),
            slash(COMMAND, 3, GUILD, Some(GUILD), ALICE, "schedule", json!([])),
        ] {
            ctx.events.send(event).unwrap();
        }
        eventually!("the command reply", fake.count(Op::Respond) == 1);
        eventually!(
            "reconciliation",
            staff(&ctx, ALICE).await == StaffCheck::Staff
                && !ctx
                    .store
                    .load_member(&GONE.to_string())
                    .await
                    .unwrap()
                    .unwrap()
                    .member
                    .has_role
        );
        eventually!("the tick", ctx.health.health().await.status == "ok");

        let registers: Vec<_> = fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Register {
                    guild, commands, ..
                } => Some((
                    guild.get(),
                    commands.into_iter().map(|c| c.name).collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            registers,
            vec![(GUILD, RETAINED.map(str::to_owned).to_vec())],
            "the retained set, once, for this guild only"
        );
        assert!(fake.calls().iter().all(|call| match call {
            Call::ListMembers { guild, .. } => guild.get() == GUILD,
            _ => true,
        }));

        let bob = ctx
            .store
            .load_member(&BOB.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bob.member.display_name.as_deref(), Some("bob"));
        assert!(
            ctx.store
                .load_member(&BOT.to_string())
                .await
                .unwrap()
                .is_none(),
            "bots never join the roster"
        );
        assert_eq!(ctx.access.owner(), Some(Id::new(OWNER)));

        let health = ctx.health.health().await;
        assert_eq!(
            (health.discord, health.scheduler, health.storage),
            ("ready", "running", "ok")
        );
        let dropped = health.dropped_events.unwrap();
        assert_eq!((dropped.other_guild, dropped.no_guild), (2, 1));
        assert!(health.last_tick_age_seconds.is_some());
    })
    .await;
    assert_eq!(
        discord.steps(),
        [
            "gateway_closed",
            "chat_stopped",
            "extraction_stopped",
            "workers_stopped",
            "tick_stopped"
        ]
    );
    assert!(discord.result().is_ok());
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn roster_writes_keep_gateway_order_and_a_deleted_admin_role_revokes() {
    let harness = Harness::new();
    // The member list is older than the update that follows GUILD_CREATE.
    harness.fake.seed_members(
        Id::new(GUILD),
        vec![
            guild_member(ALICE, "alice", false, &[BOSSING]),
            guild_member(BOB, "bob", false, &[BOSSING]),
            guild_member(OWNER, "owner", false, &[]),
        ],
    );
    let (mut discord, ctx) = harness.start().await;
    drive(&mut discord, async {
        for event in [
            ready(),
            guild_create(&[]),
            member_update(GUILD, ALICE, "alice", &[BOSSING, ADMIN_ROLE]),
            member_update(GUILD, BOB, "bobby", &[BOSSING]),
        ] {
            ctx.events.send(event).unwrap();
        }
        // Bob's update is the last job: once it is stored, every earlier
        // one (including reconciliation) has been applied.
        eventually!(
            "bob's update",
            ctx.store
                .load_member(&BOB.to_string())
                .await
                .unwrap()
                .is_some_and(|row| row.member.display_name.as_deref() == Some("bobby"))
        );
        assert_eq!(
            roles_of(&ctx.store, ALICE).await.unwrap(),
            ["10", "20"],
            "reconciliation never lands over the newer update"
        );
        // Discord OAuth sign-in's staff gate over the reconciled rows.
        assert_eq!(staff(&ctx, ALICE).await, StaffCheck::Staff);
        assert_eq!(staff(&ctx, BOB).await, StaffCheck::NotStaff);
        assert_eq!(staff(&ctx, OWNER).await, StaffCheck::Staff, "the owner");

        // Discord sends no member updates for a deleted role.
        ctx.events.send(role_delete(ADMIN_ROLE)).unwrap();
        eventually!(
            "revocation",
            staff(&ctx, ALICE).await == StaffCheck::NotStaff
        );
        assert_eq!(roles_of(&ctx.store, ALICE).await.unwrap(), ["10"]);
        let alice = ctx
            .store
            .load_member(&ALICE.to_string())
            .await
            .unwrap()
            .unwrap();
        assert!(alice.member.has_role, "the bossing role still exists");
    })
    .await;
    finish(&harness, ctx).await;
}

// ---- Delivery tick, reactions and cards ----

fn policy(harness: &Harness) -> SchedulePolicy {
    super::settings::seed(&harness.config.seeds).schedule_policy(harness.config.runtime.timezone)
}

fn week_of(policy: &SchedulePolicy, now: DateTime<Utc>) -> DateTime<Utc> {
    WeekReset {
        zone: policy.zone(),
        weekday: policy.reset_weekday,
        time: policy.reset_time,
    }
    .current_week(now)
    .unwrap()
}

async fn create_run(
    store: &SqliteStore,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
    channel: u64,
    at: DateTime<Utc>,
) -> String {
    SchedulerService::new(store, RandomIds, FixedClock(now))
        .with_attendance(policy.attendance)
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(channel.to_string()),
            week_start: week_of(policy, now),
            datetime: at,
            bosses: vec!["Kalos".into()],
            participants: vec![ALICE.to_string()],
            status: RunStatus::Planned,
            source: RunSource::Fixed,
        })
        .await
        .unwrap()
}

/// A cancelled run in `channel` with its channel notice in the outbox.
async fn cancelled_with_notice(
    store: &SqliteStore,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
    channel: u64,
) {
    let run = create_run(
        store,
        policy,
        now,
        channel,
        now + chrono::Duration::hours(3),
    )
    .await;
    SchedulerService::new(store, RandomIds, FixedClock(now))
        .with_attendance(policy.attendance)
        .as_origin(Origin::for_tests())
        .set_status(
            &run,
            StatusChange {
                status: RunStatus::Cancelled,
                announce: true,
                via_portal: true,
            },
            &policy.reminders,
        )
        .await
        .unwrap();
}

/// The message of `run`'s 60-minute countdown, once posted.
async fn countdown_message(store: &SqliteStore, run: &str) -> Option<String> {
    store
        .load(&Scope::All)
        .await
        .unwrap()
        .reminders
        .into_iter()
        .find(|row| row.run_id == run && row.kind == "countdown_60")
        .and_then(|row| row.message_id)
}

fn controlled_tick(
    harness: &Harness,
    store: Arc<SqliteStore>,
    cache: Arc<GuildCache>,
    roster: Arc<LiveRoster>,
    now: DateTime<Utc>,
    policy: SchedulePolicy,
) -> super::tick::TickLoop<FakeDiscord> {
    let clock_now = now;
    super::tick::TickLoop {
        store,
        transport: Arc::clone(&harness.fake),
        cache,
        roster,
        clock: Arc::new(move || clock_now),
        period: TICK,
        seeds: harness.config.seeds.clone(),
        config: super::tick::delivery_config(
            &harness.config.instance_id,
            policy,
            &super::settings::seed(&harness.config.seeds),
        ),
        status: Arc::new(super::tick::TickStatus::new(TICK)),
        claim_gate: Arc::new(|| Some(DeliveryEligibility::unguarded())),
        cards: Default::default(),
        quiet: Arc::new(AtomicBool::new(false)),
        post_channel: Arc::new(RwLock::new(None)),
    }
}

fn cache_with_test_channel() -> Arc<GuildCache> {
    let cache = Arc::new(GuildCache::new(Id::new(GUILD)));
    let Event::GuildCreate(create) = guild_create(&[HOME_C]) else {
        unreachable!();
    };
    let GuildCreate::Available(guild) = *create else {
        unreachable!();
    };
    cache.reset(&guild);
    cache
}

#[tokio::test]
async fn first_tick_waits_for_roster_reconciliation_before_sending() {
    let harness = Harness::new();
    let policy = policy(&harness);
    let now = auth::system_now();
    let store = store::open(&harness.config.store).await.unwrap();
    let start = now + chrono::Duration::seconds(59 * 60 + 30);
    let run = create_run(&store, &policy, now, HOME_C, start).await;
    SchedulerService::new(&store, RandomIds, FixedClock(now))
        .with_attendance(policy.attendance)
        .as_origin(Origin::for_tests())
        .add_reminder(
            &run,
            "countdown_60",
            start - chrono::Duration::minutes(60),
            None,
        )
        .await
        .unwrap()
        .expect("a new reminder");
    super::tick::recover(&store, now).await.unwrap();

    let cache = cache_with_test_channel();
    let roster = Arc::new(LiveRoster::new(Arc::clone(&cache)));
    let tick = controlled_tick(
        &harness,
        Arc::clone(&store),
        cache,
        Arc::clone(&roster),
        now,
        policy,
    );
    let (guild_ready, ready) = watch::channel(false);
    let (stopped, stop) = watch::channel(false);
    let tick = tick.run(ready, stop);
    tokio::pin!(tick);
    let control = async {
        guild_ready.send_replace(true);
        tokio::task::yield_now().await;
        sleep(TICK * 4).await;
        tokio::task::yield_now().await;
        assert_eq!(harness.fake.count(Op::Create), 0, "no pre-reconcile send");

        roster.mark_reconciled();
        eventually!("the first delivery", harness.fake.count(Op::Create) == 1);
        assert_eq!(harness.fake.count(Op::Create), 1);
        stopped.send_replace(true);
    };
    tokio::join!(tick, control);
    store::close(store, Duration::ZERO).await;
}

#[tokio::test]
async fn stop_interrupts_a_tick_waiting_for_reconciliation() {
    let harness = Harness::new();
    let policy = policy(&harness);
    let now = auth::system_now();
    let store = store::open(&harness.config.store).await.unwrap();
    super::tick::recover(&store, now).await.unwrap();

    let cache = cache_with_test_channel();
    let roster = Arc::new(LiveRoster::new(Arc::clone(&cache)));
    let tick = controlled_tick(&harness, Arc::clone(&store), cache, roster, now, policy);
    let (guild_ready, ready) = watch::channel(false);
    let (stopped, stop) = watch::channel(false);
    let tick = tick.run(ready, stop);
    tokio::pin!(tick);
    guild_ready.send_replace(true);
    tokio::select! {
        biased;
        () = &mut tick => panic!("tick stopped before the stop signal"),
        _ = tokio::task::yield_now() => {}
    }
    stopped.send_replace(true);

    tokio::time::timeout(Duration::from_secs(1), &mut tick)
        .await
        .expect("shutdown must not wait for reconciliation");
    assert!(harness.fake.calls().is_empty());
    store::close(store, Duration::ZERO).await;
}

#[tokio::test]
async fn the_tick_sends_due_work_once_and_never_resends_an_interrupted_send() {
    let harness = Harness::new();
    // An unsuccessful roster fetch still completes the reconciliation attempt.
    harness
        .fake
        .script(Op::ListMembers, Step::Reject(RejectionKind::MissingAccess));
    let policy = policy(&harness);
    let now = auth::system_now();
    let mut due_run = String::new();
    harness
        .seed(async |store| {
            cancelled_with_notice(store, &policy, now, HOME_A).await;
            cancelled_with_notice(store, &policy, now, HOME_B).await;
            // Its 60-minute countdown (the default policy) fell due 30 s ago
            // and is unsent (a row created in the past would be caught up).
            let start = now + chrono::Duration::seconds(59 * 60 + 30);
            due_run = create_run(store, &policy, now, HOME_C, start).await;
            SchedulerService::new(store, RandomIds, FixedClock(now))
                .with_attendance(policy.attendance)
                .as_origin(Origin::for_tests())
                .add_reminder(
                    &due_run,
                    "countdown_60",
                    start - chrono::Duration::minutes(60),
                    None,
                )
                .await
                .unwrap()
                .expect("a new reminder");

            // A previous process claimed HOME_A's notice and died mid-send.
            let row = store
                .pending_notices()
                .await
                .unwrap()
                .notices
                .into_iter()
                .find(|row| row.notice.channel_id.as_deref() == Some("301"))
                .unwrap();
            let channels = ["301".to_owned()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            let intent = plan_notice(
                &row.notice,
                &Roster::new(),
                &channels,
                DeliverySettings {
                    post_channel_id: None,
                    quiet_mode: false,
                    attendance: policy.attendance,
                },
            )
            .unwrap();
            let lease = store
                .begin_lease("crashed", "scheduler_tick", now)
                .await
                .unwrap();
            let claim = store
                .claim_source(&lease, &intent, &row.source, row.ordinal, now)
                .await
                .unwrap();
            assert!(matches!(claim, Claim::Fresh(_)));
        })
        .await;

    let (mut discord, ctx) = harness.start().await;
    drive(&mut discord, async {
        // Recovery ran in `start`, before the gateway, reactions or commands
        // could send: nothing is left in flight to recover now.
        let again = ctx
            .store
            .recover_on_start(auth::system_now())
            .await
            .unwrap();
        assert!(again.indeterminate.is_empty() && again.orphaned_leases == 0);
        sleep(TICK * 4).await;
        assert!(
            harness.fake.calls().is_empty(),
            "nothing is sent before the gateway is ready"
        );
        ctx.events.send(ready()).unwrap();
        ctx.events
            .send(guild_create(&[HOME_A, HOME_B, HOME_C]))
            .unwrap();
        eventually!(
            "the notice and the reminder",
            harness.creates_in(HOME_B).len() == 1
                && countdown_message(&ctx.store, &due_run).await.is_some()
        );
        // Several more ticks: nothing is sent again.
        sleep(TICK * 8).await;
        assert!(
            harness.creates_in(HOME_A).is_empty(),
            "the interrupted send is indeterminate, never resent"
        );
        assert_eq!(harness.creates_in(HOME_B).len(), 1, "notice drained once");
        let message = countdown_message(&ctx.store, &due_run).await.unwrap();
        let posts = harness
            .creates_in(HOME_C)
            .into_iter()
            .filter(|(_, id)| id.map(|id| id.to_string()).as_deref() == Some(message.as_str()))
            .count();
        assert_eq!(posts, 1, "the reminder was posted exactly once");
        assert!(
            ctx.store
                .pending_notices()
                .await
                .unwrap()
                .notices
                .is_empty(),
            "both notices drained"
        );

        // ❌ on the reminder card is Alice's RSVP.
        ctx.events
            .send(reaction(Some(GUILD), HOME_C, &message, ALICE, "❌"))
            .unwrap();
        eventually!(
            "the RSVP",
            ctx.store
                .load(&Scope::All)
                .await
                .unwrap()
                .rsvps
                .iter()
                .any(|rsvp| {
                    rsvp.run_id == due_run
                        && rsvp.user_id == ALICE.to_string()
                        && rsvp.state == RsvpState::No
                })
        );
        let health = ctx.health.health().await;
        assert_eq!(health.scheduler, "running");
    })
    .await;
    assert_eq!(
        discord.steps(),
        [
            "gateway_closed",
            "chat_stopped",
            "extraction_stopped",
            "workers_stopped",
            "tick_stopped"
        ]
    );
    finish(&harness, ctx).await;
}

struct AnyAuthority;

impl Authority for AnyAuthority {
    fn approver(&self, user_id: &str) -> Approver {
        Approver {
            user_id: user_id.to_owned(),
            has_role: true,
            is_admin: false,
            via_portal: false,
        }
    }
}

#[tokio::test]
async fn a_check_on_a_proposal_card_is_approved_through_the_desk_not_via_portal() {
    let harness = Harness::new();
    harness.fake.seed_members(
        Id::new(GUILD),
        vec![guild_member(ALICE, "alice", false, &[BOSSING, ADMIN_ROLE])],
    );
    let policy = policy(&harness);
    let (mut discord, ctx) = harness.start().await;
    drive(&mut discord, async {
        let now = auth::system_now();
        let at = now + chrono::Duration::hours(30);
        let run = create_run(&ctx.store, &policy, now, HOME_C, at).await;
        let mut roster = Roster::new();
        roster.upsert(crate::domain::members::Member {
            user_id: ALICE.to_string(),
            display_name: Some("alice".into()),
            has_role: true,
            ..Default::default()
        });
        let change = ProposedChange {
            run_id: Some(run.clone()),
            channel_id: Some(HOME_C.to_string()),
            bosses: vec!["Kalos".into()],
            participants: vec![ALICE.to_string()],
            new_datetime: Some(at + chrono::Duration::hours(1)),
            ..ProposedChange::new(ChangeKind::Move)
        };
        let proposed = SchedulerService::new(StoreRef(&*ctx.store), RandomIds, FixedClock(now))
            .with_attendance(policy.attendance)
            .propose(
                ProposalRequest {
                    change: change.clone(),
                    source: ProposalSource::Extraction,
                    source_id: "x-1".into(),
                    supersede: Supersede::Keep,
                },
                &policy,
                &roster,
            )
            .await
            .unwrap();
        let proposal = proposed.proposal.id;
        let desk = CardDesk::new(
            DeskDeps {
                store: Arc::clone(&ctx.store),
                transport: Arc::clone(&harness.fake),
                ids: RandomIds,
                clock: Arc::new(FixedClock(now)),
                directory: Arc::new(roster),
                authority: Arc::new(AnyAuthority),
                alerts: Arc::new(LogAlerts),
            },
            CardSettings {
                zone: policy.zone(),
                policy: policy.clone(),
                instance_id: "seed".into(),
            },
        );
        let posted = desk
            .post_card(&Card {
                channel_id: HOME_C.to_string(),
                entries: vec![CardEntry {
                    proposal_id: proposal.clone(),
                    change,
                    kind: AmendmentKind::Move,
                    run_id: Some(run),
                    summary: "an hour later".into(),
                    is_question: false,
                    needs_answer: false,
                    confidence: 0.9,
                    also_mentioned: Vec::new(),
                    day_ref: None,
                    time_ref: None,
                    evidence_message_ids: vec!["101".into()],
                    self_service: None,
                }],
                superseded: Vec::new(),
            })
            .await;
        assert_eq!(posted, PostResult::Posted);
        drop(desk);
        let message = crate::domain::proposals::ProposalCardStore::load_cards(
            &*ctx.store,
            std::slice::from_ref(&proposal),
        )
        .await
        .unwrap()
        .pop()
        .and_then(|card| card.message_id)
        .unwrap();

        ctx.events.send(ready()).unwrap();
        ctx.events.send(guild_create(&[HOME_C])).unwrap();
        eventually!(
            "reconciliation",
            staff(&ctx, ALICE).await == StaffCheck::Staff
        );
        // The roster snapshot follows the store after its queue empties and
        // on every tick.
        sleep(TICK * 4).await;
        ctx.events
            .send(reaction(Some(GUILD), HOME_C, &message, ALICE, "✅"))
            .unwrap();
        eventually!(
            "the approval",
            ctx.store
                .load_proposal(&proposal)
                .await
                .unwrap()
                .unwrap()
                .0
                .draft
                .status
                == DraftStatus::Merged
        );
        let written = ctx.store.outbox_notices().await.unwrap();
        assert!(!written.is_empty());
        assert!(
            written.iter().all(|row| !row.notice.via_portal),
            "a ✅ on the card is never via portal"
        );
        eventually!(
            "the merge notice",
            ctx.store
                .pending_notices()
                .await
                .unwrap()
                .notices
                .is_empty()
                && harness.creates_in(HOME_C).len() >= 2
        );
        assert!(
            harness
                .creates_in(HOME_C)
                .iter()
                .all(|(content, _)| !content.contains("via portal"))
        );
        assert!(harness.fake.calls().iter().any(|call| matches!(
            call,
            Call::Edit { edit, .. }
                if edit.content.as_deref().is_some_and(|text| text.contains("✅ applied by"))
        )));
    })
    .await;
    finish(&harness, ctx).await;
}

// ---- Startup guards and fatal closes ----

#[tokio::test]
async fn serve_refuses_to_connect_until_v4_is_stopped() {
    let temp = Temp::new();
    let config = temp.config(&[("KANADE_DISCORD_GATEWAY", "1")]);
    let error = serve_until(config, std::future::pending())
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "KANADE_EXPECT_V4_STOPPED must be 1: stop the v4 container first"
    );
    assert!(!temp.0.join("db").exists(), "the store was not opened");
}

#[tokio::test]
async fn a_disallowed_intents_close_keeps_serving_http_without_reconnecting() {
    let harness = Harness::new();
    let (events, source) = script();
    events.send(ready()).unwrap();
    events.send(guild_create(&[])).unwrap();
    events
        .send(Event::GatewayClose(Some(CloseFrame::new(
            4014,
            "Disallowed intent(s).",
        ))))
        .unwrap();
    let wiring = Wiring {
        source,
        transport: Arc::clone(&harness.fake),
        clock: Arc::new(auth::system_now),
        tick: TICK,
        extraction: extract::Timing::default(),
    };
    // Serve stays up (no exit, so no restart re-IDENTIFYing) until shutdown.
    let started = Instant::now();
    serve_with(&harness.config, sleep(Duration::from_millis(400)), wiring)
        .await
        .expect("a fatal close does not fail serve");
    assert!(started.elapsed() >= Duration::from_millis(400));
    drop(events);
    let store = store::open(&harness.config.store).await.unwrap();
    store::close(store, Duration::ZERO).await;
}

#[tokio::test]
async fn a_fatal_close_reports_closed_health_and_stops_the_discord_side() {
    let harness = Harness::new();
    let (mut discord, ctx) = harness.start().await;
    drive(&mut discord, async {
        ctx.events.send(ready()).unwrap();
        ctx.events.send(guild_create(&[])).unwrap();
        eventually!("ready", ctx.health.health().await.status == "ok");
        ctx.events
            .send(Event::GatewayClose(Some(CloseFrame::new(
                4004,
                "Authentication failed.",
            ))))
            .unwrap();
        eventually!(
            "closed health",
            ctx.health.health().await.discord == "closed"
        );
        let health = ctx.health.health().await;
        assert_eq!(health.status, "degraded");
        assert_eq!(health.storage, "ok", "the store keeps serving the portal");
        eventually!(
            "the tick stops",
            ctx.health.health().await.scheduler == "stopped"
        );
        // Still up afterwards: nothing reconnects and nothing exits.
        sleep(TICK * 4).await;
        assert_eq!(ctx.health.health().await.discord, "closed");
    })
    .await;
    assert_eq!(
        discord.steps(),
        [
            "gateway_closed",
            "chat_stopped",
            "extraction_stopped",
            "workers_stopped",
            "tick_stopped"
        ]
    );
    assert!(discord.result().is_ok());
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn retained_commands_and_their_autocomplete_dispatch_through_the_registry() {
    let harness = Harness::new();
    let fake = &harness.fake;
    let (mut discord, ctx) = harness.start().await;
    drive(&mut discord, async {
        ctx.events.send(ready()).unwrap();
        ctx.events.send(guild_create(&[HOME_A])).unwrap();
        eventually!("registration", fake.count(Op::Register) == 1);
        ctx.events
            .send(slash(
                COMMAND,
                10,
                GUILD,
                Some(GUILD),
                ALICE,
                "schedule",
                json!([]),
            ))
            .unwrap();
        ctx.events
            .send(slash(
                AUTOCOMPLETE,
                11,
                GUILD,
                Some(GUILD),
                ALICE,
                "amend",
                json!([{ "name": "run_id", "type": 3, "value": "", "focused": true }]),
            ))
            .unwrap();
        // A redelivered interaction (same id) is answered once.
        ctx.events
            .send(slash(
                COMMAND,
                10,
                GUILD,
                Some(GUILD),
                ALICE,
                "schedule",
                json!([]),
            ))
            .unwrap();
        eventually!(
            "the reply and the choices",
            fake.count(Op::Respond) == 1 && fake.count(Op::Autocomplete) == 1
        );
        sleep(TICK * 2).await;
        assert_eq!(
            fake.count(Op::Respond),
            1,
            "redelivery is not answered again"
        );
        let reply = fake
            .calls()
            .into_iter()
            .find_map(|call| match call {
                Call::Respond { reply, .. } => Some(reply),
                _ => None,
            })
            .unwrap();
        assert!(!reply.content.is_empty() || !reply.embeds.is_empty());
        assert_ne!(
            reply.content,
            crate::bot::commands::GENERIC_FAILURE,
            "/schedule ran through its S11 handler"
        );
    })
    .await;
    finish(&harness, ctx).await;
}

mod extraction;
mod outage;
