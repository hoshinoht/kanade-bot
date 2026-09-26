//! Stores, fixtures and effect records shared by the delivery tests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use kanade::bot::delivery::{
    DEFAULT_MAX_SENDS_PER_TICK, DeliveryConfig, FixedClock, IdsRef, SendOutcome, SendReport,
    StoreRef,
};
use kanade::bot::events::CardIndex;
use kanade::bot::transport::FakeDiscord;
use kanade::domain::ids::{IdGenerator, short_id};
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, DeliveryWarning, DigestInclusion, EffectKind,
    IntentContent, Lease, NotificationIntent, Receipt,
};
use kanade::domain::schedule::{ReminderPolicy, SchedulePolicy, ScheduleSnapshot};
use kanade::domain::scheduler::{ScheduleStore, SchedulerService, Scope};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use serde_json::{Value, json};

use crate::common::{clock_time, countdowns, strings, text, weekday, zone};

/// v4's first stub message id.
pub const FIRST_MESSAGE_ID: u64 = 700_000_000_000_000_001;
pub const INSTANCE: &str = "delivery-test";

/// Everything the tick needs from a store.
pub trait Store:
    ScheduleStore
    + DeliveryJournal
    + CardIndex
    + kanade::domain::notify::NoticeOutbox
    + kanade::domain::history::Checkpoints
    + kanade::domain::drafts::ProposalStore
    + kanade::bot::delivery::cards::ReminderCardStore
    + Sync
{
}

impl<S> Store for S where
    S: ScheduleStore
        + DeliveryJournal
        + CardIndex
        + kanade::domain::notify::NoticeOutbox
        + kanade::domain::history::Checkpoints
        + kanade::domain::drafts::ProposalStore
        + kanade::bot::delivery::cards::ReminderCardStore
        + Sync
{
}

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        // The store requires canonical, private (0700) directories.
        use std::os::unix::fs::DirBuilderExt;
        let root = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let path = root.join(format!("kanade-delivery-{}", uuid::Uuid::new_v4()));
        for dir in [path.clone(), path.join("locks")] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .expect("temp dir");
        }
        Self(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }

    pub fn config(&self) -> SqliteStoreConfig {
        SqliteStoreConfig {
            db_path: self.0.join("kanade.sqlite3"),
            owner_lock_dir: self.0.join("locks"),
        }
    }

    pub async fn open(&self) -> SqliteStore {
        SqliteStore::open(&self.config()).await.expect("open store")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `$f(&store, args..)` against a fresh memory store, then a fresh
/// SQLite store in a temp directory.
macro_rules! on_both_stores {
    ($f:path $(, $arg:expr)* $(,)?) => {{
        let memory = kanade::infrastructure::store::MemoryScheduleStore::new();
        $f(&memory $(, $arg)*).await;
        let dir = $crate::support::TempDir::new();
        let sqlite = dir.open().await;
        $f(&sqlite $(, $arg)*).await;
        sqlite.close().await.expect("close store");
    }};
}
pub(crate) use on_both_stores;

pub fn policy(input: &Value) -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: zone(input),
            ping_time: clock_time(&input["ping_time"]),
            countdowns: countdowns(&input["countdowns"]),
        },
        weekday(&input["reset_weekday"]),
        clock_time(&input["reset_time"]),
    )
}

pub fn config(input: &Value) -> DeliveryConfig {
    DeliveryConfig {
        instance_id: INSTANCE.into(),
        policy: policy(input),
        post_channel_id: input["post_channel_id"].as_str().map(str::to_owned),
        quiet_mode: false,
        max_sends_per_tick: DEFAULT_MAX_SENDS_PER_TICK,
        max_notice_age: kanade::domain::notify::DEFAULT_MAX_NOTICE_AGE,
    }
}

pub fn roster(input: &Value) -> Roster {
    let mut roster = Roster::new();
    for member in input["members"].as_array().expect("members") {
        roster.upsert(Member {
            user_id: text(&member["user_id"]).to_owned(),
            display_name: member["display_name"].as_str().map(str::to_owned),
            nickname: member["nickname"].as_str().map(str::to_owned),
            has_role: member["has_role"].as_bool().unwrap_or_default(),
            is_bot: false,
            ping_level: PingLevel::parse_stored(text(&member["ping_level"])).expect("level"),
        });
    }
    roster
}

pub fn channels(input: &Value) -> BTreeSet<String> {
    strings(&input["available_channel_ids"])
        .into_iter()
        .collect()
}

pub fn fake() -> FakeDiscord {
    FakeDiscord::with_first_message_id(FIRST_MESSAGE_ID)
}

pub async fn snapshot(store: &impl ScheduleStore) -> ScheduleSnapshot {
    store.load(&Scope::All).await.expect("load")
}

/// A scheduler service over `store` at `now`.
pub fn service<'a, S: ScheduleStore + Sync, I: IdGenerator>(
    store: &'a S,
    ids: &'a mut I,
    now: DateTime<Utc>,
) -> SchedulerService<StoreRef<'a, S>, IdsRef<'a, I>, FixedClock> {
    SchedulerService::new(StoreRef(store), IdsRef(ids), FixedClock(now))
}

/// Run `work` under a fresh test lease.
pub async fn with_lease<S: Store, R>(
    store: &S,
    now: DateTime<Utc>,
    work: impl AsyncFnOnce(&Lease) -> R,
) -> R {
    let lease = store
        .begin_lease(INSTANCE, "test_setup", now)
        .await
        .expect("lease");
    let value = work(&lease).await;
    store.end_lease(&lease, now).await.expect("end lease");
    value
}

/// A digest intent for `week` in `channel` (for seeding cards).
pub fn digest_intent(week: DateTime<Utc>, channel: &str) -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Digest,
        effect_context: Vec::new(),
        channel_id: channel.into(),
        targets: vec![DeliveryTarget::Digest(week)],
        mentions: Vec::new(),
        content: IntentContent::Digest {
            week_start: week,
            inclusion: DigestInclusion::default(),
        },
        warnings: Vec::new(),
    }
}

/// v4 `Repo.set_weekly_digest`, through the journal: claim and bind a card
/// for `week` without raising the digest marker.
pub async fn seed_digest<S: Store>(
    store: &S,
    week: DateTime<Utc>,
    channel: &str,
    message: &str,
    now: DateTime<Utc>,
) {
    with_lease(store, now, async |lease| {
        let Claim::Fresh(attempt) = store
            .claim(lease, &digest_intent(week, channel), None, now)
            .await
            .expect("claim")
        else {
            panic!("seeded digest is held");
        };
        let receipt = Receipt {
            channel_id: channel.into(),
            message_id: message.into(),
        };
        store
            .bind(lease, &attempt, &receipt, None, now)
            .await
            .expect("bind");
    })
    .await;
}

fn label(outcome: &SendOutcome) -> &'static str {
    match outcome {
        SendOutcome::Bound(_) => "bound",
        SendOutcome::Uncertain => "raised:DeliveryUncertainError",
        SendOutcome::Suppressed => "suppressed",
        other => panic!("no v4 label for {other:?}"),
    }
}

/// A send as the v4 replayer records a `SendPlan`.
pub fn effect_json(report: &SendReport) -> Value {
    let intent = &report.intent;
    let mut value = json!({
        "effect_kind": intent.effect.as_str(),
        "channel_id": intent.channel_id,
        "dedupe_scope": "native",
        "targets": intent.targets.iter().map(|target| json!({
            "binding_type": target.binding_type(),
            "key_primary": target.key_primary().expect("in range"),
            "key_secondary": null,
        })).collect::<Vec<_>>(),
        "mentions": intent.mentions,
        "role_mentions": [],
        "mention_everyone": false,
        "outcome": label(&report.outcome),
    });
    if let IntentContent::Digest { inclusion, .. } = &intent.content {
        value["digest"] = json!({
            "days": inclusion.days.iter().map(|day| json!({
                "runs": day.run_ids.iter().map(|id| short_id(id)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "cleared": inclusion.cleared,
            "live": inclusion.live,
            "unsettled": inclusion.unsettled,
            "at_risk": inclusion.at_risk,
        });
    }
    if !intent.warnings.is_empty() {
        value["warnings"] = intent
            .warnings
            .iter()
            .map(|warning| {
                let DeliveryWarning::HomeChannelUnavailable {
                    home_channel_id,
                    run_ids,
                } = warning;
                json!({
                    "kind": "home_channel_unavailable",
                    "home_channel_id": home_channel_id,
                    "run_ids": run_ids,
                })
            })
            .collect();
    }
    value
}

/// Register `"{fixed_key}@{week_text}"` for every timing materialised that week.
pub async fn register_materialised(
    store: &impl ScheduleStore,
    fixed_refs: &BTreeMap<String, String>,
    week: DateTime<Utc>,
    week_text: &str,
    run_refs: &mut BTreeMap<String, String>,
) {
    let state = snapshot(store).await;
    for (key, fixed_id) in fixed_refs {
        if let Some(run) = state
            .runs
            .iter()
            .find(|run| run.fixed_run_id.as_ref() == Some(fixed_id) && run.week_start == week)
        {
            run_refs.insert(format!("{key}@{week_text}"), run.id.clone());
        }
    }
}
