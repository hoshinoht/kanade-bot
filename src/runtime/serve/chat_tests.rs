//! Live chat in serve: a scripted gateway, the fake transport, a loopback
//! model gateway and a temp store. Nothing touches the network.

use std::{
    net::SocketAddr,
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
        transport::{Call, FakeDiscord, Outcome},
    },
    chat::sanitize::FAILURE_REPLY,
    domain::model_log::{ChatFilter, ChatInteraction, ChatOutcome, ModelLogStore},
    infrastructure::store::SqliteStore,
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
const ALIAS: &str = "home-chat";

// ---- Loopback model gateway ----

struct ModelStub {
    addr: SocketAddr,
    completions: Arc<Mutex<Vec<Value>>>,
}

impl ModelStub {
    /// Lists `ALIAS` in `zone`; every completion answers `reply`.
    async fn start(zone: &'static str, reply: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let completions = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&completions);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let Some((path, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let answer = if path.ends_with("/models") {
                        listing(zone)
                    } else {
                        seen.lock().unwrap().push(body);
                        completion(reply)
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
    json!({"object": "list", "data": [{
        "id": ALIAS,
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
    }]})
}

fn completion(content: &str) -> Value {
    json!({
        "id": "chatcmpl-synthetic",
        "object": "chat.completion",
        "model": ALIAS,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
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
        "roles": [role_json(GUILD), role_json(10), role_json(PILOT_ROLE), bot_role()],
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
    let temp = Temp::new();
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

fn replies(fake: &FakeDiscord, channel: u64) -> Vec<(String, Option<u64>)> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Create {
                channel: to,
                message,
                outcome: Outcome::Delivered(_),
            } if to.get() == channel => Some((
                message.content.unwrap_or_default(),
                message.reply_to.map(|id| id.get()),
            )),
            _ => None,
        })
        .collect()
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
        eventually!("the reply", !replies(&live.fake, THREAD).is_empty());
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
        assert_eq!(row.guardrail, json!({}));
        assert_eq!(stub.completions(), 1);
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
        eventually!(
            "the role-mention reply",
            replies(&live.fake, THREAD).len() == 2
        );
        assert_eq!(replies(&live.fake, THREAD)[1].1, Some(5003));
    })
    .await;
}

#[tokio::test]
async fn an_external_chat_route_is_refused_with_the_failure_line() {
    let stub = ModelStub::start("external", "should never be asked").await;
    let (live, discord) = live(&stub, &[]).await;
    drive(live, discord, async |live| {
        connect(live);
        live.events.send(question(5001, &[PILOT_ROLE])).unwrap();
        eventually!("the reply", !replies(&live.fake, THREAD).is_empty());
        assert_eq!(replies(&live.fake, THREAD)[0].0, FAILURE_REPLY);
        let row = one_chat(&live.store).await;
        assert_eq!(row.outcome, ChatOutcome::Error);
        assert!(
            row.error
                .as_deref()
                .is_some_and(|error| error.contains("pseudonymization")),
            "{:?}",
            row.error
        );
        assert_eq!(stub.completions(), 0, "nothing left the homelab");
        let limits = live.composition.admin.state.chat.as_ref().unwrap().limits();
        assert_eq!(limits.unwrap().allowance.pool.used, 0, "refunded");
    })
    .await;
}

#[tokio::test]
async fn the_unmasked_override_answers_and_marks_the_guardrail() {
    let stub = ModelStub::start("external", "Lotus is at nine tonight.").await;
    let (live, discord) = live(&stub, &[("KANADE_ALLOW_EXTERNAL_UNMASKED", "1")]).await;
    drive(live, discord, async |live| {
        connect(live);
        live.events.send(question(5001, &[PILOT_ROLE])).unwrap();
        eventually!("the reply", !replies(&live.fake, THREAD).is_empty());
        assert!(replies(&live.fake, THREAD)[0].0.contains("Lotus"));
        let row = one_chat(&live.store).await;
        assert_eq!(row.outcome, ChatOutcome::Answered);
        assert_eq!(row.guardrail, json!({"external_unmasked": true}));
        assert_eq!(stub.completions(), 1);
    })
    .await;
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
