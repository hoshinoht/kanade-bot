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
//! History rows are the real ones. [`board`] (`/__e2e/guides/board`) adds
//! the guides boards' four other guides and their states the same way.

use std::{
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use kanade::{
    api::state::ApiState,
    app::{clock::Clock, ports::GuidePort},
    bot::guides::{ChannelView, GuideChannels, Pauses, Publisher, PublisherParts},
    discord::transport::{
        AmbiguousKind, Call, FakeDiscord, Hold, Op, Outcome, RejectionKind, Step,
    },
    infrastructure::{files::GuideFiles, store::SqliteStore},
};
use kanade_domain::{
    guides::{
        ArtFile, ArtKind, Block, BossEntry, GuideChange, GuideGrants, GuideRecord, GuideSources,
        GuideStore, OpKind, OpOutcome, PlanCounts, Publish, PublishKind, PublishOp, PublishRequest,
        PublishRequested, PublishState, SlotState, plan, render,
    },
    history::{Actor, Surface},
};
use serde_json::{Value, json};
use tokio::{sync::watch, task::JoinHandle};
use twilight_model::{channel::message::MessageFlags, id::Id};

use crate::{
    Fail,
    fakes::guild::BOT_USER_ID,
    seed::{
        GUIDE_BOARD_CHANNELS, GuildChannels,
        ids::{ASAHI, GUIDE_BM, GUIDE_CARLING, GUIDE_KALOS, GUIDE_LIMBO, GUIDE_STAR, MIKA},
    },
};

type GuidePublisher = Publisher<SqliteStore, FakeDiscord>;

/// The generation's guild as the publisher sees it: every channel, with
/// what the bot may do there; nothing while Discord is "offline".
struct Channels {
    offline: Arc<AtomicBool>,
    guild: GuildChannels,
}

impl GuideChannels for Channels {
    fn view(&self, channel_id: &str) -> ChannelView {
        if self.offline.load(Ordering::SeqCst) {
            return ChannelView::Unknown;
        }
        match self.guild.find(channel_id) {
            Some(channel) => ChannelView::Granted(GuideGrants {
                view: true,
                send: channel.send,
                history: true,
                embed: channel.send,
                attach: channel.attach,
            }),
            None => ChannelView::Missing,
        }
    }
}

/// The publisher's art: the boss directory's, or while `earlier` is set
/// every file one byte longer, as art posted before it was replaced. The
/// guides board publishes one guide so, which the list then shows as "Out
/// of date" (art changed): the API renders the files as they are.
struct Sources {
    files: GuideFiles,
    earlier: AtomicBool,
}

impl GuideSources for Sources {
    fn lead(&self, key: &str) -> Option<String> {
        self.files.lead(key)
    }

    fn art(&self, kind: ArtKind, basename: &str) -> Option<ArtFile> {
        let file = self.files.art(kind, basename)?;
        if !self.earlier.load(Ordering::SeqCst) {
            return Some(file);
        }
        let mut bytes = file.bytes.to_vec();
        bytes.push(0);
        Some(ArtFile {
            bytes: bytes.into(),
            ..file
        })
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
    /// What the publisher renders with (the seed's plans use it too, so the
    /// hashes agree).
    sources: Arc<Sources>,
    discord: Arc<FakeDiscord>,
    offline: Arc<AtomicBool>,
    /// The generation's guild channels (the board adds its own).
    channels: GuildChannels,
    /// Creates parked by `{script: "hold"}`, released on `release` or close.
    holds: Mutex<Vec<Hold>>,
    worker: Option<(watch::Sender<bool>, JoinHandle<()>)>,
}

pub struct Parts<'a> {
    pub store: Arc<SqliteStore>,
    pub discord: Arc<FakeDiscord>,
    pub offline: Arc<AtomicBool>,
    pub channels: GuildChannels,
    pub state: &'a ApiState,
    pub boss_dir: Option<&'a Path>,
    pub knowledge_dir: Option<&'a Path>,
    pub clock: Clock,
    pub instance_id: String,
}

impl GuideSide {
    pub fn new(parts: Parts<'_>) -> Self {
        let sources = Arc::new(Sources {
            files: GuideFiles::new(parts.boss_dir, parts.knowledge_dir),
            earlier: AtomicBool::new(false),
        });
        let publisher = Arc::new(Publisher::new(PublisherParts {
            store: parts.store,
            transport: Arc::clone(&parts.discord),
            channels: Arc::new(Channels {
                offline: Arc::clone(&parts.offline),
                guild: parts.channels.clone(),
            }),
            catalog: Arc::clone(&parts.state.catalog),
            sources: Arc::clone(&sources) as Arc<dyn GuideSources + Send + Sync>,
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
            channels: parts.channels,
            holds: Mutex::default(),
            sources,
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
        create(
            store,
            id,
            title,
            channel,
            blocks,
            record(ago),
            record(ago - 10),
        )
        .await?;
    }
    // Publish Limbo party roles through the worker, as an admin's publish.
    request(
        side,
        store,
        state,
        LIMBO_GUIDE,
        "b7e4c2a9-1d3f-4a6b-8e5c-9f0a1b2c3d40",
        ASAHI,
        now - chrono::Duration::minutes(60),
    )
    .await?;
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

/// A guide write or publish request by the admin with Discord id `who`.
fn admin(who: &str, at: chrono::DateTime<chrono::Utc>) -> GuideRecord {
    GuideRecord {
        at,
        actor: Actor::admin(format!("discord:{who}")),
        surface: Surface::AdminPortal,
    }
}

/// One guide written as an admin writes it: created, then its first save.
async fn create(
    store: &SqliteStore,
    id: &str,
    title: &str,
    channel: Option<&str>,
    blocks: Vec<Block>,
    created: GuideRecord,
    saved: GuideRecord,
) -> Result<(), Fail> {
    let made = store
        .apply_guide(
            GuideChange::Create {
                id: id.into(),
                title: title.into(),
                channel_id: channel.map(str::to_owned),
            },
            created,
        )
        .await?;
    if !made.applied() {
        return Err(format!("creating guide {title}: {made:?}").into());
    }
    save(store, id, blocks, saved).await
}

/// Saves `blocks` over the guide's current version, title and channel kept.
async fn save(
    store: &SqliteStore,
    id: &str,
    blocks: Vec<Block>,
    record: GuideRecord,
) -> Result<(), Fail> {
    let guide = store
        .guide(id.into())
        .await?
        .ok_or_else(|| format!("no guide {id}"))?;
    let saved = store
        .apply_guide(
            GuideChange::Save {
                id: id.into(),
                version: guide.version,
                title: guide.title.clone(),
                channel_id: guide.channel_id.clone(),
                blocks,
            },
            record,
        )
        .await?;
    if !saved.applied() {
        return Err(format!("saving guide {}: {saved:?}", guide.title).into());
    }
    Ok(())
}

/// Queues run `run_id` publishing the guide's saved version, as admin `who`
/// requests it at `at`: planned as the admin's plan is (against its stored
/// slots, rendered with the publisher's sources so the worker's plan hash
/// agrees).
async fn request(
    side: &GuideSide,
    store: &SqliteStore,
    state: &ApiState,
    guide_id: &str,
    run_id: &str,
    who: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), Fail> {
    let guide = store
        .guide(guide_id.into())
        .await?
        .ok_or_else(|| format!("no guide {guide_id}"))?;
    let channel = guide
        .channel_id
        .clone()
        .ok_or_else(|| format!("{} has no channel", guide.title))?;
    let slots = store.slots(guide.id.clone()).await?;
    let catalog = Arc::clone(&state.catalog);
    let sources = Arc::clone(&side.sources);
    let blocks = guide.blocks.clone();
    let rendered = tokio::task::spawn_blocking(move || render(&blocks, &catalog, &*sources))
        .await?
        .map_err(|errors| format!("{} does not render: {errors:?}", guide.title))?;
    let planned = plan(&slots, &rendered.slots, &channel, true, None);
    let run = Publish {
        id: run_id.into(),
        guide_id: guide.id.clone(),
        version: guide.version,
        plan_hash: planned.hash.clone(),
        channel_id: channel,
        kind: PublishKind::Publish,
        confirmed: Default::default(),
        delete_old: true,
        state: PublishState::Queued,
        stop_reason: None,
        requested_by: format!("admin:discord:{who}"),
        requested_at: at,
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
            admin(who, at),
        )
        .await?
    {
        PublishRequested::Queued(_) => Ok(()),
        other => Err(format!("queueing a publish of {}: {other:?}", guide.title).into()),
    }
}

/// Waits (up to 15 s) until run `run_id` satisfies `there`; the run as it is then.
async fn until(
    store: &SqliteStore,
    run_id: &str,
    there: impl Fn(&Publish, &[PublishOp]) -> bool,
) -> Result<Publish, Fail> {
    for _ in 0..300 {
        if let Some((run, ops)) = store.publish(run_id.into()).await?
            && there(&run, &ops)
        {
            return Ok(run);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("guides board: run {run_id} did not get there within 15 s").into())
}

/// Requests a publish from the live worker and waits for it to end as `state`.
async fn publish_to_end(
    side: &GuideSide,
    store: &SqliteStore,
    state: &ApiState,
    guide_id: &str,
    who: &str,
    at: chrono::DateTime<chrono::Utc>,
    ends: PublishState,
) -> Result<(), Fail> {
    let run_id = uuid::Uuid::new_v4().to_string();
    request(side, store, state, guide_id, &run_id, who, at).await?;
    side.publisher.wake();
    let ended = until(store, &run_id, |run, _| !run.live()).await?;
    if ended.state != ends {
        return Err(format!("guides board: {guide_id} ended {ended:?}").into());
    }
    Ok(())
}

pub const KALOS_GUIDE: &str = "5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e01";
pub const STAR_GUIDE: &str = "5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e02";
pub const CARLING_GUIDE: &str = "5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e03";
pub const BM_GUIDE: &str = "5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e04";

const KALOS_INTRO: &str = "# Kalos basics\nEverything we expect before a **Chaos Kalos** clear. Ask in #kalos-four if anything here is unclear.";
const KALOS_BEFORE: &str = "## Before you join
- **Level 265 or higher**, with your symbols at the entry line. Below that, ask an organiser first.
- Bring **your own potions**: a stack of power elixirs and a few reboots. The party does not share.
- Check your familiars and buffs in town. Buffing at the door holds the whole party.
- Read the guardian order in the Kalos notes before your first try. It never changes between runs.
- Be in voice five minutes before the start time. Late joiners wait for the next attempt.

## During the run
- The caller is whoever leads the party that night. Follow their calls, even when you know a faster line.
- Burst only on the caller's count. A burst into the shield wastes your cooldowns for the next window.
- If you die, say so once in voice and wait. Respawns are called; nobody re-enters alone.
- Keep voice for calls. Jokes and stories after the clear, please.
- Swap to your backup gear only between attempts, never inside the map.

## Afterwards
- Drops are split as posted in #kalos-four the day before. Disputes go to an organiser in a DM, never in the party chat.
- Post your clear time in #kalos-four if you want it tracked.
- Can't make next week? Answer **No** on the reminder card as early as you can so someone else can take the slot.

## Common mistakes
- Standing on the purple floor after the second guardian falls.
- Using the long cooldown before the gatekeeper's armour breaks.
- Leaving the map to change gear mid-run: the door locks and the party loses a slot.";
const KALOS_NOTE: &str =
    "Chaos needs two Sharp Eyes users or a dedicated buffer. Organisers check this the day before.";

/// A block id of the guides board (`e2e0b0a7-…-<n>`).
fn block_id(n: u32) -> String {
    format!("e2e0b0a7-6b1d-4c2e-8f3a-{n:012x}")
}

fn text(n: u32, content: &str) -> Block {
    Block::Text {
        id: block_id(n),
        content: content.into(),
    }
}

fn bosses(n: u32, entries: &[(&str, Option<&str>, bool)]) -> Block {
    Block::Bosses {
        id: block_id(n),
        heading: Some("## Entry".into()),
        entries: entries
            .iter()
            .map(|&(key, note, large_image)| BossEntry {
                key: key.into(),
                note: note.map(str::to_owned),
                large_image,
            })
            .collect(),
    }
}

fn spacer(n: u32) -> Block {
    Block::Spacer { id: block_id(n) }
}

/// `POST /__e2e/guides/board`: the guides boards' list
/// (`docs/research/2026-10-16-guide-mockups`, `G_List`) over the seed's two
/// guides, through the store and the live worker. Adds `#guide-kalos`,
/// `#guide-star`, `#guide-carling` and `#guide-bm` (no Attach Files), then:
///
/// - Kalos basics: published to `#guide-kalos` (5 messages).
/// - Radiant Malefic Star: published to `#guide-star` (5 messages) with art
///   the boss directory has since replaced: Out of date (art changed).
/// - Black Mage prep: its publish to `#guide-bm` refused for the missing
///   Attach Files: Blocked.
/// - Carling + Star night: 3 messages posted, then version 3's message 4
///   timed out unposted (`ambiguous_create`): Needs attention.
/// - Limbo party roles: version 3 (7 blocks) publishing, 2 of its 4 changes
///   made and the next post held (`hold`; `/__e2e/discord/guide {script:
///   "release"}` lets it finish).
/// - Weekly bossing etiquette: the seed's draft with no channel.
pub async fn board(side: &GuideSide, store: &SqliteStore, state: &ApiState) -> Result<(), Fail> {
    side.channels.add(&GUIDE_BOARD_CHANNELS);
    let now = (state.clock)();
    let ago = |minutes: i64| now - chrono::Duration::minutes(minutes);

    let kalos = vec![
        text(1, KALOS_INTRO),
        text(2, KALOS_BEFORE),
        bosses(3, &[("Kalos", Some(KALOS_NOTE), true), ("FA", None, false)]),
        spacer(4),
        text(5, "-# Kept by the officers. Questions go to #kalos-four."),
    ];
    create(
        store,
        KALOS_GUIDE,
        "Kalos basics",
        Some(GUIDE_KALOS),
        kalos,
        admin(ASAHI, ago(2_000)),
        admin(ASAHI, ago(1_990)),
    )
    .await?;
    publish_to_end(
        side,
        store,
        state,
        KALOS_GUIDE,
        ASAHI,
        ago(900),
        PublishState::Done,
    )
    .await?;

    let star = vec![
        text(
            11,
            "# Radiant Malefic Star\nHard mode only. Read the ring rules before your first night.",
        ),
        text(
            12,
            "## Rings\n- The ring that dims is safe; the bright ring burns.",
        ),
        bosses(
            13,
            &[(
                "MaleficStar",
                Some("Hard needs 6 members over Lv 280."),
                false,
            )],
        ),
        text(
            14,
            "## Phase two\n- Stack on the caller when the star falls.",
        ),
        text(15, "-# Kept by the officers."),
    ];
    create(
        store,
        STAR_GUIDE,
        "Radiant Malefic Star",
        Some(GUIDE_STAR),
        star,
        admin(MIKA, ago(3_000)),
        admin(MIKA, ago(2_990)),
    )
    .await?;
    side.sources.earlier.store(true, Ordering::SeqCst);
    let posted = publish_to_end(
        side,
        store,
        state,
        STAR_GUIDE,
        MIKA,
        ago(2_400),
        PublishState::Done,
    )
    .await;
    side.sources.earlier.store(false, Ordering::SeqCst);
    posted?;

    let bm = vec![
        text(
            21,
            "# Black Mage prep\nWhat to bring before the first Hard attempt.",
        ),
        bosses(22, &[("BM", None, true)]),
        text(
            23,
            "## Phases\n- Four phases; the caller names the safe side.",
        ),
        text(24, "-# Kept by the officers."),
    ];
    create(
        store,
        BM_GUIDE,
        "Black Mage prep",
        Some(GUIDE_BM),
        bm,
        admin(MIKA, ago(900)),
        admin(MIKA, ago(890)),
    )
    .await?;
    publish_to_end(
        side,
        store,
        state,
        BM_GUIDE,
        MIKA,
        ago(880),
        PublishState::Stopped,
    )
    .await?;

    let carling = |all: bool| {
        let mut blocks = vec![
            text(
                31,
                "# Carling + Star night\nTuesday 22:00, both bosses back to back.",
            ),
            bosses(32, &[("Carling", None, true), ("MaleficStar", None, false)]),
            text(33, "## Karling\n- Spread damage over all three bodies."),
        ];
        if all {
            blocks.extend([
                text(34, "## Radiant Malefic Star\n- Stand in the dimming ring."),
                spacer(35),
                text(36, "-# Kept by the officers."),
            ]);
        }
        blocks
    };
    create(
        store,
        CARLING_GUIDE,
        "Carling + Star night",
        Some(GUIDE_CARLING),
        carling(false),
        admin(ASAHI, ago(200)),
        admin(ASAHI, ago(190)),
    )
    .await?;
    publish_to_end(
        side,
        store,
        state,
        CARLING_GUIDE,
        ASAHI,
        ago(180),
        PublishState::Done,
    )
    .await?;
    save(store, CARLING_GUIDE, carling(true), admin(ASAHI, ago(22))).await?;
    side.script("ambiguous_create");
    publish_to_end(
        side,
        store,
        state,
        CARLING_GUIDE,
        ASAHI,
        ago(20),
        PublishState::Stopped,
    )
    .await?;

    // Limbo party roles grows to the boards' seven messages: two edits, then
    // two posts, the first held so the run stays live.
    let limbo = vec![
        text(
            41,
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
        text(43, "## Light side\n- Two bishops and the caller."),
        text(44, "## Shade side\n- Everyone else, closest to the gate."),
        spacer(45),
        text(46, "## Swaps\n- The caller counts down from three."),
        text(47, "-# Kept by the officers."),
    ];
    save(store, LIMBO_GUIDE, limbo, admin(ASAHI, ago(3))).await?;
    side.script("hold");
    let run_id = uuid::Uuid::new_v4().to_string();
    request(side, store, state, LIMBO_GUIDE, &run_id, ASAHI, ago(1)).await?;
    side.publisher.wake();
    until(store, &run_id, |_, ops| {
        ops.iter()
            .any(|op| op.op.kind() == OpKind::Create && op.outcome == OpOutcome::Pending)
    })
    .await?;
    Ok(())
}
