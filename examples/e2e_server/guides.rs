//! Module `guides` over the fake Discord: serve's real publisher
//! ([`Publisher`], as `runtime::serve::discord` builds it) with the
//! generation's [`FakeDiscord`] as its transport and the seeded guild as its
//! channel view, ready at once (the e2e gateway never sends `READY`, so the
//! serve-side publisher, which waits for it, is left off). Each generation's
//! fake starts holding every message its store says is posted, so edits and
//! deletes of seeded slots land, and mints ids above them.
//!
//! [`seed`] writes the template's guides through the store and publishes
//! the first one with the worker itself, so its slots, op log, journal and
//! History rows are the real ones.

use std::{
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use kanade::{
    api::{
        auth::Clock,
        state::{ApiState, GuidePort},
    },
    bot::{
        guides::{ChannelView, GuideChannels, Pauses, Publisher, PublisherParts},
        transport::{AmbiguousKind, Call, FakeDiscord, Hold, Op, Outcome, RejectionKind, Step},
    },
    domain::{
        guides::{
            Block, BossEntry, GuideChange, GuideGrants, GuideRecord, GuideStore, PlanCounts,
            Publish, PublishKind, PublishRequest, PublishRequested, PublishState, SlotState, plan,
            render,
        },
        history::{Actor, Surface},
    },
    infrastructure::{files::GuideFiles, store::SqliteStore},
};
use serde_json::{Value, json};
use tokio::{sync::watch, task::JoinHandle};
use twilight_model::{channel::message::MessageFlags, id::Id};

use crate::{
    Fail,
    fakes::guild::BOT_USER_ID,
    seed::{
        CHANNELS,
        ids::{ASAHI, GUIDE_LIMBO},
    },
};

type GuidePublisher = Publisher<SqliteStore, FakeDiscord>;

/// The seeded guild as the publisher sees it: every seeded channel, with
/// what the bot may do there; nothing while Discord is "offline".
struct Channels {
    offline: Arc<AtomicBool>,
}

impl GuideChannels for Channels {
    fn view(&self, channel_id: &str) -> ChannelView {
        if self.offline.load(Ordering::SeqCst) {
            return ChannelView::Unknown;
        }
        match CHANNELS.iter().find(|channel| channel.id == channel_id) {
            Some(channel) => ChannelView::Granted(GuideGrants {
                view: true,
                send: channel.send,
                history: true,
                embed: channel.send,
                attach: channel.send,
            }),
            None => ChannelView::Missing,
        }
    }
}

/// The fake Discord for a generation over `store`: holding every posted
/// guide message, minting ids above the highest.
pub async fn discord(store: &SqliteStore) -> Result<Arc<FakeDiscord>, Fail> {
    let mut posted = Vec::new();
    for guide in store.guides().await? {
        for slot in store.slots(guide.id).await? {
            if slot.state != SlotState::Posted {
                continue;
            }
            let (Some(channel), Some(message)) = (
                slot.channel_id
                    .parse::<u64>()
                    .ok()
                    .and_then(Id::new_checked),
                slot.message_id
                    .as_deref()
                    .and_then(|id| id.parse::<u64>().ok())
                    .and_then(Id::new_checked),
            ) else {
                continue;
            };
            posted.push((channel, message));
        }
    }
    let fake = match posted.iter().map(|(_, message)| message.get()).max() {
        Some(highest) => FakeDiscord::with_first_message_id(highest + 1),
        None => FakeDiscord::new(),
    };
    for (channel, message) in posted {
        fake.seed_message(channel, message);
    }
    Ok(Arc::new(fake))
}

/// One generation's guide publisher, its worker and the scripted calls.
pub struct GuideSide {
    publisher: Arc<GuidePublisher>,
    /// What renders the seed's plan (the publisher's own adapter).
    files: GuideFiles,
    discord: Arc<FakeDiscord>,
    offline: Arc<AtomicBool>,
    /// Creates parked by `{script: "hold"}`, released on `release` or close.
    holds: Mutex<Vec<Hold>>,
    worker: Option<(watch::Sender<bool>, JoinHandle<()>)>,
}

pub struct Parts<'a> {
    pub store: Arc<SqliteStore>,
    pub discord: Arc<FakeDiscord>,
    pub offline: Arc<AtomicBool>,
    pub state: &'a ApiState,
    pub boss_dir: Option<&'a Path>,
    pub knowledge_dir: Option<&'a Path>,
    pub clock: Clock,
    pub instance_id: String,
}

impl GuideSide {
    pub fn new(parts: Parts<'_>) -> Self {
        let files = GuideFiles::new(parts.boss_dir, parts.knowledge_dir);
        let publisher = Arc::new(Publisher::new(PublisherParts {
            store: parts.store,
            transport: Arc::clone(&parts.discord),
            channels: Arc::new(Channels {
                offline: Arc::clone(&parts.offline),
            }),
            catalog: Arc::clone(&parts.state.catalog),
            sources: Arc::new(files.clone()),
            self_id: Arc::new(|| BOT_USER_ID.parse::<u64>().ok().and_then(Id::new_checked)),
            clock: parts.clock,
            instance_id: parts.instance_id,
            // Short, so an unconfirmed post's two searches end within a test.
            pauses: Pauses {
                search: Duration::from_millis(200),
                retry: Duration::from_millis(200),
            },
        }));
        Self {
            publisher,
            discord: parts.discord,
            offline: parts.offline,
            holds: Mutex::default(),
            files,
            worker: None,
        }
    }

    pub fn port(&self) -> Arc<dyn GuidePort> {
        Arc::clone(&self.publisher) as Arc<dyn GuidePort>
    }

    /// Start the worker (the gateway counts as ready at once).
    pub fn spawn(&mut self) {
        let (ready_tx, ready) = watch::channel(true);
        let (stop, stopped) = watch::channel(false);
        let publisher = Arc::clone(&self.publisher);
        let task = tokio::spawn(async move {
            // The sender lives as long as the worker, so `ready` stays true.
            let _ready = ready_tx;
            publisher.run(ready, stopped).await;
        });
        self.worker = Some((stop, task));
    }

    /// Stop the worker between ops (parked creates are let go first).
    pub async fn stop(mut self) {
        self.release();
        if let Some((stop, task)) = self.worker.take() {
            let _ = stop.send(true);
            if let Err(error) = task.await {
                eprintln!("e2e_server: the guide worker failed: {error}");
            }
        }
    }

    fn release(&self) {
        for hold in self
            .holds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            hold.release();
        }
    }

    /// `POST /__e2e/discord/guide {script}`; `false` for an unknown script.
    pub fn script(&self, script: &str) -> bool {
        match script {
            // The next post times out without landing: the search finds
            // nothing and the slot is left unknown.
            "ambiguous_create" => self.discord.script(
                Op::Create,
                Step::Ambiguous {
                    kind: AmbiguousKind::Timeout,
                    applied: false,
                },
            ),
            // The next edit finds its message deleted by hand.
            "edit_404" => self
                .discord
                .script(Op::Edit, Step::Reject(RejectionKind::UnknownMessage)),
            "offline" => self.offline.store(true, Ordering::SeqCst),
            "online" => self.offline.store(false, Ordering::SeqCst),
            // Park the next post until `release`, so a run can be seen running.
            "hold" => self
                .holds
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(self.discord.hold(Op::Create)),
            "release" => self.release(),
            _ => return false,
        }
        true
    }

    /// `GET /__e2e/discord/calls`: every call the fake Discord answered, in
    /// order, with what it answered.
    pub fn calls(&self) -> Value {
        let flags = self.discord.create_flags();
        let mut creates = 0;
        let calls: Vec<Value> = self
            .discord
            .calls()
            .iter()
            .map(|call| match call {
                Call::Create {
                    channel,
                    message,
                    outcome,
                } => {
                    let silent = flags
                        .get(creates)
                        .is_some_and(|flags| flags.contains(MessageFlags::SUPPRESS_NOTIFICATIONS));
                    creates += 1;
                    json!({
                        "op": "create",
                        "channel": channel.to_string(),
                        "message": delivered_id(outcome),
                        "outcome": outcome_word(outcome),
                        "silent": silent,
                        "embeds": message.embeds.len(),
                        "files": message.attachments.len(),
                    })
                }
                Call::Edit {
                    channel,
                    message,
                    outcome,
                    ..
                } => json!({
                    "op": "edit",
                    "channel": channel.to_string(),
                    "message": message.to_string(),
                    "outcome": outcome_word(outcome),
                }),
                Call::Delete {
                    channel,
                    message,
                    outcome,
                } => json!({
                    "op": "delete",
                    "channel": channel.to_string(),
                    "message": message.to_string(),
                    "outcome": outcome_word(outcome),
                }),
                other => json!({ "op": format!("{:?}", other.op()).to_lowercase() }),
            })
            .collect();
        Value::Array(calls)
    }
}

fn delivered_id<T: ToString>(outcome: &Outcome<T>) -> Option<String> {
    match outcome {
        Outcome::Delivered(id) => Some(id.to_string()),
        _ => None,
    }
}

fn outcome_word<T>(outcome: &Outcome<T>) -> String {
    match outcome {
        Outcome::Delivered(_) => "delivered".into(),
        Outcome::DefinitelyRejected(kind) => format!("rejected: {kind:?}"),
        Outcome::Ambiguous(kind) => format!("ambiguous: {kind:?}"),
    }
}

/// The published seed guide (specs open it as `/guides?item=<id>`).
pub const LIMBO_GUIDE: &str = "3d0c5b8a-7e21-4f3a-b6d9-2a4c8e1f5a10";

/// Invented guides: "Limbo party roles", published to `#guide-limbo` (five
/// messages), and "Weekly bossing etiquette", a draft with no channel.
pub async fn seed(side: &GuideSide, store: &SqliteStore, state: &ApiState) -> Result<(), Fail> {
    let now = (state.clock)();
    let record = |minutes: i64| GuideRecord {
        at: now - chrono::Duration::minutes(minutes),
        actor: Actor::admin(format!("discord:{ASAHI}")),
        surface: Surface::AdminPortal,
    };
    let text = |id: &str, content: &str| Block::Text {
        id: id.into(),
        content: content.into(),
    };
    let limbo = vec![
        text(
            "6c1f4a2e-0b51-4d0e-9a51-3f7d2c8e1a01",
            "# Limbo party roles\nWho stands where, and who calls the swaps.",
        ),
        Block::Bosses {
            id: "6c1f4a2e-0b51-4d0e-9a51-3f7d2c8e1a02".into(),
            heading: Some("## Entry".into()),
            entries: vec![BossEntry {
                key: "Limbo".into(),
                note: Some("Bring two bishops; the caller stands at the gate.".into()),
                large_image: true,
            }],
        },
        text(
            "6c1f4a2e-0b51-4d0e-9a51-3f7d2c8e1a03",
            "## Light side\n- Two bishops and the caller.",
        ),
        Block::Spacer {
            id: "6c1f4a2e-0b51-4d0e-9a51-3f7d2c8e1a04".into(),
        },
        text(
            "6c1f4a2e-0b51-4d0e-9a51-3f7d2c8e1a05",
            "-# Kept by the officers.",
        ),
    ];
    let etiquette = vec![
        text(
            "0e9b7d3c-5a14-4b6f-8c2d-71e0f4a9b301",
            "# Weekly bossing etiquette\nBe on time, and say so in the party channel if you can't make it.",
        ),
        text(
            "0e9b7d3c-5a14-4b6f-8c2d-71e0f4a9b302",
            "## Clears\n- Post your clear time in #boss-schedule.",
        ),
    ];
    let guides = [
        (
            LIMBO_GUIDE,
            "Limbo party roles",
            Some(GUIDE_LIMBO),
            limbo,
            180,
        ),
        (
            "8a2e6f1d-4c3b-4e5a-9f70-1b2d3c4e5f20",
            "Weekly bossing etiquette",
            None,
            etiquette,
            120,
        ),
    ];
    for (id, title, channel, blocks, ago) in guides {
        let created = store
            .apply_guide(
                GuideChange::Create {
                    id: id.into(),
                    title: title.into(),
                    channel_id: channel.map(str::to_owned),
                },
                record(ago),
            )
            .await?;
        if !created.applied() {
            return Err(format!("seeding guide {title}: {created:?}").into());
        }
        let saved = store
            .apply_guide(
                GuideChange::Save {
                    id: id.into(),
                    version: 1,
                    title: title.into(),
                    channel_id: channel.map(str::to_owned),
                    blocks,
                },
                record(ago - 10),
            )
            .await?;
        if !saved.applied() {
            return Err(format!("saving guide {title}: {saved:?}").into());
        }
    }
    // Publish Limbo party roles through the worker, as an admin's publish.
    let guide = store
        .guide(LIMBO_GUIDE.into())
        .await?
        .ok_or("the seeded guide is gone")?;
    let catalog = Arc::clone(&state.catalog);
    let sources = side.files.clone();
    let blocks = guide.blocks.clone();
    let rendered = tokio::task::spawn_blocking(move || render(&blocks, &catalog, &sources))
        .await?
        .map_err(|errors| format!("the seeded guide does not render: {errors:?}"))?;
    let planned = plan(&[], &rendered.slots, GUIDE_LIMBO, true, None);
    let run = Publish {
        id: "b7e4c2a9-1d3f-4a6b-8e5c-9f0a1b2c3d40".into(),
        guide_id: guide.id.clone(),
        version: guide.version,
        plan_hash: planned.hash.clone(),
        channel_id: GUIDE_LIMBO.into(),
        kind: PublishKind::Publish,
        confirmed: Default::default(),
        delete_old: true,
        state: PublishState::Queued,
        stop_reason: None,
        requested_by: format!("admin:discord:{ASAHI}"),
        requested_at: now - chrono::Duration::minutes(60),
        started_at: None,
        finished_at: None,
        cancel_requested_at: None,
    };
    let counts: PlanCounts = planned.counts;
    match store
        .request_publish(
            PublishRequest {
                publish: run,
                counts,
            },
            record(60),
        )
        .await?
    {
        PublishRequested::Queued(_) => {}
        other => return Err(format!("queueing the seeded publish: {other:?}").into()),
    }
    let (_stop, stopped) = watch::channel(false);
    let ended = side
        .publisher
        .run_next(&stopped)
        .await
        .ok_or("the seeded publish was not claimed")?;
    if ended.state != PublishState::Done {
        return Err(format!("the seeded publish ended {ended:?}").into());
    }
    Ok(())
}
