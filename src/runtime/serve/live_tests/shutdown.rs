//! V03: serve shuts down with every shutdown-relevant activity in flight at
//! once (a chat answer and an extraction call hanging on the model, a tick
//! inside a Discord send, a slash command inside its reply, an open admin
//! event stream and an admin request inside a CDN fetch), in order and
//! inside Compose's stop grace. Real time: the run spans real sockets, SQLite
//! threads and reqwest, which paused time would race.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{Json, Router, routing::get, routing::post};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use twilight_model::gateway::payload::incoming::MessageCreate;

use super::*;
use crate::{
    api::{
        avatars::{AvatarCache, AvatarFetch, FetchFuture},
        server,
    },
    bot::{
        extract_feed::snowflake_before,
        transport::{Hold, TransportConfig},
    },
    domain::model_log::{ChatFilter, ExtractionFilter, ExtractionOutcome, ModelLogStore},
    extract::pipeline::CALL_CANCELLED,
    runtime::{error::Error, logging},
};

/// deploy/compose.yaml `stop_grace_period`: SIGKILL follows SIGTERM after it.
const COMPOSE_STOP_GRACE: Duration = Duration::from_secs(30);
/// How long held calls keep going after shutdown is triggered: the tick's
/// send is one slow attempt (inside `TransportConfig::attempt_timeout`,
/// 10 s); the admin avatar fetch (CDN timeout 20 s) outlasts the Discord stop
/// so it is still in flight when the HTTP drain begins.
const TICK_SEND_TAIL: Duration = Duration::from_secs(8);
const AVATAR_FETCH_TAIL: Duration = Duration::from_secs(10);
/// Longer than any shutdown grace: only a cut ends these calls.
const MODEL_HANG: Duration = Duration::from_secs(120);

const ALIAS: &str = "home-model";
const TOKEN: &str = "shutdown-break-glass-token-0123456789abcdef";
const HOST: &str = "kanade.test";
const ORIGIN: &str = "https://kanade.test";
const CHAT_CATEGORY: u64 = 410;
const CHAT_CHANNEL: u64 = 411;
const TIMING: extract::Timing = extract::Timing {
    debounce: Duration::from_millis(150),
    drain_interval: Duration::from_millis(40),
};

// ---- Timeline ----

/// Every captured log line in emission order (exact), each stamped when the
/// watcher first saw it: a few ms late at most, as it polls between the other
/// tasks' steps. The test's own points are log lines too (`v03_*`).
#[derive(Clone, Default)]
struct Timeline(Arc<Mutex<Vec<(String, Instant)>>>);

impl Timeline {
    fn scan(&self) {
        let lines = logging::captured();
        let mut seen = self.0.lock().unwrap();
        let now = Instant::now();
        for line in &lines[seen.len()..] {
            seen.push((line["event"].as_str().unwrap_or_default().to_owned(), now));
        }
    }

    /// Stamps lines until dropped.
    async fn watch(self) -> Infallible {
        loop {
            self.scan();
            sleep(Duration::from_millis(2)).await;
        }
    }

    /// From the trigger on: `(position, event, stamp)`.
    fn after_trigger(&self) -> Vec<(usize, String, Instant)> {
        let seen = self.0.lock().unwrap();
        let start = seen
            .iter()
            .position(|(event, _)| event == "v03_trigger")
            .unwrap_or(seen.len());
        seen.iter()
            .enumerate()
            .skip(start)
            .map(|(at, (event, stamp))| (at, event.clone(), *stamp))
            .collect()
    }

    /// The first `event` from the trigger on.
    fn first(&self, event: &str) -> (usize, Instant) {
        self.after_trigger()
            .into_iter()
            .find(|(_, name, _)| name == event)
            .map(|(at, _, stamp)| (at, stamp))
            .unwrap_or_else(|| panic!("no `{event}` after the trigger\n{}", self.breakdown()))
    }

    /// `+s.mmm event` per line from the trigger on.
    fn breakdown(&self) -> String {
        let lines = self.after_trigger();
        let Some((_, _, trigger)) = lines.first().cloned() else {
            return "(not triggered)".into();
        };
        lines
            .iter()
            .map(|(_, event, stamp)| {
                format!("  +{:>6.3}s {event}", (*stamp - trigger).as_secs_f64())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A point of the test's own, in the log sequence.
fn mark(event: &'static str) {
    logging::event("INFO", event, json!({}));
}

fn logged(event: &str) -> Vec<Value> {
    logging::captured()
        .into_iter()
        .filter(|line| line["event"] == event)
        .collect()
}

// ---- The model gateway: every completion hangs ----

#[derive(Clone)]
struct HangingModel {
    url: String,
    chat: Arc<AtomicUsize>,
    extract: Arc<AtomicUsize>,
}

impl HangingModel {
    async fn start() -> Self {
        let kanata = json!({
            "operations": ["chat"], "structured_output": true,
            "sampling_controls": true, "reasoning_control": false,
            "function_tools": true, "streaming": false,
            "trust_zone": "local", "context_tokens": 32768,
        });
        let listing = json!({"object": "list", "data": [{
            "id": ALIAS, "object": "model", "kanata": kanata,
        }]});
        let chat = Arc::new(AtomicUsize::new(0));
        let extract = Arc::new(AtomicUsize::new(0));
        let (chats, extracts) = (chat.clone(), extract.clone());
        let app = Router::new()
            .route("/v1/models", get(move || async move { Json(listing) }))
            .route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| async move {
                    // Chat offers tools; extraction asks for structured output.
                    if body.get("tools").is_some() {
                        chats.fetch_add(1, Ordering::SeqCst);
                    } else {
                        extracts.fetch_add(1, Ordering::SeqCst);
                    }
                    sleep(MODEL_HANG).await;
                    Json(json!({}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            url: format!("http://{addr}/v1"),
            chat,
            extract,
        }
    }

    fn calls(&self) -> (usize, usize) {
        (
            self.chat.load(Ordering::SeqCst),
            self.extract.load(Ordering::SeqCst),
        )
    }
}

// ---- The Discord CDN: a fetch waits for its release ----

#[derive(Clone, Default)]
struct SlowCdn {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl AvatarFetch for SlowCdn {
    fn fetch<'a>(&'a self, _: &'a str) -> FetchFuture<'a> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Err("timeout")
        })
    }
}

// ---- Fixtures ----

fn harness(model: &HangingModel) -> Harness {
    let mut harness = Harness::with(&[
        ("KANADE_MODEL_BASE_URL", model.url.as_str()),
        ("KANADE_CHAT_MODEL", ALIAS),
        ("KANADE_EXTRACT_MODEL", ALIAS),
        // Both calls hang at once rather than one waiting for a permit.
        ("KANADE_MODEL_PERMITS", "4"),
        ("KANADE_CHAT_ENABLED", "1"),
        ("KANADE_CHAT_CATEGORY_IDS", "410"),
        ("KANADE_CHAT_PILOT_ROLE_ID", "10"),
        ("KANADE_WATCH_CHANNEL_IDS", "301"),
        ("KANADE_EXTRACTION_ENABLED", "1"),
        ("KANADE_ADMIN_HOST", HOST),
    ]);
    let token = harness._temp.0.join("admin_token");
    std::fs::write(&token, format!("{TOKEN}\n")).unwrap();
    harness.config.runtime.admin_auth.token_file = Some(token);
    // Alice has a portrait for the admin app to fetch.
    let mut alice = member_json(ALICE, "alice", false, &[BOSSING, ADMIN_ROLE]);
    alice["user"]["avatar"] = json!("0123456789abcdef0123456789abcdef");
    harness.fake.seed_members(
        Id::new(GUILD),
        vec![parse(alice), guild_member(BOB, "bob", false, &[BOSSING])],
    );
    harness
}

/// The delivery homes (A is watched) and a chat category with its channel.
fn guild() -> Event {
    let category = json!({
        "id": CHAT_CATEGORY.to_string(), "type": 4, "name": "chat",
        "parent_id": null, "position": 0, "permission_overwrites": [],
    });
    let mut chat = channel_json(CHAT_CHANNEL);
    chat["parent_id"] = json!(CHAT_CATEGORY.to_string());
    guild_create_with(
        vec![
            channel_json(HOME_A),
            channel_json(HOME_B),
            channel_json(HOME_C),
            category,
            chat,
        ],
        Vec::new(),
    )
}

/// Alice (bossing role, which is also the chat pilot role) posts `content`.
fn message(n: u64, channel: u64, content: &str, to_bot: bool) -> Event {
    let now = auth::system_now();
    let mentions = if to_bot {
        let mut bot = user_json(SELF, "kanade", true);
        bot["public_flags"] = json!(0);
        json!([bot])
    } else {
        json!([])
    };
    Event::MessageCreate(Box::new(parse::<MessageCreate>(json!({
        "id": (snowflake_before(now).get() + 1 + n).to_string(),
        "channel_id": channel.to_string(),
        "guild_id": GUILD.to_string(),
        "author": user_json(ALICE, "alice", false),
        "member": {
            "roles": [BOSSING.to_string()],
            "joined_at": "2026-01-01T00:00:00.000000+00:00",
            "deaf": false, "mute": false, "flags": 0,
        },
        "content": content,
        "timestamp": twilight_model::util::Timestamp::from_micros(now.timestamp_micros())
            .unwrap().iso_8601().to_string(),
        "edited_timestamp": null,
        "tts": false, "mention_everyone": false, "mentions": mentions, "mention_roles": [],
        "attachments": [], "embeds": [], "pinned": false, "type": 0,
    }))))
}

// ---- Admin HTTP over the served listener ----

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> String {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    match body {
        Some(body) => text.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )),
        None => text.push_str("\r\n"),
    }
    text
}

/// One request; `(status, head, body)`.
async fn http(address: SocketAddr, text: String) -> (u16, String, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap();
    (status, head.to_owned(), body.to_owned())
}

/// The break-glass sign-in's session cookie.
async fn sign_in(address: SocketAddr) -> String {
    let body = format!(r#"{{"token":"{TOKEN}"}}"#);
    let (status, head, text) = http(
        address,
        request(
            "POST",
            "/api/admin/auth/token",
            &[("Origin", ORIGIN)],
            Some(&body),
        ),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    head.split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("set-cookie"))
        .and_then(|(_, value)| value.trim().split(';').next())
        .unwrap()
        .to_owned()
}

/// Opens the admin event stream, waits for its `ready` event, then reads it
/// to the end on a task that marks when it ended.
async fn open_events(address: SocketAddr, cookie: &str) -> JoinHandle<()> {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let text = request("GET", "/api/admin/events", &[("Cookie", cookie)], None);
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    while !String::from_utf8_lossy(&seen).contains("event: ready") {
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "{}", String::from_utf8_lossy(&seen));
        seen.extend_from_slice(&chunk[..read]);
    }
    assert!(String::from_utf8_lossy(&seen).starts_with("HTTP/1.1 200"));
    tokio::spawn(async move {
        while stream.read(&mut chunk).await.is_ok_and(|read| read > 0) {}
        mark("v03_event_stream_ended");
    })
}

// ---- Serve ----

/// `serve_with` → `serve_live` (`super::super`), line for line, with the
/// Discord side kept so its completed steps can be read, each phase end
/// marked, and the avatar cache's CDN swapped for `cdn`.
async fn serve_live(
    config: &ServeConfig,
    shutdown: impl Future<Output = ()>,
    wiring: Wiring<Script, FakeDiscord>,
    cdn: (std::path::PathBuf, SlowCdn),
) -> (Result<(), Error>, Vec<&'static str>) {
    let store = store::open(&config.store).await.unwrap();
    let ended = async {
        let prepared = discord::prepare(config, wiring.tick);
        let health = LiveHealth::new(store.clone())
            .with_discord(prepared.probe.clone(), prepared.tick_status.clone())
            .with_extraction(prepared.extraction.clone());
        let mut composition =
            api::compose(config, store.clone(), prepared.cache.clone(), health).await?;
        // Test-only: routes stay external until the startup listing names
        // their zone (as in the extraction suite).
        composition.model_tasks.report_done().await;
        Arc::get_mut(&mut composition.admin.state)
            .expect("nothing shares the state yet")
            .avatars = Some(Arc::new(AvatarCache::new(
            Some(cdn.0),
            Some(Box::new(cdn.1)),
        )));
        let mut discord =
            discord::start(config, store.clone(), &mut composition, prepared, wiring).await?;
        let served = server::serve(&config.runtime, Some(composition.admin), async {
            discord.until(shutdown).await;
            mark("v03_discord_stopped");
        })
        .await;
        mark("v03_http_drained");
        discord.stop().await;
        Ok::<_, Error>((discord.result().and(served), discord.steps().to_vec()))
    }
    .await;
    store::close(store, super::super::CLOSE_WAIT).await;
    mark("v03_serve_returned");
    match ended {
        Ok(ended) => ended,
        Err(error) => (Err(error), Vec::new()),
    }
}

/// A future's panic as its output, so the caller can still wind serve down.
struct Caught<F>(std::pin::Pin<Box<F>>);

impl<F: Future> Future for Caught<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let inner = self.0.as_mut();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Ok(std::task::Poll::Ready(output)) => std::task::Poll::Ready(Ok(output)),
            Err(panic) => std::task::Poll::Ready(Err(panic)),
        }
    }
}

// ---- The test ----

#[tokio::test]
async fn shutdown_with_everything_in_flight_is_ordered_and_inside_the_stop_grace() {
    logging::capture();
    let model = HangingModel::start().await;
    let harness = harness(&model);
    let policy = policy(&harness);
    let now = auth::system_now();
    harness
        .seed(async |store| {
            for (id, name) in [(ALICE, "alice"), (BOB, "bob")] {
                store
                    .apply_gateway(gateway_member(id, name, &[BOSSING]))
                    .await
                    .unwrap();
            }
            // Two due notices: the tick's first send is held, the second
            // must never start once shutdown has begun.
            cancelled_with_notice(store, &policy, now, HOME_A).await;
            cancelled_with_notice(store, &policy, now, HOME_B).await;
        })
        .await;
    let fake = &harness.fake;
    // The first Discord message create is the tick's first notice.
    let tick_send = fake.hold(Op::Create);
    let holds = Mutex::new(vec![tick_send.clone()]);
    let timeline = Timeline::default();
    let (events, source) = script();
    let wiring = Wiring {
        source,
        transport: Arc::clone(fake),
        clock: Arc::new(auth::system_now),
        tick: TICK,
        extraction: TIMING,
    };
    let cdn = SlowCdn::default();
    let (trigger, triggered) = oneshot::channel::<()>();
    let serve = serve_live(
        &harness.config,
        async {
            let _ = triggered.await;
        },
        wiring,
        (harness._temp.0.join("avatars"), cdn.clone()),
    );

    struct InFlight {
        creates: usize,
        probe: Hold,
        avatar: JoinHandle<(u16, String)>,
        stream: JoinHandle<()>,
    }
    let scenario = async {
        eventually!("the admin listener", !logged("server_started").is_empty());
        let address: SocketAddr = logged("server_started")[0]["bind"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        events.send(ready()).unwrap();
        events.send(guild()).unwrap();

        // 3. The tick inside its first send.
        tokio::time::timeout(Duration::from_secs(10), tick_send.entered())
            .await
            .expect("the tick's send");

        // 1. A chat answer hanging on the model, its placeholder posted.
        events
            .send(message(
                1,
                CHAT_CHANNEL,
                &format!("<@{SELF}> when is lotus?"),
                true,
            ))
            .unwrap();
        eventually!(
            "the chat call and its placeholder",
            model.calls().0 == 1 && fake.count(Op::Create) == 1
        );

        // 6. An admin request inside a CDN fetch (Alice's portrait). Not a
        // digest post: the tick's send holds the one delivery operation, so
        // a digest post now is refused at once (503), never in flight.
        let cookie = sign_in(address).await;
        let text = request(
            "GET",
            &format!("/api/admin/members/{ALICE}/avatar"),
            &[("Cookie", &cookie)],
            None,
        );
        let mut avatar = tokio::spawn(async move {
            let (status, _, body) = http(address, text).await;
            mark("v03_avatar_answered");
            (status, body)
        });
        tokio::select! {
            () = cdn.entered.notified() => {}
            done = &mut avatar => panic!("the avatar request ended unfetched: {done:?}"),
            () = sleep(Duration::from_secs(10)) => panic!("no avatar fetch"),
        }

        // 2. An extraction call hanging on the model.
        events
            .send(message(2, HOME_A, "nkalos amend to 10pm", false))
            .unwrap();
        eventually!("the extraction call", model.calls().1 == 1);

        // 4. A slash command inside its reply.
        let reply = fake.hold(Op::Respond);
        holds.lock().unwrap().push(reply.clone());
        events
            .send(slash(
                COMMAND,
                70,
                GUILD,
                Some(GUILD),
                ALICE,
                "schedule",
                json!([]),
            ))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), reply.entered())
            .await
            .expect("the command's reply");

        // 5. An open admin event stream.
        let stream = open_events(address, &cookie).await;

        // Any message create starting from here on parks on this (already
        // released) hold and so shows up as entered.
        let probe = fake.hold(Op::Create);
        probe.release();
        let creates = fake.count(Op::Create);

        mark("v03_trigger");
        let at = Instant::now();
        // Each held call returns as a production one would: the reply at
        // Discord's interaction deadline, the send after a slow attempt.
        for (hold, tail) in [
            (reply, TransportConfig::default().respond_deadline),
            (tick_send.clone(), TICK_SEND_TAIL),
        ] {
            tokio::spawn(async move {
                tokio::time::sleep_until(at + tail).await;
                hold.release();
            });
        }
        let release = Arc::clone(&cdn.release);
        tokio::spawn(async move {
            tokio::time::sleep_until(at + AVATAR_FETCH_TAIL).await;
            release.notify_one();
        });
        trigger.send(()).unwrap();
        InFlight {
            creates,
            probe,
            avatar,
            stream,
        }
    };
    let ((served, steps), held) = tokio::join!(
        async {
            tokio::select! {
                ended = serve => ended,
                never = timeline.clone().watch() => match never {},
            }
        },
        async {
            // A failed setup still shuts serve down (its trigger dropped,
            // every held call let go) before the failure is reported.
            let held = Caught(Box::pin(scenario)).await;
            if held.is_err() {
                for hold in holds.lock().unwrap().iter() {
                    hold.release();
                }
                cdn.release.notify_one();
            }
            held
        }
    );
    let held = held.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let (status, body) = held.avatar.await.unwrap();
    held.stream.await.unwrap();
    timeline.scan();
    let breakdown = timeline.breakdown();
    eprintln!("V03 shutdown breakdown (from trigger):\n{breakdown}");

    // Ordered: the Discord side (its steps), then HTTP, then the store.
    assert!(served.is_ok(), "{served:?}\n{breakdown}");
    assert_eq!(
        steps,
        [
            "gateway_closed",
            "chat_stopped",
            "extraction_stopped",
            "workers_stopped",
            "tick_stopped"
        ],
        "{breakdown}"
    );
    let order = [
        "v03_trigger",
        "gateway_tasks_aborted",
        "gateway_closed",
        "chat_stopped",
        "extraction_calls_cancelled",
        "tick_stopped",
        "v03_discord_stopped",
        "shutdown_started",
        "v03_event_stream_ended",
        // The avatar handler, still running when the drain began.
        "avatar_fetch_failed",
        "v03_http_drained",
        "store_closed",
        "v03_serve_returned",
    ];
    for pair in order.windows(2) {
        assert!(
            timeline.first(pair[0]).0 < timeline.first(pair[1]).0,
            "{pair:?}\n{breakdown}"
        );
    }
    let at = |event: &str| timeline.first(event).1;
    assert!(logged("store_close_failed").is_empty(), "{breakdown}");

    // 4. The command task outlived the drain grace and was aborted unanswered.
    let aborted = logged("gateway_tasks_aborted");
    assert_eq!(aborted.len(), 1, "{breakdown}");
    assert_eq!(aborted[0]["tasks"], 1);
    assert_eq!(fake.count(Op::Respond), 0);

    // 3. The running tick finished its one send; nothing new started.
    assert!(
        tokio::time::timeout(Duration::ZERO, held.probe.entered())
            .await
            .is_err(),
        "a message create started after shutdown began\n{breakdown}"
    );
    assert_eq!(
        fake.count(Op::Create),
        held.creates + 1,
        "only the held send completed"
    );
    let notices = harness.creates_in(HOME_A).len() + harness.creates_in(HOME_B).len();
    assert_eq!(notices, 1, "the second notice was never sent");
    assert!(
        at("tick_stopped") >= at("v03_trigger") + TICK_SEND_TAIL,
        "stop waited for the tick's send\n{breakdown}"
    );

    // 5 and 6. The stream ended with the HTTP shutdown (ordered above); the
    // avatar request was still in flight then and the drain let it finish (a
    // monogram, as the fetch failed).
    assert_eq!(status, 200, "{body}");
    assert!(
        timeline.first("v03_avatar_answered").0 > timeline.first("shutdown_started").0,
        "{breakdown}"
    );

    // 1 and 2. Both hanging calls were cut and logged as such.
    let store = store::open(&harness.config.store).await.unwrap();
    let chats = store
        .list_chats(&ChatFilter {
            limit: 10,
            ..ChatFilter::default()
        })
        .await
        .unwrap()
        .items;
    assert_eq!(chats.len(), 1, "{chats:?}");
    // `chat::driver::run::CUT` (private) reads the same.
    assert!(
        chats[0]
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("cancelled: serve shut down")),
        "{:?}",
        chats[0]
    );
    let extractions = store
        .list_extractions(&ExtractionFilter {
            limit: 10,
            ..ExtractionFilter::default()
        })
        .await
        .unwrap()
        .items;
    assert_eq!(extractions.len(), 1, "{extractions:?}");
    assert_eq!(extractions[0].outcome, ExtractionOutcome::Failed);
    assert_eq!(extractions[0].error.as_deref(), Some(CALL_CANCELLED));
    store::close(store, Duration::ZERO).await;
    assert_eq!(model.calls(), (1, 1), "no model call after the cut");

    let total = at("v03_serve_returned") - at("v03_trigger");
    assert!(
        total < COMPOSE_STOP_GRACE,
        "shutdown took {total:?}, over Compose's {COMPOSE_STOP_GRACE:?}\n{breakdown}"
    );
}
