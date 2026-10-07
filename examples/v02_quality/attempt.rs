//! One attempt: a fresh temp store seeded with the world, live serve's
//! Discord side over the fake transport and the shared model stack, one
//! question or burst at T0, then everything the rubric reads, as JSON.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use kanade::{
    bot::{
        gateway::{EventSource, GatewayError},
        transport::{FakeDiscord, Op},
    },
    domain::{
        model_log::ModelLogStore,
        settings::{SettingsStore, keys},
    },
    infrastructure::{llm::setup::ModelStack, store::SqliteStore},
    runtime::{
        config::ServeConfig,
        serve::{discord::Wiring, extract::Timing, harness, settings},
    },
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep};
use twilight_gateway::Event;
use twilight_model::id::Id;

use crate::{
    cases::{Case, Input, Line},
    collect::{
        card_history, chat_json, chat_row, extraction_logs, label, messages, proposals,
        rsvp_changes, rsvps, runs, tool_calls,
    },
    persona::PersonaBundle,
    requests::Requests,
    world::{self, BOT, C1, ContextMessage, GUILD},
};

/// Shortened from serve's 90 s; still far longer than posting a burst takes.
const TIMING: Timing = Timing {
    debounce: Duration::from_secs(2),
    drain_interval: Duration::from_millis(50),
};
const TICK: Duration = Duration::from_secs(1);
const READY_WAIT: Duration = Duration::from_secs(30);
const ANSWER_WAIT: Duration = Duration::from_secs(600);
const QUIET: Duration = Duration::from_millis(750);

/// What every attempt shares: the model stack, the request counter and the
/// serve environment (paths are filled per attempt).
pub struct Shared {
    pub models: Arc<ModelStack>,
    pub requests: Arc<Requests>,
    pub mapping: BTreeMap<String, String>,
    pub persona: Arc<PersonaBundle>,
}

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

/// A private temp directory, removed on drop (the temp store goes with it).
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> std::io::Result<Self> {
        let base = fs::canonicalize(std::env::temp_dir())?;
        let root = base.join(format!("kanade-v02-{label}-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        Ok(Self(root))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// The serve environment over `dir`: a fake token, a fresh store, the
/// tracked boss catalog and knowledge, and a persona directory holding only
/// the bundle under test and a catalog naming it the default (serve also
/// gets the stored `persona` row: [`select_persona`]).
pub fn config_in(
    dir: &Path,
    mapping: &BTreeMap<String, String>,
    persona: &PersonaBundle,
) -> Result<ServeConfig, String> {
    let path = |relative: &str| dir.join(relative).display().to_string();
    fs::write(dir.join("token"), "v02-harness-fake-token\n").map_err(|e| e.to_string())?;
    let bundles = dir.join("personas/bundles");
    fs::create_dir_all(&bundles).map_err(|e| e.to_string())?;
    fs::copy(&persona.path, bundles.join(&persona.file_name))
        .map_err(|e| format!("persona bundle {}: {e}", persona.file_name))?;
    let id = persona.id.as_str();
    fs::write(
        dir.join("personas/catalog.yaml"),
        format!(
            "schema_version: 1\ndefault: {id}\npersonas:\n  - id: {id}\n    label: '{id}'\n    aliases: []\n"
        ),
    )
    .map_err(|e| format!("persona catalog: {e}"))?;
    let mut values = mapping.clone();
    for (key, value) in [
        ("KANADE_DISCORD_TOKEN_FILE", path("token")),
        ("KANADE_DB_PATH", path("db/kanade.sqlite3")),
        ("KANADE_OWNER_LOCK_DIR", path("locks")),
        ("KANADE_PERSONA_DIR", path("personas")),
        (
            "KANADE_CATALOG_FILE",
            repo("boss/bosses.yaml").display().to_string(),
        ),
        (
            "KANADE_KNOWLEDGE_DIR",
            repo("boss/knowledge").display().to_string(),
        ),
    ] {
        values.insert(key.to_owned(), value);
    }
    ServeConfig::from_mapping(&values).map_err(|error| error.to_string())
}

/// The stored `persona` row, as the live bot has it after the operator's
/// switch (it logs `persona_selected configured=<id> effective=<id>`).
pub async fn select_persona(store: &SqliteStore, persona: &PersonaBundle) -> Result<(), String> {
    store
        .put_settings_rows(
            [(keys::PERSONA.to_owned(), persona.id.as_str().to_owned())]
                .into_iter()
                .collect(),
        )
        .await
        .map_err(|e| format!("persona setting: {e}"))
}

/// A chat turn must answer as the bundle under test in its default voice;
/// anything else is a setup error, not a model result.
fn check_persona(record: &Value, persona: &PersonaBundle) -> Result<(), String> {
    let row = &record["chat"]["interaction"];
    if row.is_null() {
        return Ok(());
    }
    let effective = row["persona"].as_str().unwrap_or_default();
    let source = row["profile_source"].as_str().unwrap_or_default();
    if effective == persona.id.as_str() && source == "default" {
        Ok(())
    } else {
        Err(format!(
            "{SETUP} the turn answered as persona `{effective}` (profile source `{source}`), not `{}` in its default voice",
            persona.id.as_str()
        ))
    }
}

/// Errors that invalidate the run rather than the attempt.
pub const SETUP: &str = "setup:";

pub async fn run(case: &Case, attempt: u32, shared: &Shared) -> Value {
    let started = Instant::now();
    let sent_before = shared.requests.sent();
    let paths_before = shared.requests.by_path();
    let refused_before = shared.requests.refused();
    let mut record = json!({
        "case": case.id,
        "attempt": attempt,
        "kind": match case.input { Input::Chat { .. } => "chat", Input::Burst { .. } => "extraction" },
        "input": input_json(case),
        "t0": label(world::t0()),
        "error": null,
    });
    let ran = run_in(case, shared, &mut record)
        .await
        .and_then(|()| check_persona(&record, &shared.persona));
    if let Err(error) = ran {
        record["setup_error"] = json!(error.starts_with(SETUP));
        record["error"] = json!(error);
    }
    let paths_after = shared.requests.by_path();
    let delta: BTreeMap<&String, usize> = paths_after
        .iter()
        .map(|(path, count)| (path, count - paths_before.get(path).copied().unwrap_or(0)))
        .filter(|(_, count)| *count > 0)
        .collect();
    record["requests"] = json!({
        "sent": shared.requests.sent() - sent_before,
        "by_path": delta,
        "refused_by_cap": shared.requests.refused() - refused_before,
        "run_total": shared.requests.sent(),
    });
    record["elapsed_ms"] = json!(started.elapsed().as_millis() as u64);
    record
}

fn input_json(case: &Case) -> Value {
    match &case.input {
        Input::Chat { asker, text } => json!({
            "channel": "C1",
            "asker": world::name_of(*asker),
            "content": format!("<@{BOT}> {text}"),
        }),
        Input::Burst {
            channel,
            lines,
            context,
        } => json!({
            "channel": if *channel == C1 { "C1" } else { "C2" },
            "context": context.iter().map(|line| json!({
                "author": world::name_of(line.author),
                "at": format!("{:02}:{:02}", line.at.0, line.at.1),
                "text": line.text,
                "processed": true,
            })).collect::<Vec<_>>(),
            "lines": lines.iter().map(|line| match line {
                Line::Post { author, at, text } => json!({
                    "post": format!("{:02}:{:02}", at.0, at.1),
                    "author": world::name_of(*author), "text": text,
                }),
                Line::Edit { of, at, text } => json!({
                    "edit_of": of, "at": format!("{:02}:{:02}", at.0, at.1), "text": text,
                }),
            }).collect::<Vec<_>>(),
        }),
    }
}

async fn run_in(case: &Case, shared: &Shared, record: &mut Value) -> Result<(), String> {
    let temp = TempDir::new(case.id).map_err(|e| format!("temp dir: {e}"))?;
    let config = config_in(&temp.0, &shared.mapping, &shared.persona)?;
    let policy = settings::seed(&config.seeds).schedule_policy(config.runtime.timezone);
    let context: Vec<ContextMessage> = match &case.input {
        Input::Burst { context, .. } => context
            .iter()
            .map(|line| ContextMessage {
                author: line.author,
                channel: C1,
                at: world::local(13, line.at.0, line.at.1),
                text: line.text,
            })
            .collect(),
        Input::Chat { .. } => Vec::new(),
    };
    harness::with_store(&config, async |store| {
        world::seed(store, &policy, &context).await?;
        select_persona(store, &shared.persona).await
    })
    .await
    .map_err(|e| e.to_string())??;

    let now = Arc::new(AtomicI64::new(world::t0().timestamp_micros()));
    let reading = Arc::clone(&now);
    let fake = Arc::new(FakeDiscord::new());
    fake.seed_members(Id::new(GUILD), world::guild_members());
    let (events, receiver) = mpsc::unbounded_channel();
    let wiring = Wiring {
        source: Script(receiver, false),
        transport: Arc::clone(&fake),
        clock: Arc::new(move || {
            DateTime::from_timestamp_micros(reading.load(Ordering::SeqCst)).unwrap_or_default()
        }),
        tick: TICK,
        extraction: TIMING,
    };
    let mut live = harness::start(&config, wiring, Some(Arc::clone(&shared.models)))
        .await
        .map_err(|e| format!("serve start: {e}"))?;
    let (stop, stopped) = oneshot::channel::<()>();
    let store = Arc::clone(&live.store);
    let drive = async {
        let outcome = script(case, &store, &fake, &events, &now, shared).await;
        let _ = stop.send(());
        outcome
    };
    let ((), outcome) = tokio::join!(
        live.discord.until(async {
            let _ = stopped.await;
        }),
        drive
    );
    let collected = match outcome {
        Ok(collected) => collected,
        Err(error) => {
            drop(store);
            live.close().await;
            return Err(error);
        }
    };
    drop(store);
    live.close().await;
    if let Value::Object(fields) = collected {
        for (key, value) in fields {
            record[key] = value;
        }
    }
    drop(temp);
    Ok(())
}

async fn wait_for<F: Future<Output = bool>>(limit: Duration, mut check: impl FnMut() -> F) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if check().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// No new Discord call for [`QUIET`] (cards and edits have landed).
async fn quiet(fake: &FakeDiscord) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = fake.calls().len();
    while Instant::now() < deadline {
        sleep(QUIET).await;
        let now = fake.calls().len();
        if now == seen {
            return;
        }
        seen = now;
    }
}

async fn script(
    case: &Case,
    store: &SqliteStore,
    fake: &FakeDiscord,
    events: &mpsc::UnboundedSender<Event>,
    now: &AtomicI64,
    shared: &Shared,
) -> Result<Value, String> {
    let send = |event: Event| {
        events
            .send(event)
            .map_err(|_| "the gateway stopped".to_owned())
    };
    send(world::ready())?;
    send(world::guild_create())?;
    // The startup rescan runs once the roster has reconciled and the listing
    // is in; waiting for it means chat and extraction are both up and the
    // rescan cannot claim the case's messages.
    let rescanned = wait_for(READY_WAIT, || async {
        fake.count(Op::ListMembers) > 0
            && store
                .recent_rescan_jobs(5)
                .await
                .is_ok_and(|jobs| !jobs.is_empty() && jobs.iter().all(|job| job.status.is_final()))
    })
    .await;
    sleep(TICK).await;
    let baseline = fake.calls().len();
    let rsvps_before = rsvps(store).await?;
    let runs_before = runs(store).await?;
    let completions_before = shared.requests.completions();
    let mut out = json!({ "startup_rescan_done": rescanned });

    match &case.input {
        Input::Chat { asker, text } => {
            let id = world::snowflake(world::t0(), 1);
            let content = format!("<@{BOT}> {text}");
            send(world::posted(world::message_json(
                id,
                C1,
                *asker,
                &content,
                world::t0(),
                None,
            )))?;
            let answered = wait_for(ANSWER_WAIT, || async {
                chat_row(store, id).await.is_some()
            })
            .await;
            quiet(fake).await;
            let row = chat_row(store, id).await;
            out["chat"] = json!({
                "logged": answered,
                "interaction": row.as_ref().map(chat_json),
                "tool_calls": row.as_ref().map(tool_calls).unwrap_or_default(),
            });
        }
        Input::Burst { channel, lines, .. } => {
            let mut posts: Vec<(u64, DateTime<Utc>)> = Vec::new();
            for (n, line) in lines.iter().enumerate() {
                let (event, id, text, at) = match line {
                    Line::Post { author, at, text } => {
                        let at = world::local(13, at.0, at.1);
                        let id = world::snowflake(at, n as u64);
                        posts.push((id, at));
                        let message = world::message_json(id, *channel, *author, text, at, None);
                        (world::posted(message), id, *text, at)
                    }
                    Line::Edit { of, at, text } => {
                        let (id, created) = posts[*of];
                        let at = world::local(13, at.0, at.1);
                        let author = match &lines[*of] {
                            Line::Post { author, .. } => *author,
                            Line::Edit { .. } => return Err("an edit of an edit".into()),
                        };
                        let message =
                            world::message_json(id, *channel, author, text, created, Some(at));
                        (world::edited(message), id, *text, at)
                    }
                };
                // Received live: the clock reads the line's time while the
                // handler takes it (an older message would be a replay).
                now.store(at.timestamp_micros(), Ordering::SeqCst);
                send(event)?;
                let cached = wait_for(Duration::from_secs(10), || async {
                    store
                        .messages_by_ids(&[id.to_string()])
                        .await
                        .is_ok_and(|rows| rows.first().is_some_and(|row| row.content == text))
                })
                .await;
                if !cached {
                    return Err(format!("line {n} was never cached"));
                }
            }
            // The burst flushes at T0 (the anchor).
            now.store(world::t0().timestamp_micros(), Ordering::SeqCst);
            let ids: Vec<String> = posts.iter().map(|(id, _)| id.to_string()).collect();
            // Gated bursts make no call: give the debounce its time, then wait
            // only if a completion went out.
            sleep(TIMING.debounce + Duration::from_secs(3)).await;
            let called = shared.requests.completions() > completions_before;
            let logged = !called
                || wait_for(ANSWER_WAIT, || async {
                    extraction_logs(store).await.iter().any(|log| {
                        log["message_ids"].as_array().is_some_and(|logged| {
                            logged.iter().any(|id| ids.iter().any(|want| want == id))
                        })
                    })
                })
                .await;
            sleep(TIMING.debounce).await;
            quiet(fake).await;
            let logs = extraction_logs(store).await;
            let amendments: Vec<Value> = logs
                .iter()
                .flat_map(|log| {
                    log["parsed"]["amendments"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
            out["extraction"] = json!({
                "called": called,
                "logged": logged,
                "message_ids": ids,
                "logs": logs,
                "amendments": amendments,
            });
        }
    }

    out["messages"] = json!(messages(fake, baseline));
    out["card_history"] = json!(card_history(fake, baseline));
    out["proposals"] = json!(proposals(store).await?);
    let rsvps_after = rsvps(store).await?;
    out["rsvp_changes"] = json!(rsvp_changes(&rsvps_before, &rsvps_after));
    out["runs_changed"] = json!(runs(store).await? != runs_before);
    Ok(out)
}
