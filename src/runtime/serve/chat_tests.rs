//! Live chat in serve: a scripted gateway, the fake transport, a loopback
//! model gateway and a temp store. Nothing touches the network.

use std::{
    collections::VecDeque,
    fs,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep};
use twilight_gateway::Event;
use twilight_model::gateway::payload::incoming::{GuildCreate, MessageCreate, Ready};

use super::{
    api::{self, Composition},
    chat::LiveStrategyGuides,
    discord::{self, Discord, Wiring},
    extract,
    health::LiveHealth,
    store,
    tests::Temp,
};
use crate::{
    api::auth,
    bot::{
        gateway::{EventSource, GatewayError},
        transport::{Call, FakeDiscord, Op, Outcome},
    },
    chat::tools::read::{GuideError, StrategyGuides},
    domain::{
        catalog::BossReference,
        members::{Member, MemberProfile, MemberStore},
        model_log::{ChatFilter, ChatInteraction, ChatOutcome, ModelLogStore},
        settings::{SettingsStore, keys},
    },
    infrastructure::{
        files::{load_catalog, load_knowledge_dir},
        store::SqliteStore,
    },
    runtime::application::HealthProbe,
};

const GUILD: u64 = 900;
const SELF: u64 = 800;
const OWNER: u64 = 1003;
const ALICE: u64 = 1001;
const PILOT_ROLE: u64 = 30;
const CATEGORY: u64 = 40;
const CHANNEL: u64 = 50;
const THREAD: u64 = 60;
const BOT_ROLE: u64 = 35;
const ROLE_PRIORITY_FIRST: u64 = 41;
const ROLE_PRIORITY_SECOND: u64 = 42;
const ROLE_NOT_HELD: u64 = 43;
const ALIAS: &str = "home-chat";
/// A second homelab alias a saved switch can move chat to.
const OTHER: &str = "home-chat-b";

// ---- Loopback model gateway ----

struct ModelStub {
    addr: SocketAddr,
    completions: Arc<Mutex<Vec<Value>>>,
}

impl ModelStub {
    /// Lists `ALIAS` in `zone`; every completion answers `reply`.
    async fn start(zone: &'static str, reply: &'static str) -> Self {
        Self::scripted(zone, reply, Vec::new()).await
    }

    /// Lists `ALIAS` in `zone`; scripted completions go out before `reply`.
    async fn scripted(zone: &'static str, reply: &'static str, scripted: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let completions = Arc::new(Mutex::new(Vec::new()));
        let scripted: Arc<Mutex<VecDeque<Value>>> = Arc::new(Mutex::new(scripted.into()));
        let seen = Arc::clone(&completions);
        let queued = Arc::clone(&scripted);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                let queued = Arc::clone(&queued);
                tokio::spawn(async move {
                    let Some((path, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let answer = if path.ends_with("/models") {
                        listing(zone)
                    } else {
                        let model = body["model"].as_str().unwrap_or(ALIAS).to_owned();
                        seen.lock().unwrap().push(body);
                        queued
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or_else(|| completion(&model, reply))
                    };
                    let text = answer.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { addr, completions }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn completions(&self) -> usize {
        self.completions.lock().unwrap().len()
    }

    /// The first request's tool names and its `request_tools` bundle enum.
    fn first_surface(&self) -> (Vec<String>, Value) {
        let bodies = self.completions.lock().unwrap();
        let tools = bodies[0]["tools"].as_array().unwrap();
        let names = tools
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
            .collect();
        let request = tools
            .iter()
            .find(|tool| tool["function"]["name"] == "request_tools")
            .unwrap();
        let bundles = request["function"]["parameters"]["properties"]["bundle"]["enum"].clone();
        (names, bundles)
    }

    /// `(model, reasoning_effort)` of every completion request so far.
    fn sent(&self) -> Vec<(String, Option<String>)> {
        self.completions
            .lock()
            .unwrap()
            .iter()
            .map(|body| {
                (
                    body["model"].as_str().unwrap_or_default().to_owned(),
                    body["reasoning_effort"].as_str().map(str::to_owned),
                )
            })
            .collect()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, Value)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = stream
            .read(&mut chunk)
            .await
            .ok()
            .filter(|read| *read > 0)?;
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let path = head.split(' ').nth(1)?.to_owned();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[end + 4..].to_vec();
    while body.len() < length {
        let read = stream
            .read(&mut chunk)
            .await
            .ok()
            .filter(|read| *read > 0)?;
        body.extend_from_slice(&chunk[..read]);
    }
    Some((path, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

fn listing(zone: &str) -> Value {
    let entry = |id: &str| {
        json!({
            "id": id,
            "object": "model",
            "kanata": {
                "operations": ["chat"],
                "structured_output": true,
                "sampling_controls": true,
                "reasoning_control": true,
                "function_tools": true,
                "streaming": false,
                "trust_zone": zone,
                "reasoning_efforts": ["none", "low", "medium", "high"],
                "context_tokens": 32768,
            },
        })
    };
    json!({"object": "list", "data": [entry(ALIAS), entry(OTHER)]})
}

fn completion(model: &str, content: &str) -> Value {
    json!({
        "id": "chatcmpl-synthetic",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2},
    })
}

fn tool_completion(name: &str, arguments: Value) -> Value {
    json!({
        "id": "chatcmpl-synthetic-tools",
        "object": "chat.completion",
        "model": ALIAS,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": null, "tool_calls": [{
                "id": "call_synthetic", "type": "function",
                "function": {"name": name, "arguments": arguments}
            }]},
            "finish_reason": "tool_calls",
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2},
    })
}

// ---- Gateway fixtures ----

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("twilight fixture")
}

fn user_json(id: u64, name: &str, bot: bool) -> Value {
    json!({
        "id": id.to_string(), "username": name, "global_name": null,
        "discriminator": "0", "avatar": null, "bot": bot,
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

fn role_json(id: u64) -> Value {
    json!({
        "color": 0,
        "colors": { "primary_color": 0, "secondary_color": null, "tertiary_color": null },
        "hoist": false, "id": id.to_string(), "managed": false, "mentionable": false,
        "name": format!("role-{id}"), "permissions": "0", "position": 1, "flags": 0,
    })
}

fn channel_json(id: u64, kind: u8, parent: Option<u64>) -> Value {
    json!({
        "id": id.to_string(), "type": kind, "name": format!("chan-{id}"),
        "parent_id": parent.map(|id| id.to_string()), "position": 0,
        "permission_overwrites": [],
    })
}

/// A category, a text channel in it and a thread under the channel.
fn guild_create() -> Event {
    let mut guild = json!({
        "afk_channel_id": null, "afk_timeout": 300, "application_id": null, "banner": null,
        "default_message_notifications": 0, "description": null, "discovery_splash": null,
        "emojis": [], "explicit_content_filter": 0, "features": [], "icon": null,
        "id": GUILD.to_string(), "mfa_level": 0, "name": "guild", "nsfw_level": 0,
        "owner_id": OWNER.to_string(), "preferred_locale": "en-US",
        "premium_progress_bar_enabled": false, "premium_tier": 0,
        "public_updates_channel_id": null,
        "roles": [
            role_json(GUILD), role_json(10), role_json(PILOT_ROLE), bot_role(),
            role_json(ROLE_PRIORITY_FIRST), role_json(ROLE_PRIORITY_SECOND), role_json(ROLE_NOT_HELD),
        ],
        "rules_channel_id": null, "splash": null, "system_channel_flags": 0,
        "system_channel_id": null, "verification_level": 0, "vanity_url_code": null,
    });
    for list in [
        "presences",
        "stickers",
        "voice_states",
        "members",
        "guild_scheduled_events",
        "stage_instances",
    ] {
        guild[list] = json!([]);
    }
    guild["channels"] = json!([
        channel_json(CATEGORY, 4, None),
        channel_json(CHANNEL, 0, Some(CATEGORY))
    ]);
    guild["threads"] = json!([{
        "id": THREAD.to_string(), "type": 11, "name": "thread",
        "parent_id": CHANNEL.to_string(), "owner_id": ALICE.to_string(),
        "thread_metadata": {
            "archived": false, "auto_archive_duration": 1440,
            "archive_timestamp": "2026-09-25T12:00:00.000000+00:00", "locked": false,
        },
    }]);
    Event::GuildCreate(Box::new(parse::<GuildCreate>(guild)))
}

/// The bot's managed integration role.
fn bot_role() -> Value {
    let mut role = role_json(BOT_ROLE);
    role["managed"] = json!(true);
    role["tags"] = json!({ "bot_id": SELF.to_string() });
    role
}

/// Alice asks in the thread, mentioning the bot, holding `roles`.
fn question(id: u64, roles: &[u64]) -> Event {
    Event::MessageCreate(Box::new(parse::<MessageCreate>(question_json(id, roles))))
}

fn question_with_text(id: u64, roles: &[u64], text: &str) -> Event {
    let mut message = question_json(id, roles);
    message["content"] = json!(format!("<@{SELF}> {text}"));
    Event::MessageCreate(Box::new(parse::<MessageCreate>(message)))
}

/// Alice asks through `@Kanade` resolved to the bot's managed role.
fn question_by_role(id: u64, roles: &[u64]) -> Event {
    let mut message = question_json(id, roles);
    message["content"] = json!(format!("<@&{BOT_ROLE}> when is lotus?"));
    message["mentions"] = json!([]);
    message["mention_roles"] = json!([BOT_ROLE.to_string()]);
    Event::MessageCreate(Box::new(parse::<MessageCreate>(message)))
}

fn question_json(id: u64, roles: &[u64]) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": THREAD.to_string(),
        "guild_id": GUILD.to_string(),
        "author": user_json(ALICE, "alice", false),
        "member": {
            "roles": roles.iter().map(u64::to_string).collect::<Vec<_>>(),
            "joined_at": "2026-01-01T00:00:00.000000+00:00",
            "deaf": false, "mute": false, "flags": 0,
        },
        "content": format!("<@{SELF}> when is lotus?"),
        "timestamp": "2026-09-25T12:00:00.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [{
            "id": SELF.to_string(), "username": "kanade", "discriminator": "0",
            "avatar": null, "bot": true, "public_flags": 0,
        }],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
    })
}

// ---- Harness ----

struct Script(mpsc::UnboundedReceiver<Event>, bool);

impl EventSource for Script {
    async fn next_event(&mut self) -> Option<Result<Event, GatewayError>> {
        if self.1 {
            return None;
        }
        Some(Ok(self.0.recv().await?))
    }

    fn close(&mut self) {
        self.1 = true;
    }
}

struct Live {
    _temp: Temp,
    fake: Arc<FakeDiscord>,
    store: Arc<SqliteStore>,
    events: mpsc::UnboundedSender<Event>,
    health: Arc<dyn HealthProbe>,
    composition: Composition,
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

async fn live(stub: &ModelStub, extra: &[(&str, &str)]) -> (Live, Discord) {
    live_with_role_profiles(stub, extra, &[], None, None).await
}

async fn live_with_role_profiles(
    stub: &ModelStub,
    extra: &[(&str, &str)],
    profile_ids: &[&str],
    assignments: Option<Value>,
    saved_profile: Option<&str>,
) -> (Live, Discord) {
    let temp = Temp::new();
    if !profile_ids.is_empty() {
        let profiles_dir = temp.0.join("Personas/profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        let example = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas/profiles/example.yaml"),
        )
        .unwrap();
        for id in profile_ids {
            let profile = example
                .replacen("id: example", &format!("id: {id}"), 1)
                .replacen("label: Example", &format!("label: {id}"), 1);
            fs::write(profiles_dir.join(format!("{id}.yaml")), profile).unwrap();
        }
    }
    let url = stub.url();
    let mut values = vec![
        ("KANADE_DISCORD_GATEWAY", "1"),
        ("KANADE_EXPECT_V4_STOPPED", "1"),
        ("KANADE_CHAT_ENABLED", "1"),
        ("KANADE_CHAT_CATEGORY_IDS", "40"),
        ("KANADE_CHAT_PILOT_ROLE_ID", "30"),
        ("KANADE_MODEL_BASE_URL", url.as_str()),
        ("KANADE_CHAT_MODEL", ALIAS),
    ];
    values.extend_from_slice(extra);
    let config = temp.config(&values);
    let store = store::open(&config.store).await.unwrap();
    let mut rows = Vec::new();
    if let Some(assignments) = assignments {
        rows.push((keys::ROLE_PROFILES.to_owned(), assignments.to_string()));
    }
    if let Some(profile) = saved_profile {
        rows.push((keys::PROFILE_VISIBILITY.to_owned(), profile.to_owned()));
    }
    if !rows.is_empty() {
        store.put_settings_rows(rows).await.unwrap();
    }
    if let Some(profile) = saved_profile {
        store
            .put_member(MemberProfile {
                member: Member {
                    user_id: ALICE.to_string(),
                    display_name: Some("Synthetic Alice".into()),
                    ..Member::default()
                },
                reply_style: Some(profile.to_owned()),
                ..MemberProfile::default()
            })
            .await
            .unwrap();
    }
    let prepared = discord::prepare(&config, Duration::from_millis(50));
    let health = LiveHealth::new(store.clone())
        .with_discord(prepared.probe.clone(), prepared.tick_status.clone());
    let mut composition = api::compose(&config, store.clone(), prepared.cache.clone(), health)
        .await
        .unwrap();
    let models = composition.models.clone().expect("model stack");
    eventually!("the model listing", models.catalog().listed);
    let fake = Arc::new(FakeDiscord::new());
    let (events, receiver) = mpsc::unbounded_channel();
    let wiring = Wiring {
        source: Script(receiver, false),
        transport: Arc::clone(&fake),
        clock: Arc::new(auth::system_now),
        tick: Duration::from_millis(50),
        extraction: extract::Timing::default(),
    };
    let discord = discord::start(&config, store.clone(), &mut composition, prepared, wiring)
        .await
        .unwrap();
    let live = Live {
        _temp: temp,
        fake,
        store,
        events,
        health: Arc::clone(&composition.admin.health),
        composition,
    };
    (live, discord)
}

/// Run the Discord side while `script` runs, stop it, then close the store.
async fn drive(live: Live, mut discord: Discord, script: impl AsyncFnOnce(&Live)) {
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::join!(
        discord.until(async {
            let _ = stopped.await;
        }),
        async {
            script(&live).await;
            let _ = stop.send(());
        }
    );
    let Live {
        _temp,
        store,
        events,
        health,
        composition,
        ..
    } = live;
    drop((events, health, composition, discord));
    store::close(store, Duration::ZERO).await;
}

/// Ready, then the guild with the chat category, its channel and thread.
fn connect(live: &Live) {
    live.events.send(ready()).unwrap();
    live.events.send(guild_create()).unwrap();
}

/// What members see in `channel`: each delivered message's latest text (the
/// staging placeholder is edited into the answer) and what it replies to.
fn replies(fake: &FakeDiscord, channel: u64) -> Vec<(String, Option<u64>)> {
    let mut shown: Vec<(u64, String, Option<u64>)> = Vec::new();
    for call in fake.calls() {
        match call {
            Call::Create {
                channel: to,
                message,
                outcome: Outcome::Delivered(id),
            } if to.get() == channel => shown.push((
                id.get(),
                message.content.unwrap_or_default(),
                message.reply_to.map(|id| id.get()),
            )),
            Call::Edit {
                message,
                edit,
                outcome: Outcome::Delivered(()),
                ..
            } => {
                if let Some(entry) = shown.iter_mut().find(|entry| entry.0 == message.get()) {
                    entry.1 = edit.content.unwrap_or_default();
                }
            }
            _ => {}
        }
    }
    shown
        .into_iter()
        .map(|(_, text, reply_to)| (text, reply_to))
        .collect()
}

/// The persona's staging line is still showing: the answer is not in yet.
fn answered(fake: &FakeDiscord, channel: u64, count: usize) -> bool {
    let shown = replies(fake, channel);
    shown.len() == count && fake.count(Op::Edit) >= count
}

async fn chats(store: &SqliteStore) -> Vec<ChatInteraction> {
    store
        .list_chats(&ChatFilter {
            limit: 50,
            ..ChatFilter::default()
        })
        .await
        .unwrap()
        .items
}

async fn one_chat(store: &SqliteStore) -> ChatInteraction {
    eventually!("the chat-log row", chats(store).await.len() == 1);
    chats(store).await.remove(0)
}

// ---- Tests ----

#[tokio::test]
async fn a_pilot_member_in_a_chat_category_thread_is_answered_as_a_reply_and_logged() {
    let stub = ModelStub::start("local", "Lotus is at nine tonight.").await;
    let (live, discord) = live(&stub, &[]).await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        live.events.send(question(5001, &[PILOT_ROLE])).unwrap();
        eventually!("the reply", answered(&live.fake, THREAD, 1));
        let posted = replies(&live.fake, THREAD);
        assert_eq!(posted.len(), 1);
        assert!(
            posted[0].0.contains("Lotus is at nine"),
            "{:?}",
            posted[0].0
        );
        assert_eq!(posted[0].1, Some(5001), "a reply to the question");
        let row = one_chat(&live.store).await;
        assert_eq!(row.outcome, ChatOutcome::Answered);
        assert_eq!(row.message_id.as_deref(), Some("5001"));
        assert_eq!(
            row.channel_id.as_deref(),
            Some("50"),
            "a thread keys on its parent"
        );
        assert_eq!(
            row.guardrail,
            json!({
                "context": {"window": 32768, "reserve": 1024, "source": "catalog"},
                "delivery": {"placeholder": "edited", "parts": 1, "delivered": 1},
            })
        );
        assert_eq!(stub.completions(), 1);
        // The persona it answered as, and the round as sent.
        assert_eq!(row.persona.as_deref(), Some("kanade"));
        assert_eq!(row.profile, None);
        assert_eq!(row.profile_source.as_deref(), Some("default"));
        assert_eq!(row.rounds[0].model, ALIAS);
        assert_eq!(row.rounds[0].route.as_deref(), Some("homelab"));
        // A member without the pilot role is ignored.
        live.events.send(question(5002, &[])).unwrap();
        sleep(Duration::from_millis(200)).await;
        assert_eq!(chats(&live.store).await.len(), 1);
        assert_eq!(stub.completions(), 1);
        let limits = live.composition.admin.state.chat.as_ref().unwrap().limits();
        assert_eq!(limits.unwrap().allowance.pool.used, 1);
        // `@Kanade` resolved to the bot's managed role summons it too.
        live.events
            .send(question_by_role(5003, &[PILOT_ROLE]))
            .unwrap();
        eventually!("the role-mention reply", answered(&live.fake, THREAD, 2));
        assert_eq!(replies(&live.fake, THREAD)[1].1, Some(5003));
    })
    .await;
}

#[tokio::test]
async fn live_chat_uses_the_shared_checked_in_strategy_guides() {
    let stub = ModelStub::scripted(
        "local",
        "The checked-in guide is ready.",
        vec![
            tool_completion(
                "get_boss_strategy",
                json!({"boss": "MaleficStar", "difficulty": "Hard"}),
            ),
            completion(ALIAS, "The checked-in guide is ready."),
            tool_completion("get_boss_strategy", json!({"boss": "NotABoss"})),
            completion(ALIAS, "That boss is not in the guild catalog."),
        ],
    )
    .await;
    let knowledge = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("boss/knowledge")
        .display()
        .to_string();
    let (live, discord) = live(&stub, &[("KANADE_KNOWLEDGE_DIR", &knowledge)]).await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        live.events
            .send(question_with_text(
                5201,
                &[PILOT_ROLE],
                "strategy for Hard MaleficStar?",
            ))
            .unwrap();
        eventually!("the strategy reply", answered(&live.fake, THREAD, 1));
        let (tools, bundles) = stub.first_surface();
        assert!(tools.iter().any(|tool| tool == "get_boss_strategy"));
        assert_eq!(
            bundles,
            json!(["strategy", "run_changes", "weekly_changes"])
        );
        live.events
            .send(question_with_text(
                5202,
                &[PILOT_ROLE],
                "what is the strategy for NotABoss?",
            ))
            .unwrap();
        eventually!("the unknown-boss reply", answered(&live.fake, THREAD, 2));
        eventually!("two chat rows", chats(&live.store).await.len() == 2);
        let rows = chats(&live.store).await;
        let known = rows
            .iter()
            .find(|row| row.message_id.as_deref() == Some("5201"))
            .unwrap();
        assert_eq!(known.rounds[0].tools, ["get_boss_strategy"]);
        let guide = known.rounds[0].tool_calls[0]["result"].as_str().unwrap();
        assert!(
            guide.starts_with("# Radiant Malefic Star (MaleficStar)"),
            "{guide}"
        );
        assert!(guide.contains("### Hard"), "{guide}");
        assert!(!guide.contains("## Sources"), "{guide}");
        let unknown = rows
            .iter()
            .find(|row| row.message_id.as_deref() == Some("5202"))
            .unwrap();
        let error = unknown.rounds[0].tool_calls[0]["result"].as_str().unwrap();
        assert!(error.contains("NotABoss"), "{error}");
    })
    .await;
}

#[test]
fn a_guide_edited_after_startup_is_unreadable_not_missing() {
    let temp = Temp::new();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/knowledge");
    let dir = temp.0.join("knowledge");
    fs::create_dir(&dir).unwrap();
    for entry in fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let catalog =
        load_catalog(&Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml")).unwrap();
    let knowledge = load_knowledge_dir(&dir).unwrap();
    let guides = LiveStrategyGuides {
        knowledge: &knowledge,
        catalog: &catalog,
    };
    let reference = |short: &str| BossReference {
        short: short.into(),
        difficulty: None,
    };
    assert!(guides.render(&reference("Seren")).is_ok());
    // Broken YAML, then valid YAML that lost required parts.
    fs::write(dir.join("seren.yaml"), "summary: [\n").unwrap();
    fs::write(dir.join("maleficstar.yaml"), "boss: MaleficStar\n").unwrap();
    assert_eq!(
        guides.render(&reference("Seren")),
        Err(GuideError::Unreadable)
    );
    assert_eq!(
        guides.render(&reference("MaleficStar")),
        Err(GuideError::Unreadable)
    );
    assert_eq!(
        guides.render(&reference("NotABoss")),
        Err(GuideError::Missing)
    );
}

#[test]
fn live_guides_offer_only_non_catalog_event_documents() {
    let temp = Temp::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = temp.0.join("knowledge");
    fs::create_dir(&dir).unwrap();
    fs::copy(
        root.join("boss/knowledge/schema.json"),
        dir.join("schema.json"),
    )
    .unwrap();
    fs::write(
        dir.join("_meta.yaml"),
        "schema_version: 2\nresearched_as_of: '2031-04-05'\n",
    )
    .unwrap();
    // Invented documents: one event outside the catalog, one under a catalog key.
    let doc = |key: &str| {
        format!(
            "boss: {key}\nevent:\n  name: Invented Season\n  availability: Invented World only.\n  \
             aliases: [Zeph, 제피]\nsummary: Invented.\ncore: [a]\ndanger: [b]\ntips: [c]\n\
             sources:\n- {{url: 'https://example.invalid/', title: t, author: a, kind: guide, \
             fetched: '2031-04-01'}}\n"
        )
    };
    fs::write(dir.join("zephyrine.yaml"), doc("Zephyrine")).unwrap();
    fs::write(dir.join("lotus.yaml"), doc("Lotus")).unwrap();
    let catalog = load_catalog(&root.join("boss/bosses.yaml")).unwrap();
    let knowledge = load_knowledge_dir(&dir).unwrap();
    assert_eq!(knowledge.events.len(), 2);
    let guides = LiveStrategyGuides {
        knowledge: &knowledge,
        catalog: &catalog,
    };
    let events = guides.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, "Zephyrine");
    assert_eq!(events[0].aliases, ["Zeph", "제피"]);
    let event = guides
        .render(&BossReference {
            short: "Zephyrine".into(),
            difficulty: None,
        })
        .unwrap();
    assert!(
        event.starts_with("# Zephyrine\nAvailability: Invented World only.\n"),
        "{event}"
    );
    let lotus = guides
        .render(&BossReference {
            short: "Lotus".into(),
            difficulty: None,
        })
        .unwrap();
    assert!(lotus.starts_with("# Lotus (Lotus)\n_Researched"), "{lotus}");
}

#[tokio::test]
async fn without_a_knowledge_dir_live_chat_hides_the_strategy_bundle() {
    let stub = ModelStub::start("local", "I have no guides here.").await;
    let (live, discord) = live(&stub, &[]).await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        live.events
            .send(question_with_text(
                5301,
                &[PILOT_ROLE],
                "strategy for Hard MaleficStar?",
            ))
            .unwrap();
        eventually!("the reply", answered(&live.fake, THREAD, 1));
        let (tools, bundles) = stub.first_surface();
        assert!(!tools.iter().any(|tool| tool == "get_boss_strategy"));
        assert_eq!(bundles, json!(["run_changes", "weekly_changes"]));
    })
    .await;
}

#[tokio::test]
async fn chat_resolves_live_discord_roles_before_saved_style_without_granting_access() {
    let stub = ModelStub::start("local", "Lotus is at nine tonight.").await;
    let (live, discord) = live_with_role_profiles(
        &stub,
        &[],
        &["role-private", "other", "saved"],
        Some(json!([
            {"role_id": ROLE_NOT_HELD.to_string(), "profile": "missing"},
            {"role_id": ROLE_PRIORITY_SECOND.to_string(), "profile": "role-private"},
            {"role_id": ROLE_PRIORITY_FIRST.to_string(), "profile": "other"}
        ])),
        Some("saved"),
    )
    .await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        // Discord order is role 41 then 42; saved assignment priority is 42 then 41.
        live.events
            .send(question(
                5101,
                &[PILOT_ROLE, ROLE_PRIORITY_FIRST, ROLE_PRIORITY_SECOND],
            ))
            .unwrap();
        eventually!("role-profile reply", answered(&live.fake, THREAD, 1));
        let row = one_chat(&live.store).await;
        assert_eq!(row.profile.as_deref(), Some("role-private"));
        assert_eq!(row.profile_source.as_deref(), Some("role"));
        assert_eq!(stub.completions(), 1);

        // The role assignment is not an access grant: the pilot role is absent.
        live.events
            .send(question(5102, &[ROLE_PRIORITY_SECOND]))
            .unwrap();
        sleep(Duration::from_millis(200)).await;
        assert_eq!(chats(&live.store).await.len(), 1);
        assert_eq!(stub.completions(), 1);
    })
    .await;
}

#[tokio::test]
async fn a_saved_model_switch_reaches_the_next_question_without_a_restart() {
    use crate::api::admin::config::ModelCatalog;
    use crate::domain::settings::Reasoning;

    let stub = ModelStub::start("local", "Lotus is at nine tonight.").await;
    let (live, discord) = live(&stub, &[("KANADE_CHAT_REASONING", "low")]).await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        live.events.send(question(5001, &[PILOT_ROLE])).unwrap();
        eventually!("the first reply", answered(&live.fake, THREAD, 1));
        assert_eq!(stub.sent(), [(ALIAS.to_owned(), Some("low".to_owned()))]);

        // What a config PATCH applies after saving: the reasoning first.
        let stack = live.composition.models.clone().unwrap();
        let mut models = live.composition.settings.models.clone();
        models.chat.reasoning = Reasoning::High;
        stack.apply(&models).unwrap();
        live.events.send(question(5002, &[PILOT_ROLE])).unwrap();
        eventually!("the second reply", answered(&live.fake, THREAD, 2));
        assert_eq!(stub.sent()[1], (ALIAS.to_owned(), Some("high".to_owned())));

        // Then another alias with its own level.
        models.chat.alias = Some(OTHER.to_owned());
        models.chat.reasoning = Reasoning::Medium;
        stack.apply(&models).unwrap();
        live.events.send(question(5003, &[PILOT_ROLE])).unwrap();
        eventually!("the third reply", answered(&live.fake, THREAD, 3));
        assert_eq!(
            stub.sent()[2],
            (OTHER.to_owned(), Some("medium".to_owned()))
        );
        // Each chat row's round records what that question actually sent.
        eventually!("three chat rows", chats(&live.store).await.len() == 3);
        let mut rows = chats(&live.store).await;
        rows.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        let rounds: Vec<(String, Option<String>)> = rows
            .iter()
            .map(|row| (row.rounds[0].model.clone(), row.rounds[0].reasoning.clone()))
            .collect();
        assert_eq!(
            rounds,
            [
                (ALIAS.to_owned(), Some("low".to_owned())),
                (ALIAS.to_owned(), Some("high".to_owned())),
                (OTHER.to_owned(), Some("medium".to_owned())),
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn an_external_chat_route_sends_raw_member_data_without_opt_in_or_a_model_view() {
    use crate::domain::members::{Member, MemberProfile, MemberStore};
    let stub = ModelStub::start("external", "Lotus is at nine tonight.").await;
    let (live, discord) = live(&stub, &[]).await;
    live.store
        .put_member(MemberProfile {
            member: Member {
                user_id: ALICE.to_string(),
                display_name: Some("Synthetic Alicia Quartz".into()),
                nickname: Some("SyntheticQuartz".into()),
                has_role: true,
                ..Member::default()
            },
            aliases: vec!["syntheticalias".into()],
            reply_style: None,
            roles: Vec::new(),
            is_guild_admin: false,
        })
        .await
        .unwrap();
    drive(live, discord, async |live| {
        connect(live);
        let mut asked = question_json(5001, &[PILOT_ROLE]);
        let raw = "Synthetic Alicia Quartz SyntheticQuartz syntheticalias SyntheticMember \
            999000111222333444 https://synthetic.invalid/member?id=fixture";
        asked["content"] = json!(format!("<@{SELF}> {raw}"));
        live.events
            .send(Event::MessageCreate(Box::new(parse::<MessageCreate>(
                asked,
            ))))
            .unwrap();
        eventually!("the reply", answered(&live.fake, THREAD, 1));
        assert!(replies(&live.fake, THREAD)[0].0.contains("Lotus"));
        let row = one_chat(&live.store).await;
        assert_eq!(row.outcome, ChatOutcome::Answered);
        assert_eq!(row.rounds[0].route.as_deref(), Some("external_unmasked"));
        assert_eq!(
            row.guardrail,
            json!({
                "context": {"window": 32768, "reserve": 1024, "source": "catalog"},
                "delivery": {"placeholder": "edited", "parts": 1, "delivered": 1},
                "external_unmasked": true,
            })
        );
        let sent = serde_json::to_string(&*stub.completions.lock().unwrap()).unwrap();
        for part in [
            "Synthetic Alicia Quartz",
            "SyntheticQuartz",
            "syntheticalias",
            "SyntheticMember",
            "999000111222333444",
            "https://synthetic.invalid/member?id=fixture",
        ] {
            assert!(
                sent.contains(part),
                "{part} did not reach the provider: {sent}"
            );
        }
        assert!(
            live.store
                .load_masked_chat(&row.id)
                .await
                .unwrap()
                .is_none(),
            "new unmasked turns do not create a historical Model view"
        );
    })
    .await;
}

fn alice(nickname: &str, aliases: &[&str]) -> crate::domain::members::MemberProfile {
    use crate::domain::members::{Member, MemberProfile};
    MemberProfile {
        member: Member {
            user_id: ALICE.to_string(),
            display_name: Some("Alicia Quartz".into()),
            nickname: Some(nickname.into()),
            has_role: true,
            ..Member::default()
        },
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        reply_style: None,
        roles: Vec::new(),
        is_guild_admin: false,
    }
}

/// The request is built from locally rendered history, so an external route
/// receives its original text even after the current member profile changes.
#[tokio::test]
async fn renamed_member_names_in_chat_history_are_sent_unchanged() {
    use crate::domain::members::MemberStore;
    let stub = ModelStub::start("external", "Sure Oldnick, Lotus is at nine.").await;
    let (live, discord) = live(&stub, &[]).await;
    live.store
        .put_member(alice("Oldnick", &["zorblax"]))
        .await
        .unwrap();
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        let mut first = question_json(5001, &[PILOT_ROLE]);
        first["content"] = json!(format!("<@{SELF}> zorblax here, when is lotus?"));
        live.events
            .send(Event::MessageCreate(Box::new(parse::<MessageCreate>(
                first,
            ))))
            .unwrap();
        eventually!("the first reply", answered(&live.fake, THREAD, 1));
        // Renamed and the alias removed before the next question.
        live.store.put_member(alice("Newnick", &[])).await.unwrap();
        live.events.send(question(5002, &[PILOT_ROLE])).unwrap();
        eventually!("the second reply", answered(&live.fake, THREAD, 2));
        let bodies = stub.completions.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2);
        let second = bodies[1].to_string();
        assert!(second.contains("lotus"), "history went out: {second}");
        for raw in ["Oldnick", "zorblax"] {
            assert!(second.contains(raw), "{raw} was changed: {second}");
        }
    })
    .await;
}

/// The next question's request body (the last completion so far).
fn last_body(stub: &ModelStub) -> String {
    stub.completions
        .lock()
        .unwrap()
        .last()
        .expect("a completion")
        .to_string()
}

#[tokio::test]
async fn a_deflected_question_and_its_line_never_reach_the_next_prompt() {
    let line = crate::domain::settings::DEFAULT_DEFLECTION_LINE;
    let stub = ModelStub::start("local", "Lotus is at nine tonight.").await;
    let (live, discord) = live(&stub, &[]).await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("chat idle", live.health.health().await.chat == Some("idle"));
        live.events
            .send(question_with_text(
                5001,
                &[PILOT_ROLE],
                "this fucking bot, when is kalos?",
            ))
            .unwrap();
        eventually!("the line", answered(&live.fake, THREAD, 1));
        assert_eq!(replies(&live.fake, THREAD)[0].0, line);
        assert_eq!(stub.completions(), 0, "no model call");
        live.events.send(question(5002, &[PILOT_ROLE])).unwrap();
        eventually!("the second reply", answered(&live.fake, THREAD, 2));
        assert_eq!(stub.completions(), 1);
        let body = last_body(&stub);
        assert!(body.contains("when is lotus"), "{body}");
        for kept_out in ["fucking", "when is kalos", line] {
            assert!(!body.contains(kept_out), "{kept_out} leaked: {body}");
        }
        let rows = chats(&live.store).await;
        let rude = rows
            .iter()
            .find(|row| row.message_id.as_deref() == Some("5001"))
            .unwrap();
        assert_eq!(rude.outcome, ChatOutcome::Profanity);
        assert!(rude.question.contains("fucking"), "logged in full");
    })
    .await;
}

#[tokio::test]
async fn a_replaced_reply_stays_out_and_a_recovered_one_stays_in() {
    let line = crate::domain::settings::DEFAULT_DEFLECTION_LINE;
    for (retry, kept) in [("Shit, nine.", false), ("Kalos is at nine.", true)] {
        let stub = ModelStub::scripted(
            "local",
            "Lotus is at nine tonight.",
            vec![
                completion(ALIAS, "Kalos is at nine, shit."),
                completion(ALIAS, retry),
            ],
        )
        .await;
        let (live, discord) = live(&stub, &[]).await;
        drive(live, discord, async |live| {
            connect(live);
            eventually!("chat idle", live.health.health().await.chat == Some("idle"));
            live.events
                .send(question_with_text(5001, &[PILOT_ROLE], "when is kalos?"))
                .unwrap();
            eventually!("the first reply", answered(&live.fake, THREAD, 1));
            let shown = &replies(&live.fake, THREAD)[0].0;
            assert_eq!(shown == line, !kept, "{shown}");
            live.events.send(question(5002, &[PILOT_ROLE])).unwrap();
            eventually!("the second reply", answered(&live.fake, THREAD, 2));
            assert_eq!(stub.completions(), 3, "the clean retry ran");
            let body = last_body(&stub);
            assert!(!body.contains("shit") && !body.contains(line), "{body}");
            assert_eq!(body.contains("when is kalos"), kept, "{body}");
            assert_eq!(body.contains("Kalos is at nine."), kept, "{body}");
        })
        .await;
    }
}

#[tokio::test]
async fn chat_off_ignores_questions_and_health_says_disabled() {
    let stub = ModelStub::start("local", "x").await;
    let (live, discord) = live(&stub, &[("KANADE_CHAT_ENABLED", "0")]).await;
    drive(live, discord, async |live| {
        connect(live);
        eventually!("the guild", !live.fake.calls().is_empty());
        assert_eq!(live.health.health().await.chat, Some("disabled"));
        live.events.send(question(5001, &[PILOT_ROLE])).unwrap();
        sleep(Duration::from_millis(200)).await;
        assert!(replies(&live.fake, THREAD).is_empty());
        assert!(chats(&live.store).await.is_empty());
        assert_eq!(stub.completions(), 0);
    })
    .await;
}
