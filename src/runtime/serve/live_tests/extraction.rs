//! Extraction in live serve: the scripted gateway and fake transport of the
//! parent module, a loopback model gateway and the admin router over the
//! same composition. Debounce and backlog pace are shortened.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::{Json, Router, routing::get, routing::post};
use chrono::{NaiveTime, TimeZone};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use twilight_model::gateway::payload::incoming::MessageCreate;

use super::*;
use crate::{
    api::{auth::wire::SESSION_COOKIE, listeners},
    bot::{extract_feed::snowflake_before, transport::HistoryPage},
    domain::{
        model_log::{ExtractionFilter, ExtractionOutcome, ModelLogStore, RescanJob, RescanStatus},
        proposals::ProposalCardStore,
    },
    extract::rescan::{RescanError, RescanRequest},
};

const ALIAS: &str = "kanata-extract";
const TOKEN: &str = "extraction-break-glass-token-0123456789abcdef";
const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const CATEGORY: u64 = 400;
const PARTY: u64 = 401;
const THREAD: u64 = 402;
const TIMING: extract::Timing = extract::Timing {
    debounce: Duration::from_millis(150),
    drain_interval: Duration::from_millis(40),
};

// ---- The model gateway ----

#[derive(Clone)]
struct Model {
    url: String,
    chats: Arc<AtomicUsize>,
    /// The reply content every completion gets.
    answer: Arc<Mutex<String>>,
    /// How long each completion takes.
    delay: Arc<Mutex<Duration>>,
}

impl Model {
    async fn start(trust_zone: &str) -> Self {
        let listing = json!({"object": "list", "data": [{
            "id": ALIAS, "object": "model",
            "kanata": {
                "operations": ["chat"], "structured_output": true,
                "sampling_controls": true, "reasoning_control": false,
                "function_tools": true, "streaming": false,
                "trust_zone": trust_zone, "context_tokens": 32768,
            },
        }]});
        let chats = Arc::new(AtomicUsize::new(0));
        let answer = Arc::new(Mutex::new(nothing()));
        let delay = Arc::new(Mutex::new(Duration::ZERO));
        let (count, reply, wait) = (chats.clone(), answer.clone(), delay.clone());
        let app = Router::new()
            .route("/v1/models", get(move || async move { Json(listing) }))
            .route(
                "/v1/chat/completions",
                post(move || async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    let pause = *wait.lock().unwrap();
                    sleep(pause).await;
                    let content = reply.lock().unwrap().clone();
                    Json(json!({
                        "id": "chatcmpl-1", "object": "chat.completion", "model": ALIAS,
                        "choices": [{"index": 0, "finish_reason": "stop",
                            "message": {"role": "assistant", "content": content}}],
                        "usage": {"prompt_tokens": 3, "completion_tokens": 2},
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            url: format!("http://{addr}/v1"),
            chats,
            answer,
            delay,
        }
    }

    fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
    }

    fn answer(&self, content: String) {
        *self.answer.lock().unwrap() = content;
    }
}

fn nothing() -> String {
    r#"{"amendments": [], "summary": "no schedule change"}"#.to_owned()
}

fn moved(evidence: u64) -> String {
    json!({
        "amendments": [{
            "kind": "move", "bosses": ["NKalos"], "day_ref": null, "time_ref": "10pm",
            "participants": [ALICE.to_string()], "rsvp": null, "is_question": false,
            "confidence": 0.9, "evidence_message_ids": [evidence.to_string()],
            "target_run_hint": null,
        }],
        "summary": "nkalos to 10pm",
    })
    .to_string()
}

// ---- Harness ----

fn harness(model: &Model, enabled: bool) -> Harness {
    harness_with(model, enabled, &[])
}

fn harness_with(model: &Model, enabled: bool, extra: &[(&str, &str)]) -> Harness {
    let mut values = vec![
        ("KANADE_MODEL_BASE_URL", model.url.as_str()),
        ("KANADE_EXTRACT_MODEL", ALIAS),
        ("KANADE_WATCH_CHANNEL_IDS", "301"),
        ("KANADE_WATCH_CATEGORY_IDS", "400"),
        ("KANADE_EXTRACTION_ENABLED", if enabled { "1" } else { "0" }),
        ("KANADE_ADMIN_HOST", "kanade.test"),
    ];
    values.extend_from_slice(extra);
    let mut harness = Harness::with(&values);
    let token = harness._temp.0.join("admin_token");
    std::fs::write(&token, format!("{TOKEN}\n")).unwrap();
    harness.config.runtime.admin_auth.token_file = Some(token);
    harness.timing = TIMING;
    harness.fake.seed_members(
        Id::new(GUILD),
        vec![
            guild_member(ALICE, "alice", false, &[BOSSING]),
            guild_member(BOB, "bob", false, &[BOSSING]),
        ],
    );
    harness
}

/// Home A (watched by id), a category with a party channel, and a thread
/// under that channel (watched through the category).
fn guild() -> Event {
    let category = json!({
        "id": CATEGORY.to_string(), "type": 4, "name": "bossing",
        "parent_id": null, "position": 0, "permission_overwrites": [],
    });
    let mut party = channel_json(PARTY);
    party["parent_id"] = json!(CATEGORY.to_string());
    let thread = json!({
        "id": THREAD.to_string(), "type": 11, "name": "tonight",
        "parent_id": PARTY.to_string(), "owner_id": ALICE.to_string(),
        "thread_metadata": {
            "archived": false, "auto_archive_duration": 1440,
            "archive_timestamp": "2026-09-25T12:00:00.000000+00:00", "locked": false,
        },
    });
    guild_create_with(
        vec![channel_json(HOME_A), channel_json(HOME_B), category, party],
        vec![thread],
    )
}

async fn started(harness: &Harness) -> (Discord, Ctx) {
    // As in production, the store already knows the members: the startup
    // rescan can run before the roster task has reconciled.
    harness
        .seed(async |store| {
            for (id, name) in [(ALICE, "alice"), (BOB, "bob")] {
                store
                    .apply_gateway(gateway_member(id, name, &[BOSSING]))
                    .await
                    .unwrap();
            }
        })
        .await;
    let (discord, mut ctx) = harness.start().await;
    // Routes stay external until the startup listing names their zone.
    ctx.composition.model_tasks.report_done().await;
    (discord, ctx)
}

/// Guild up and the roster (bossing role holders) reconciled.
async fn connect(ctx: &Ctx) {
    ctx.events.send(ready()).unwrap();
    ctx.events.send(guild()).unwrap();
    eventually!(
        "reconciliation",
        roles_of(&ctx.store, ALICE).await.is_some() && roles_of(&ctx.store, BOB).await.is_some()
    );
    sleep(TICK * 4).await;
}

fn snowflake(at: DateTime<Utc>, n: u64) -> u64 {
    snowflake_before(at).get() + 1 + n
}

fn message_json(
    id: u64,
    channel: u64,
    author: (u64, bool),
    content: &str,
    at: DateTime<Utc>,
) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": channel.to_string(),
        "guild_id": GUILD.to_string(),
        "author": user_json(author.0, "someone", author.1),
        "content": content,
        "timestamp": twilight_model::util::Timestamp::from_micros(at.timestamp_micros()).unwrap().iso_8601().to_string(),
        "edited_timestamp": null,
        "tts": false, "mention_everyone": false, "mentions": [], "mention_roles": [],
        "attachments": [], "embeds": [], "pinned": false, "type": 0,
    })
}

fn posted(id: u64, channel: u64, author: u64, content: &str, at: DateTime<Utc>) -> Event {
    Event::MessageCreate(Box::new(parse::<MessageCreate>(message_json(
        id,
        channel,
        (author, author == SELF || author == BOT),
        content,
        at,
    ))))
}

async fn cached(ctx: &Ctx, id: u64) -> Option<crate::domain::model_log::WatchedMessage> {
    ctx.store
        .messages_by_ids(&[id.to_string()])
        .await
        .unwrap()
        .pop()
}

async fn logs(ctx: &Ctx) -> Vec<crate::domain::model_log::ExtractionLog> {
    let mut items = ctx
        .store
        .list_extractions(&ExtractionFilter {
            limit: 50,
            ..ExtractionFilter::default()
        })
        .await
        .unwrap()
        .items;
    items.reverse();
    items
}

async fn jobs(ctx: &Ctx) -> Vec<RescanJob> {
    ctx.store.recent_rescan_jobs(20).await.unwrap()
}

async fn job(ctx: &Ctx, source: &str) -> Option<RescanJob> {
    jobs(ctx).await.into_iter().find(|job| job.source == source)
}

fn replies(fake: &FakeDiscord) -> Vec<String> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Respond { reply, .. } | Call::CompleteDeferred { reply, .. } => {
                Some(reply.content)
            }
            _ => None,
        })
        .collect()
}

// ---- The admin API over the same composition ----

struct Admin {
    address: SocketAddr,
    cookie: String,
    csrf: String,
    /// Holds the API state (and so the store) until stopped.
    server: tokio::task::JoinHandle<()>,
}

async fn http(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, Vec<(String, String)>, String) {
    let mut text =
        format!("{method} {path} HTTP/1.1\r\nHost: kanade.test\r\nConnection: close\r\n");
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
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    (status, headers, body.to_owned())
}

impl Admin {
    async fn start(ctx: &Ctx, harness: &Harness) -> Self {
        let mut site = listeners::Site::admin(&harness.config.runtime.http);
        site.auth = Some(Arc::clone(&ctx.composition.admin.auth));
        site.state = Some(Arc::clone(&ctx.composition.admin.state));
        site.health = Some(Arc::clone(&ctx.composition.admin.health));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        site.listener_ip = Some(address.ip());
        let router = listeners::router(site);
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        let body = format!(r#"{{"token":"{TOKEN}"}}"#);
        let (status, headers, text) = http(
            address,
            "POST",
            "/api/admin/auth/token",
            &[ORIGIN],
            Some(&body),
        )
        .await;
        assert_eq!(status, 200, "{text}");
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        let cookie = header("set-cookie").split(';').next().unwrap().to_owned();
        assert!(cookie.starts_with(SESSION_COOKIE));
        Self {
            address,
            cookie,
            csrf: header("x-kanade-csrf"),
            server,
        }
    }

    async fn stop(self) {
        self.server.abort();
        let _ = self.server.await;
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let body = body.map(|body| body.to_string());
        let (status, _, text) = http(
            self.address,
            method,
            path,
            &[
                ORIGIN,
                ("Cookie", &self.cookie),
                ("X-Kanade-CSRF", &self.csrf),
            ],
            body.as_deref(),
        )
        .await;
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

// ---- Tests ----

#[tokio::test]
async fn extraction_off_records_watched_messages_but_never_calls_the_model() {
    let model = Model::start("local").await;
    let harness = harness(&model, false);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        let now = auth::system_now();
        let id = snowflake(now, 1);
        ctx.events
            .send(posted(id, HOME_A, ALICE, "nkalos amend to 10pm @here", now))
            .unwrap();
        eventually!("the cached message", cached(&ctx, id).await.is_some());
        sleep(TIMING.debounce * 4).await;
        assert_eq!(model.chats(), 0);
        assert!(logs(&ctx).await.is_empty());
        assert_eq!(
            ctx.health.health().await.extraction,
            Some("disabled"),
            "informational only"
        );
        assert_eq!(ctx.health.health().await.status, "ok");
        assert!(jobs(&ctx).await.is_empty(), "no startup rescan while off");

        // Rescans are refused too, from the API port and from /rescan.
        let runner = &ctx.composition.admin.state.rescans.as_ref().unwrap().runner;
        let refused = runner
            .submit(RescanRequest {
                channels: vec![HOME_A.to_string()],
                window: "week".into(),
                source: "portal".into(),
                automated: false,
                requested_by: None,
            })
            .await;
        assert_eq!(refused.unwrap_err(), RescanError::Closed);
        ctx.events
            .send(slash(
                COMMAND,
                70,
                GUILD,
                Some(GUILD),
                ALICE,
                "rescan",
                json!([]),
            ))
            .unwrap();
        eventually!(
            "the refusal",
            replies(&harness.fake)
                .iter()
                .any(|reply| reply.contains("Rescans aren't available"))
        );
        assert!(jobs(&ctx).await.is_empty());
        assert_eq!(model.chats(), 0);
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn a_message_becomes_a_card_and_a_participants_check_moves_the_run() {
    let model = Model::start("local").await;
    let harness = harness(&model, true);
    let policy = policy(&harness);
    let zone = harness.config.runtime.timezone;
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        let now = auth::system_now();
        let tomorrow = now.with_timezone(&zone).date_naive() + chrono::Days::new(1);
        let local = |hour| {
            zone.from_local_datetime(&tomorrow.and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap()))
                .single()
                .unwrap()
                .with_timezone(&Utc)
        };
        let run = SchedulerService::new(&*ctx.store, RandomIds, FixedClock(now))
            .with_attendance(policy.attendance)
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: None,
                channel_id: Some(HOME_A.to_string()),
                week_start: week_of(&policy, local(20)),
                datetime: local(20),
                bosses: vec!["NKalos".into()],
                participants: vec![ALICE.to_string(), BOB.to_string()],
                status: RunStatus::Planned,
                source: RunSource::Fixed,
            })
            .await
            .unwrap();
        connect(&ctx).await;
        assert_eq!(ctx.health.health().await.extraction, Some("idle"));

        let evidence = snowflake(now, 10);
        model.answer(moved(evidence));
        // The loop guard: the bot's own and other bots' posts are never cached.
        ctx.events
            .send(posted(snowflake(now, 8), HOME_A, SELF, "nkalos amend to 10pm", now))
            .unwrap();
        ctx.events
            .send(posted(snowflake(now, 9), HOME_A, BOT, "nkalos amend to 10pm", now))
            .unwrap();
        ctx.events
            .send(posted(evidence, HOME_A, ALICE, "nkalos amend to 10pm", now))
            .unwrap();
        eventually!("the extraction", !logs(&ctx).await.is_empty());
        let log = logs(&ctx).await.remove(0);
        assert_eq!(log.outcome, ExtractionOutcome::Proposed, "{log:?}");
        assert_eq!(log.message_ids, [evidence.to_string()]);
        assert_eq!(model.chats(), 1);
        for guarded in [8, 9] {
            assert!(cached(&ctx, snowflake(now, guarded)).await.is_none());
        }
        let proposal = log.proposal_ids[0].clone();
        eventually!(
            "the card",
            ctx.store
                .load_cards(std::slice::from_ref(&proposal))
                .await
                .unwrap()
                .first()
                .is_some_and(|card| card.message_id.is_some())
        );
        let card = ctx
            .store
            .load_cards(std::slice::from_ref(&proposal))
            .await
            .unwrap()
            .remove(0)
            .message_id
            .unwrap();

        // Bob is on the run too: a participant's ✅ approves it.
        ctx.events
            .send(reaction(Some(GUILD), HOME_A, &card, BOB, "✅"))
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
        let snapshot = ctx.store.load(&Scope::Run(run.clone())).await.unwrap();
        assert_eq!(snapshot.runs[0].datetime, local(22));
        eventually!(
            "the merge notice and the refreshed card",
            ctx.store
                .pending_notices()
                .await
                .unwrap()
                .notices
                .is_empty()
                && harness.fake.calls().iter().any(|call| matches!(
                    call,
                    Call::Edit { edit, .. }
                        if edit.content.as_deref().is_some_and(|text| text.contains("✅ applied by"))
                ))
        );
        let posts = harness.creates_in(HOME_A);
        assert!(posts.len() >= 2, "card and merge notice: {posts:?}");
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn a_thread_under_a_watched_category_is_read_under_its_parent_channel() {
    let model = Model::start("local").await;
    let harness = harness(&model, true);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        let now = auth::system_now();
        let (in_thread, unwatched) = (snowflake(now, 1), snowflake(now, 2));
        ctx.events
            .send(posted(
                unwatched,
                HOME_B,
                ALICE,
                "nkalos amend to 10pm",
                now,
            ))
            .unwrap();
        ctx.events
            .send(posted(
                in_thread,
                THREAD,
                ALICE,
                "nkalos amend to 10pm",
                now,
            ))
            .unwrap();
        eventually!("the extraction", !logs(&ctx).await.is_empty());
        let row = cached(&ctx, in_thread).await.unwrap();
        assert_eq!(row.channel_id, PARTY.to_string(), "filed under the parent");
        let log = logs(&ctx).await.remove(0);
        assert_eq!(log.channel_id.as_deref(), Some(PARTY.to_string().as_str()));
        assert_eq!(log.outcome, ExtractionOutcome::NoChange);
        assert!(cached(&ctx, unwatched).await.is_none(), "not watched");
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn stale_history_is_cached_for_rescans_but_makes_no_card() {
    let model = Model::start("local").await;
    let harness = harness(&model, true);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        let old = auth::system_now() - chrono::Duration::minutes(10);
        let id = snowflake(old, 1);
        ctx.events
            .send(posted(id, HOME_A, ALICE, "nkalos amend to 10pm @here", old))
            .unwrap();
        eventually!("the cached message", cached(&ctx, id).await.is_some());
        sleep(TIMING.debounce * 4 + TIMING.drain_interval * 4).await;
        assert_eq!(model.chats(), 0);
        assert!(logs(&ctx).await.is_empty());
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn rescans_page_discord_history_from_startup_the_api_and_the_slash_command() {
    let model = Model::start("local").await;
    let harness = harness(&model, true);
    let now = auth::system_now();
    // 130 chatty messages over the last hour (two pages), one gated one, and
    // one from a month ago (outside every window read here).
    let mut history: Vec<twilight_model::channel::Message> = (0..130u64)
        .map(|n| {
            let at = now - chrono::Duration::minutes(60) + chrono::Duration::seconds(n as i64);
            parse(message_json(
                snowflake(at, n),
                HOME_A,
                (ALICE, false),
                "just chatting",
                at,
            ))
        })
        .collect();
    let gated_at = now - chrono::Duration::minutes(20);
    history.push(parse(message_json(
        snowflake(gated_at, 500),
        HOME_A,
        (ALICE, false),
        "nkalos amend to 10pm",
        gated_at,
    )));
    let old = now - chrono::Duration::days(30);
    history.push(parse(message_json(
        snowflake(old, 1),
        HOME_A,
        (ALICE, false),
        "nkalos wed?",
        old,
    )));
    harness.fake.seed_history(history);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        eventually!(
            "the startup rescan",
            job(&ctx, "startup")
                .await
                .is_some_and(|job| job.status.is_final())
        );
        let startup = job(&ctx, "startup").await.unwrap();
        assert_eq!(startup.status, RescanStatus::Done, "{startup:?}");
        assert!(startup.automated);
        assert_eq!(startup.window, "24h");
        assert_eq!(startup.channels, [HOME_A.to_string(), PARTY.to_string()]);
        let result = &startup.results[0];
        assert_eq!(result["backfilled"], 131, "{result}");
        assert_eq!(result["gated"], 1, "{result}");
        assert_eq!(result["calls"], 1, "{result}");
        assert_eq!(result["unread"], 0, "{result}");
        let pages: Vec<HistoryPage> = harness
            .fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::ChannelMessages { page, limit, .. } => {
                    assert_eq!(limit, 100);
                    Some(page)
                }
                _ => None,
            })
            .collect();
        assert!(
            pages.len() >= 2
                && pages
                    .iter()
                    .all(|page| matches!(page, HistoryPage::After(_)))
        );

        // The API: the admin router over the same state.
        let admin = Admin::start(&ctx, &harness).await;
        let (status, queued) = admin
            .send(
                "POST",
                "/api/admin/rescan",
                Some(json!({"channels": [HOME_A.to_string()], "window": "two_weeks"})),
            )
            .await;
        assert_eq!(status, 200, "{queued}");
        let id = queued["id"].as_str().unwrap().to_owned();
        let view = async || {
            admin
                .send("GET", &format!("/api/admin/rescan/{id}"), None)
                .await
                .1
        };
        eventually!(
            "the API rescan",
            view().await["state"]
                .as_str()
                .is_some_and(|state| state != "queued" && state != "running")
        );
        let view = view().await;
        assert_eq!(view["state"], "done", "{view}");
        assert_eq!(view["unread"], 0, "{view}");
        assert_eq!(view["channels"][0]["unread"], 0, "{view}");
        let api = jobs(&ctx)
            .await
            .into_iter()
            .find(|job| job.id == id)
            .unwrap();
        assert_eq!(api.status, RescanStatus::Done, "{api:?}");
        assert!(!api.automated);
        let result = &api.results[0];
        assert_eq!(
            (result["gated"].clone(), result["unread"].clone()),
            (json!(1), json!(0)),
            "{result}"
        );

        // `/rescan` in the watched channel.
        ctx.events
            .send(slash(
                COMMAND,
                71,
                GUILD,
                Some(GUILD),
                ALICE,
                "rescan",
                json!([{"name": "window", "type": 3, "value": "24h"}]),
            ))
            .unwrap();
        eventually!(
            "the slash rescan",
            job(&ctx, "slash")
                .await
                .is_some_and(|job| job.status.is_final())
        );
        assert!(
            replies(&harness.fake)
                .iter()
                .any(|reply| reply.contains("Re-reading"))
        );
        let slash_job = job(&ctx, "slash").await.unwrap();
        assert_eq!(slash_job.status, RescanStatus::Done);
        assert_eq!(slash_job.results[0]["unread"], 0);
        assert_eq!(
            slash_job.requested_by.as_deref(),
            Some(ALICE.to_string().as_str())
        );
        assert_eq!(ctx.health.health().await.extraction, Some("idle"));
        admin.stop().await;
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn an_external_extraction_route_is_refused_and_logged_without_a_call() {
    let model = Model::start("external").await;
    let harness = harness(&model, true);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        let now = auth::system_now();
        ctx.events
            .send(posted(
                snowflake(now, 1),
                HOME_A,
                ALICE,
                "nkalos amend to 10pm",
                now,
            ))
            .unwrap();
        eventually!("the log", !logs(&ctx).await.is_empty());
        let log = logs(&ctx).await.remove(0);
        assert_eq!(log.outcome, ExtractionOutcome::Failed);
        assert!(
            log.error
                .as_deref()
                .is_some_and(|error| error.contains("pseudonymization")),
            "{log:?}"
        );
        assert_eq!(model.chats(), 0, "nothing left the homelab");
        let health = ctx.health.health().await;
        assert_eq!(health.extraction, Some("degraded"));
        assert_eq!(health.status, "ok");
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn saved_settings_switch_extraction_live() {
    let model = Model::start("local").await;
    let harness = harness(&model, false);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        let admin = Admin::start(&ctx, &harness).await;
        let now = auth::system_now();
        ctx.events
            .send(posted(
                snowflake(now, 1),
                HOME_A,
                ALICE,
                "nkalos amend to 10pm",
                now,
            ))
            .unwrap();
        sleep(TIMING.debounce * 3).await;
        assert_eq!(model.chats(), 0);

        let (status, body) = admin
            .send(
                "PATCH",
                "/api/admin/config",
                Some(json!({"watching": {"extract_enabled": true}})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        eventually!(
            "the switch",
            ctx.health.health().await.extraction == Some("idle")
        );
        let now = auth::system_now();
        ctx.events
            .send(posted(
                snowflake(now, 2),
                HOME_A,
                ALICE,
                "nkalos amend to 10pm",
                now,
            ))
            .unwrap();
        eventually!("the call", model.chats() == 1);

        let (status, body) = admin
            .send(
                "PATCH",
                "/api/admin/config",
                Some(json!({"watching": {"paused": true}})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        eventually!(
            "paused",
            ctx.health.health().await.extraction == Some("disabled")
        );
        let now = auth::system_now();
        ctx.events
            .send(posted(
                snowflake(now, 3),
                HOME_A,
                ALICE,
                "nkalos amend to 10pm",
                now,
            ))
            .unwrap();
        eventually!("cached", cached(&ctx, snowflake(now, 3)).await.is_some());
        sleep(TIMING.debounce * 3).await;
        assert_eq!(model.chats(), 1);
        admin.stop().await;
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

#[tokio::test]
async fn shutdown_stops_rescans_before_the_store_closes() {
    let model = Model::start("local").await;
    *model.delay.lock().unwrap() = Duration::from_millis(800);
    let harness = harness(&model, true);
    let at = auth::system_now() - chrono::Duration::minutes(5);
    harness.fake.seed_history(vec![parse(message_json(
        snowflake(at, 1),
        HOME_A,
        (ALICE, false),
        "nkalos amend to 10pm",
        at,
    ))]);
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        eventually!("the startup call", model.chats() == 1);
        let runner = &ctx.composition.admin.state.rescans.as_ref().unwrap().runner;
        let queued = runner
            .submit(RescanRequest {
                channels: vec![HOME_B.to_string()],
                window: "week".into(),
                source: "portal".into(),
                automated: false,
                requested_by: None,
            })
            .await
            .unwrap();
        assert_eq!(queued.job.status, RescanStatus::Queued);
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
    // Written by `Rescans::close` and the stopped worker, before the store closes.
    let startup = job(&ctx, "startup").await.unwrap();
    assert_eq!(startup.status, RescanStatus::Cancelled, "{startup:?}");
    let portal = job(&ctx, "portal").await.unwrap();
    assert_eq!(portal.status, RescanStatus::Cancelled);
    assert_eq!(portal.error.as_deref(), Some("shut down"));
    finish(&harness, ctx).await;
}

/// A pilot's `@Kanade` question in `channel`.
fn question(id: u64, channel: u64, content: &str, at: DateTime<Utc>) -> Value {
    let mut message = message_json(id, channel, (ALICE, false), content, at);
    message["member"] = json!({
        "roles": [BOSSING.to_string(), PILOT.to_string()],
        "joined_at": "2026-01-01T00:00:00.000000+00:00",
        "deaf": false, "mute": false, "flags": 0,
    });
    let mut me = user_json(SELF, "kanade", true);
    me["public_flags"] = json!(0);
    message["mentions"] = json!([me]);
    message
}

const PILOT: u64 = 30;

async fn chat_rows(ctx: &Ctx) -> Vec<crate::domain::model_log::ChatInteraction> {
    ctx.store
        .list_chats(&crate::domain::model_log::ChatFilter {
            limit: 50,
            ..crate::domain::model_log::ChatFilter::default()
        })
        .await
        .unwrap()
        .items
}

#[tokio::test]
async fn a_chat_question_in_the_watched_chat_category_is_never_extracted() {
    let model = Model::start("local").await;
    // As in the live guild: the chat category is the watched category.
    let harness = harness_with(
        &model,
        true,
        &[
            ("KANADE_CHAT_ENABLED", "1"),
            ("KANADE_CHAT_CATEGORY_IDS", "400"),
            ("KANADE_CHAT_PILOT_ROLE_ID", "30"),
            ("KANADE_CHAT_MODEL", ALIAS),
        ],
    );
    let (mut discord, ctx) = started(&harness).await;
    drive(&mut discord, async {
        connect(&ctx).await;
        let now = auth::system_now();
        let asked = snowflake(now, 1);
        let text = format!("<@{SELF}> nkalos amend to 10pm");
        ctx.events
            .send(Event::MessageCreate(Box::new(parse::<MessageCreate>(
                question(asked, PARTY, &text, now),
            ))))
            .unwrap();
        eventually!("the chat answer", !chat_rows(&ctx).await.is_empty());
        eventually!("the cached question", cached(&ctx, asked).await.is_some());
        // Its edit stays chat's too.
        let mut edited = question(asked, PARTY, &format!("{text} pls"), now);
        edited["edited_timestamp"] = json!(
            twilight_model::util::Timestamp::from_micros(auth::system_now().timestamp_micros())
                .unwrap()
                .iso_8601()
                .to_string()
        );
        ctx.events
            .send(Event::MessageUpdate(Box::new(parse(edited))))
            .unwrap();
        sleep(TIMING.debounce * 4).await;
        assert!(logs(&ctx).await.is_empty(), "no extraction of the question");
        assert_eq!(model.chats(), 1, "the chat answer only");

        // The same words without the mention are party chat: extracted.
        let plain = snowflake(auth::system_now(), 2);
        ctx.events
            .send(posted(
                plain,
                PARTY,
                ALICE,
                "nkalos amend to 10pm",
                auth::system_now(),
            ))
            .unwrap();
        eventually!("the extraction", !logs(&ctx).await.is_empty());
        let log = logs(&ctx).await.remove(0);
        assert_eq!(log.message_ids, [plain.to_string()]);
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn shutdown_cuts_hanging_extraction_calls_and_stays_bounded() {
    let model = Model::start("local").await;
    *model.delay.lock().unwrap() = Duration::from_secs(120);
    // One permit: the live burst waits behind the hanging rescan call.
    let harness = harness_with(&model, true, &[("KANADE_MODEL_PERMITS", "1")]);
    let at = auth::system_now() - chrono::Duration::minutes(5);
    harness.fake.seed_history(vec![parse(message_json(
        snowflake(at, 1),
        HOME_A,
        (ALICE, false),
        "nkalos amend to 10pm",
        at,
    ))]);
    let (mut discord, ctx) = started(&harness).await;
    let mut stopping = None;
    drive(&mut discord, async {
        connect(&ctx).await;
        eventually!("the rescan call", model.chats() == 1);
        let now = auth::system_now();
        let live = snowflake(now, 5);
        ctx.events
            .send(posted(live, PARTY, ALICE, "nkalos amend to 10pm", now))
            .unwrap();
        eventually!("the cached message", cached(&ctx, live).await.is_some());
        // Past the debounce: the burst waits for the one permit.
        sleep(TIMING.debounce * 3).await;
        assert_eq!(model.chats(), 1);
        stopping = Some(Instant::now());
    })
    .await;
    let took = stopping.unwrap().elapsed();
    assert!(
        took < extract::STOP_GRACE + extract::CANCEL_GRACE + Duration::from_secs(2),
        "{took:?}"
    );
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
    let rows = logs(&ctx).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in &rows {
        assert_eq!(row.outcome, ExtractionOutcome::Failed);
        assert_eq!(
            row.error.as_deref(),
            Some(crate::extract::pipeline::CALL_CANCELLED)
        );
    }
    let live = rows
        .iter()
        .find(|row| row.channel_id.as_deref() == Some(PARTY.to_string().as_str()))
        .unwrap();
    let unread = cached(&ctx, snowflake_of(live)).await.unwrap();
    assert!(unread.processed_at.is_none(), "left for a later read");
    let startup = job(&ctx, "startup").await.unwrap();
    assert_eq!(startup.status, RescanStatus::Cancelled);
    finish(&harness, ctx).await;
}

fn snowflake_of(row: &crate::domain::model_log::ExtractionLog) -> u64 {
    row.message_ids[0].parse().unwrap()
}
