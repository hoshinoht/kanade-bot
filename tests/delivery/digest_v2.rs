//! The weekly digest as Components V2: a legacy digest converted in place
//! on its first refresh, a V2 digest left as posted once the week outgrows
//! the V2 budget (Discord cannot take the flag off), sent as the embed when
//! the week is over the budget, and linking the portal only while it has a
//! public origin and is open.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::TimeDelta;
use kanade::bot::delivery::cards::CardKit;
use kanade::bot::delivery::cards::redesign::{MY_RUNS, OPEN_PORTAL};
use kanade::bot::transport::{Call, FakeDiscord, MessageId, Op, Outcome, RejectionKind};
use kanade::domain::schedule::RunStatus;
use kanade::infrastructure::store::SqliteStore;
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::ButtonStyle;

use crate::cards::{answer, created, edits, kit, refresher, run, tonight, world};
use crate::scenarios::{self, HOME, now, previous_week};
use crate::support::{self, mark_digest_week};

struct Week {
    store: Arc<SqliteStore>,
    fake: Arc<FakeDiscord>,
    world: scenarios::World,
    runs: Vec<String>,
}

impl Week {
    /// `count` Kalos runs tonight, a minute apart, and last week's digest
    /// marked done so this week's is due.
    async fn new(count: i64) -> Self {
        let store = Arc::new(SqliteStore::open_temp().await);
        let mut runs = Vec::new();
        for n in 0..count {
            let at = tonight() + TimeDelta::minutes(n);
            runs.push(run(&*store, &["XKalos"], &["1001"], at, RunStatus::Planned).await);
        }
        mark_digest_week(&*store, previous_week(), now()).await;
        Self {
            store,
            fake: Arc::new(support::fake()),
            world: world(),
            runs,
        }
    }

    /// Post this week's digest with `cards`; its message id.
    async fn post(&self, cards: &CardKit) -> MessageId {
        let mut delivery =
            scenarios::delivery(&*self.store, &self.world, &*self.fake).with_cards(cards.clone());
        delivery.post_week_digest(now()).await.expect("digest");
        self.fake
            .calls()
            .into_iter()
            .find_map(|call| match call {
                Call::Create {
                    outcome: Outcome::Delivered(id),
                    ..
                } => Some(id),
                _ => None,
            })
            .expect("the digest was posted")
    }

    /// An answer, then a refresh with `cards`: how many posts it edited.
    async fn refresh(&self, cards: &CardKit) -> usize {
        answer(
            &*self.store,
            &self.runs[0],
            "1001",
            true,
            now() + TimeDelta::minutes(1),
        )
        .await;
        refresher(&self.store, &self.fake, &self.world, cards.clone())
            .refresh(&self.runs[..1])
            .await
    }
}

#[tokio::test]
async fn a_legacy_digest_turns_v2_on_its_first_refresh() {
    let week = Week::new(1).await;
    let digest = MessageId::new(7001);
    week.fake
        .seed_message(twilight_model::id::Id::new(HOME.parse().unwrap()), digest);
    support::seed_digest(&*week.store, scenarios::week(), HOME, "7001", now()).await;
    assert!(!week.fake.is_v2(digest));

    assert_eq!(week.refresh(&kit(None)).await, 1);
    let [edit] = edits(&week.fake).try_into().expect("one edit");
    let layout = edit.components.as_deref().expect("a V2 edit");
    let texts = support::v2_texts(layout);
    assert!(texts[0].starts_with("## Boss week · "), "{texts:?}");
    assert!(texts[1].contains("· 1 in"), "{texts:?}");
    assert_eq!((edit.content, edit.embeds), (None, None));
    assert!(week.fake.is_v2(digest), "converted in place");
    assert!(created(&week.fake).is_empty(), "no new post");
}

#[tokio::test]
async fn a_v2_digest_is_left_as_posted_once_the_week_outgrows_the_budget() {
    let week = Week::new(1).await;
    let cards = kit(None);
    let digest = week.post(&cards).await;
    assert!(week.fake.is_v2(digest));
    // Sixty runs on one day: the render falls back to the embed.
    for n in 1..60 {
        let at = tonight() + TimeDelta::minutes(n);
        run(&*week.store, &["XKalos"], &["1001"], at, RunStatus::Planned).await;
    }

    // This process posted it: no edit is even tried.
    assert_eq!(week.refresh(&cards).await, 0);
    assert!(edits(&week.fake).is_empty());
    assert_eq!(week.fake.count(Op::Flags), 0);

    // A restarted process does not know: Discord refuses the embed edit,
    // one flags read tells why, and later refreshes leave it alone.
    let restarted = kit(None);
    assert_eq!(week.refresh(&restarted).await, 0);
    let [refused] = week
        .fake
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Edit { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("one edit tried");
    assert_eq!(
        refused,
        Outcome::DefinitelyRejected(RejectionKind::Http {
            status: 400,
            code: Some(50035)
        })
    );
    assert_eq!(week.fake.count(Op::Flags), 1);
    assert_eq!(week.refresh(&restarted).await, 0);
    assert_eq!(week.fake.count(Op::Edit), 1, "not tried again");
    assert_eq!(week.fake.count(Op::Flags), 1);
    assert!(week.fake.is_v2(digest), "still the V2 post");
}

#[tokio::test]
async fn a_week_over_the_v2_budget_posts_the_embed() {
    // Sixty runs on one day: that day's text would pass Discord's 2,000
    // bytes per text display.
    let week = Week::new(60).await;
    let cards = kit(None);
    let digest = week.post(&cards).await;
    let [post] = created(&week.fake).try_into().expect("one post");
    assert!(post.components.is_empty(), "no V2 layout");
    assert!(
        post.content
            .as_deref()
            .is_some_and(|text| text.starts_with("🗓️ **")),
        "{:?}",
        post.content
    );
    assert_eq!(post.embeds.len(), 1);
    assert!(!week.fake.is_v2(digest));

    // Its refresh stays an embed edit.
    assert_eq!(week.refresh(&cards).await, 1);
    let [edit] = edits(&week.fake).try_into().expect("one edit");
    assert_eq!(edit.components, None);
    assert!(edit.embeds.is_some());
}

/// Every button of a V2 layout as (label, link URL), in order.
fn buttons(components: &[Component]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for component in components {
        match component {
            Component::Container(container) => out.extend(buttons(&container.components)),
            Component::ActionRow(row) => out.extend(buttons(&row.components)),
            Component::Button(button) => out.push((
                button.label.clone().unwrap_or_default(),
                button
                    .url
                    .clone()
                    .filter(|_| button.style == ButtonStyle::Link),
            )),
            _ => {}
        }
    }
    out
}

/// `kit(None)` with a public portal origin, the portal open while `open`
/// is set, read live.
fn with_portal(open: &Arc<AtomicBool>) -> CardKit {
    let open = Arc::clone(open);
    let mut cards = kit(None);
    cards.v2.portal = Some(PORTAL.into());
    cards.v2.portal_open = Some(Arc::new(move || open.load(Ordering::SeqCst)));
    cards
}

const PORTAL: &str = "https://kanade.example";

#[tokio::test]
async fn the_portal_button_follows_the_live_portal_switch() {
    let week = Week::new(1).await;
    let open = Arc::new(AtomicBool::new(false));
    let cards = with_portal(&open);

    // Closed: the tunnel is stopped, so no dead link; "My runs" stays.
    week.post(&cards).await;
    let [post] = created(&week.fake).try_into().expect("one post");
    assert_eq!(buttons(&post.components), [(MY_RUNS.to_owned(), None)]);

    // Opened: the posted digest's next refresh links the portal.
    open.store(true, Ordering::SeqCst);
    assert_eq!(week.refresh(&cards).await, 1);
    let [edit] = edits(&week.fake).try_into().expect("one edit");
    assert_eq!(
        buttons(edit.components.as_deref().expect("a V2 edit")),
        [
            (MY_RUNS.to_owned(), None),
            (OPEN_PORTAL.to_owned(), Some(PORTAL.to_owned()))
        ]
    );
}

#[tokio::test]
async fn without_a_public_origin_there_is_no_portal_button() {
    let week = Week::new(1).await;
    let mut cards = with_portal(&Arc::new(AtomicBool::new(true)));
    cards.v2.portal = None;
    week.post(&cards).await;
    let [post] = created(&week.fake).try_into().expect("one post");
    assert_eq!(buttons(&post.components), [(MY_RUNS.to_owned(), None)]);
}
