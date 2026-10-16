//! The guide publisher (module `guides`) over the real store and journal and
//! the fake Discord: plans run in order, only as confirmed, silently, and
//! an unconfirmed create is never sent twice. Synthetic guides only:
//! invented text, an invented catalog and in-memory art.

mod publish;
mod reconcile;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use tokio::sync::watch;
use twilight_model::channel::Message;
use twilight_model::id::Id;

use kanade::bot::guides::{ChannelView, GuideChannels, Pauses, Publisher, PublisherParts};
use kanade::bot::transport::{Call, FakeDiscord, Op};
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec, GuideSpec};
use kanade::domain::guides::{
    ArtFile, ArtKind, Block, BossEntry, Confirmations, GuideChange, GuideGrants, GuideRecord,
    GuideSources, GuideStore, GuideWrite, Plan, Publish, PublishKind, PublishOp, PublishRequest,
    PublishRequested, PublishState, PublishedSlot, RenderedSlot, plan, plan_unpublish, render,
};
use kanade::domain::history::{Actor, Surface};
use kanade::infrastructure::store::SqliteStore;

pub const GUIDE: &str = "00000000-0000-4000-8000-00000000aaaa";
pub const CHANNEL: &str = "200000000000000001";
pub const OLD: &str = "200000000000000002";
pub const SELF_ID: u64 = 900_000_000_000_000_001;
const STRANGER: u64 = 900_000_000_000_000_002;

pub fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap()
}

fn block_id(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

pub fn text(n: usize, content: &str) -> Block {
    Block::Text {
        id: block_id(n),
        content: content.to_owned(),
    }
}

pub fn spacer(n: usize) -> Block {
    Block::Spacer { id: block_id(n) }
}

/// A bosses block with `MaleficStar` (mixed-case art key); `large_image`
/// adds its entry art, which changes the message's files.
pub fn bosses(n: usize, note: Option<&str>, large_image: bool) -> Block {
    Block::Bosses {
        id: block_id(n),
        heading: Some("Invented heading".into()),
        entries: vec![BossEntry {
            key: "MaleficStar".into(),
            note: note.map(str::to_owned),
            large_image,
        }],
    }
}

fn catalog() -> BossTable {
    let difficulty = |prefix: &str, label: &str| DifficultySpec {
        prefix: prefix.into(),
        label: label.into(),
    };
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![difficulty("h", "Hard"), difficulty("x", "Extreme")],
        bosses: vec![BossSpec {
            short: "MaleficStar".into(),
            full: Some("Invented Malefic Star".into()),
            level: Some(280),
            difficulties: Some(vec!["h".into(), "x".into()]),
            guide: Some(GuideSpec {
                colour: Some(0x39BFFF),
            }),
            ..BossSpec::default()
        }],
    })
    .unwrap()
}

/// In-memory art, matched exactly as named.
struct Art;

impl GuideSources for Art {
    fn lead(&self, _key: &str) -> Option<String> {
        None
    }

    fn art(&self, kind: ArtKind, basename: &str) -> Option<ArtFile> {
        (basename == "MaleficStar").then(|| ArtFile {
            file_name: format!("{basename}.png"),
            bytes: Arc::from(format!("invented {}", kind.as_str()).into_bytes()),
        })
    }
}

/// The guild cache's answer per channel; unlisted channels are missing.
#[derive(Default)]
pub struct Channels(Mutex<BTreeMap<String, GuideGrants>>);

pub const ALL: GuideGrants = GuideGrants {
    view: true,
    send: true,
    history: true,
    embed: true,
    attach: true,
};

impl Channels {
    pub fn set(&self, channel: &str, grants: GuideGrants) {
        self.0.lock().unwrap().insert(channel.into(), grants);
    }
}

impl GuideChannels for Channels {
    fn view(&self, channel_id: &str) -> ChannelView {
        self.0
            .lock()
            .unwrap()
            .get(channel_id)
            .map_or(ChannelView::Missing, |grants| ChannelView::Granted(*grants))
    }
}

pub type Worker = Publisher<SqliteStore, FakeDiscord>;

pub struct Rig {
    pub store: Arc<SqliteStore>,
    pub fake: Arc<FakeDiscord>,
    pub channels: Arc<Channels>,
    pub publisher: Arc<Worker>,
    pub stop: watch::Sender<bool>,
    runs: Mutex<u32>,
}

fn record() -> GuideRecord {
    GuideRecord {
        at: at(),
        actor: Actor::admin("token"),
        surface: Surface::AdminPortal,
    }
}

impl Rig {
    pub async fn new() -> Self {
        Self::on(Arc::new(SqliteStore::open_temp().await)).await
    }

    /// A worker over `store` (a "restart" keeps the store, not the worker).
    pub async fn on(store: Arc<SqliteStore>) -> Self {
        let fake = Arc::new(FakeDiscord::new());
        let channels = Arc::new(Channels::default());
        channels.set(CHANNEL, ALL);
        channels.set(OLD, ALL);
        Self::with(store, fake, channels)
    }

    pub fn with(store: Arc<SqliteStore>, fake: Arc<FakeDiscord>, channels: Arc<Channels>) -> Self {
        let publisher = Arc::new(Publisher::new(PublisherParts {
            store: Arc::clone(&store),
            transport: Arc::clone(&fake),
            channels: Arc::clone(&channels) as Arc<dyn GuideChannels>,
            catalog: Arc::new(catalog()),
            sources: Arc::new(Art),
            self_id: Arc::new(|| Some(Id::new(SELF_ID))),
            clock: Arc::new(at),
            instance_id: "guides-test".into(),
            pauses: Pauses {
                search: Duration::ZERO,
                retry: Duration::ZERO,
            },
        }));
        Self {
            store,
            fake,
            channels,
            publisher,
            stop: watch::Sender::new(false),
            runs: Mutex::new(0),
        }
    }

    /// The guide in `channel` showing `blocks` (created on first use).
    pub async fn save(&self, channel: &str, blocks: &[Block]) {
        let current = self.store.guide(GUIDE.into()).await.unwrap();
        let version = match current {
            Some(guide) => guide.version,
            None => {
                let change = GuideChange::Create {
                    id: GUIDE.into(),
                    title: "Invented guide".into(),
                    channel_id: Some(channel.into()),
                };
                match self.store.apply_guide(change, record()).await.unwrap() {
                    GuideWrite::Created(guide) => guide.version,
                    other => panic!("not created: {other:?}"),
                }
            }
        };
        let change = GuideChange::Save {
            id: GUIDE.into(),
            version,
            title: "Invented guide".into(),
            channel_id: Some(channel.into()),
            blocks: blocks.to_vec(),
        };
        let written = self.store.apply_guide(change, record()).await.unwrap();
        assert!(matches!(written, GuideWrite::Saved(_)), "{written:?}");
    }

    /// The saved guide's messages, as the API preview renders them.
    pub async fn desired(&self) -> Vec<RenderedSlot> {
        let guide = self.store.guide(GUIDE.into()).await.unwrap().unwrap();
        render(&guide.blocks, &catalog(), &Art).unwrap().slots
    }

    pub async fn slots(&self) -> Vec<PublishedSlot> {
        self.store.slots(GUIDE.into()).await.unwrap()
    }

    /// The plan the admin previews (checked through the port, as the API).
    pub async fn plan(&self, delete_old: bool) -> Plan {
        let guide = self.store.guide(GUIDE.into()).await.unwrap().unwrap();
        let channel = guide.channel_id.unwrap();
        let slots = self.slots().await;
        let inspection = self.publisher.inspect_channel(&channel, &slots).await;
        plan(
            &slots,
            &self.desired().await,
            &channel,
            delete_old,
            inspection.as_ref(),
        )
    }

    /// Queue a run of `kind` confirming `confirmed` for `plan_hash`.
    pub async fn request(
        &self,
        kind: PublishKind,
        plan_hash: &str,
        confirmed: Confirmations,
        delete_old: bool,
    ) -> Publish {
        let guide = self.store.guide(GUIDE.into()).await.unwrap().unwrap();
        let id = {
            let mut runs = self.runs.lock().unwrap();
            *runs += 1;
            format!("run-{runs}")
        };
        let publish = Publish {
            id,
            guide_id: GUIDE.into(),
            version: guide.version,
            plan_hash: plan_hash.into(),
            channel_id: guide.channel_id.unwrap(),
            kind,
            confirmed,
            delete_old,
            state: PublishState::Queued,
            stop_reason: None,
            requested_by: "admin:token".into(),
            requested_at: at(),
            started_at: None,
            finished_at: None,
            cancel_requested_at: None,
        };
        let request = PublishRequest {
            publish,
            counts: Default::default(),
        };
        match self.store.request_publish(request, record()).await.unwrap() {
            PublishRequested::Queued(queued) => queued,
            other => panic!("not queued: {other:?}"),
        }
    }

    /// Queue the current plan, confirming what it needs.
    pub async fn queue(&self, delete_old: bool) -> Publish {
        let plan = self.plan(delete_old).await;
        self.request(PublishKind::Publish, &plan.hash, plan.needs, delete_old)
            .await
    }

    pub async fn queue_unpublish(&self) -> Publish {
        let plan = plan_unpublish(&self.slots().await, CHANNEL);
        self.request(
            PublishKind::Unpublish,
            &plan.hash,
            Confirmations::default(),
            false,
        )
        .await
    }

    /// Run the next queued run to its end.
    pub async fn run(&self) -> Publish {
        let stop = self.stop.subscribe();
        self.publisher.run_next(&stop).await.expect("a queued run")
    }

    /// Queue the current plan and run it.
    pub async fn publish(&self) -> Publish {
        self.queue(true).await;
        self.run().await
    }

    pub async fn ops(&self, run: &Publish) -> Vec<PublishOp> {
        self.store.publish(run.id.clone()).await.unwrap().unwrap().1
    }

    /// Discord effects only (creates, edits, deletes), in call order.
    pub fn effects(&self) -> Vec<Call> {
        self.fake
            .calls()
            .into_iter()
            .filter(|call| matches!(call.op(), Op::Create | Op::Edit | Op::Delete))
            .collect()
    }

    pub fn effect_count(&self) -> usize {
        self.effects().len()
    }
}

/// Short labels of effects: `create:<content>`, `edit:<id>`, `delete:<id>`.
pub fn labels(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .map(|call| match call {
            Call::Create {
                channel, message, ..
            } => format!(
                "create:{channel}:{}",
                message.content.as_deref().unwrap_or_default()
            ),
            Call::Edit { message, .. } => format!("edit:{message}"),
            Call::Delete {
                channel, message, ..
            } => format!("delete:{channel}:{message}"),
            other => format!("{:?}", other.op()),
        })
        .collect()
}

/// A message in the guide channel showing `slot`, by the bot (or not).
pub fn shown(id: u64, slot: &RenderedSlot, by_bot: bool) -> Message {
    let author = if by_bot { SELF_ID } else { STRANGER };
    let embeds: Vec<_> = slot
        .embeds
        .iter()
        .map(|embed| {
            json!({
                "type": "rich",
                "title": embed.title,
                "description": embed.description,
                "color": embed.colour,
                "thumbnail": embed.thumbnail.as_ref().map(|name| json!({
                    "url": format!("https://cdn.invalid/attachments/1/2/{name}?ex=1"),
                })),
            })
        })
        .collect();
    serde_json::from_value(json!({
        "id": id.to_string(),
        "channel_id": CHANNEL,
        "author": {
            "id": author.to_string(),
            "username": "kanade",
            "discriminator": "0",
            "avatar": null,
            "bot": by_bot,
        },
        // Discord may trim and re-space what it stores.
        "content": format!(" {} ", slot.content),
        "timestamp": "2026-10-16T12:00:00.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": embeds,
        "pinned": false,
        "type": 0,
    }))
    .expect("message fixture")
}
