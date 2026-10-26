//! Stores and fixtures shared by the delivery tests.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use kanade::bot::delivery::{FixedClock, IdsRef};
use kanade::discord::transport::FakeDiscord;
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use kanade_domain::ids::IdGenerator;
use kanade_domain::notify::{
    CardIndex, Claim, DeliveryJournal, DeliveryTarget, DigestInclusion, EffectKind, IntentContent,
    Lease, NotificationIntent, Receipt,
};
use kanade_domain::schedule::ScheduleSnapshot;
use kanade_domain::scheduler::{ScheduleStore, SchedulerService, Scope, StoreRef};
use twilight_model::channel::message::Component;

/// v4's first stub message id.
pub const FIRST_MESSAGE_ID: u64 = 700_000_000_000_000_001;
pub const INSTANCE: &str = "delivery-test";

/// Everything the tick needs from a store.
pub trait Store:
    ScheduleStore
    + DeliveryJournal
    + CardIndex
    + kanade_domain::notify::NoticeOutbox
    + kanade_domain::notify::DeclineNoticeStore
    + kanade_domain::history::Checkpoints
    + kanade_domain::drafts::ProposalStore
    + kanade_domain::notify::ReminderCardStore
    + kanade_domain::notify::DigestPhraseStore
    + kanade_domain::ownership::OwnerRequestStore
    + kanade_domain::completion::RunPromptStore
    + Sync
{
}

impl<S> Store for S where
    S: ScheduleStore
        + DeliveryJournal
        + CardIndex
        + kanade_domain::notify::NoticeOutbox
        + kanade_domain::notify::DeclineNoticeStore
        + kanade_domain::history::Checkpoints
        + kanade_domain::drafts::ProposalStore
        + kanade_domain::notify::ReminderCardStore
        + kanade_domain::notify::DigestPhraseStore
        + kanade_domain::ownership::OwnerRequestStore
        + kanade_domain::completion::RunPromptStore
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

/// A send that timed out, after Discord did (`applied`) or did not apply it.
pub fn timed_out(applied: bool) -> kanade::discord::transport::Step {
    kanade::discord::transport::Step::Ambiguous {
        kind: kanade::discord::transport::AmbiguousKind::Timeout,
        applied,
    }
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

/// Raise the digest marker to `week` at `now`, under a test lease.
pub async fn mark_digest_week<S: Store>(store: &S, week: DateTime<Utc>, now: DateTime<Utc>) {
    with_lease(store, now, async |lease| {
        store
            .record_digest_week(lease, week, now)
            .await
            .expect("digest marker");
    })
    .await;
}

/// The stored digest phrase for the boss week starting at `week`.
pub async fn week_phrase(
    store: &impl kanade_domain::notify::DigestPhraseStore,
    week: DateTime<Utc>,
) -> Option<String> {
    let key = kanade_domain::notify::DedupeKey::native(&[DeliveryTarget::Digest(week)])
        .expect("digest key");
    store.digest_phrase(key.as_str()).await.expect("phrase")
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

/// Every text display of a Components V2 layout, in order (sections and
/// containers opened).
pub fn v2_texts(components: &[Component]) -> Vec<String> {
    let mut out = Vec::new();
    for component in components {
        match component {
            Component::TextDisplay(text) => out.push(text.content.clone()),
            Component::Container(container) => out.extend(v2_texts(&container.components)),
            Component::Section(section) => out.extend(v2_texts(&section.components)),
            _ => {}
        }
    }
    out
}

/// A V2 digest's phrase: the line under its `## Boss week` title.
pub fn v2_digest_phrase(components: &[Component]) -> Option<String> {
    v2_texts(components)
        .first()?
        .lines()
        .nth(1)
        .map(str::to_owned)
}
