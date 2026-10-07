//! The redesigned weekly digest as Components V2: converted in place on the
//! first refresh after the style flips to redesigned, left as posted once
//! the style flips back (Discord cannot take the flag off), and sent as the
//! embed when the week is over the V2 budget.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::TimeDelta;
use kanade::bot::delivery::cards::CardKit;
use kanade::bot::transport::{Call, FakeDiscord, MessageId, Op, Outcome, RejectionKind};
use kanade::domain::notify::DeliveryJournal;
use kanade::domain::schedule::RunStatus;
use kanade::domain::settings::MessageStyle;
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::cards::{answer, created, edits, kit, refresher, run, tonight, world};
use crate::scenarios::{self, now, previous_week};
use crate::support::{self, with_lease};

/// `kit(None)` in the redesigned style while `redesign` is set, read live.
fn switchable(redesign: &Arc<AtomicBool>) -> CardKit {
    let redesign = Arc::clone(redesign);
    CardKit {
        style: Some(Arc::new(move || {
            if redesign.load(Ordering::SeqCst) {
                MessageStyle::Redesigned
            } else {
                MessageStyle::Classic
            }
        })),
        ..kit(None)
    }
}

struct Week {
    store: Arc<MemoryScheduleStore>,
    fake: Arc<FakeDiscord>,
    world: scenarios::World,
    runs: Vec<String>,
}

impl Week {
    /// `count` Kalos runs tonight, a minute apart, and last week's digest
    /// marked done so this week's is due.
    async fn new(count: i64) -> Self {
        let store = Arc::new(MemoryScheduleStore::new());
        let mut runs = Vec::new();
        for n in 0..count {
            let at = tonight() + TimeDelta::minutes(n);
            runs.push(run(&*store, &["XKalos"], &["1001"], at, RunStatus::Planned).await);
        }
        with_lease(&*store, now(), async |lease| {
            store
                .record_digest_week(lease, previous_week(), now())
                .await
                .expect("digest marker");
        })
        .await;
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
async fn a_classic_digest_turns_v2_on_the_first_refresh_after_the_switch() {
    let week = Week::new(1).await;
    let style = Arc::new(AtomicBool::new(false));
    let cards = switchable(&style);
    let digest = week.post(&cards).await;
    let [post] = created(&week.fake).try_into().expect("one post");
    assert!(post.components.is_empty(), "classic is an embed");
    assert!(!week.fake.is_v2(digest));

    style.store(true, Ordering::SeqCst);
    assert_eq!(week.refresh(&cards).await, 1);
    let [edit] = edits(&week.fake).try_into().expect("one edit");
    let layout = edit.components.as_deref().expect("a V2 edit");
    let texts = support::v2_texts(layout);
    assert!(texts[0].starts_with("## Boss week · "), "{texts:?}");
    assert!(texts[1].contains("· 1 in"), "{texts:?}");
    assert_eq!((edit.content, edit.embeds), (None, None));
    assert!(week.fake.is_v2(digest), "converted in place");
    assert_eq!(created(&week.fake).len(), 1, "no new post");
}

#[tokio::test]
async fn a_v2_digest_is_left_as_posted_after_the_style_flips_back() {
    let week = Week::new(1).await;
    let style = Arc::new(AtomicBool::new(true));
    let cards = switchable(&style);
    let digest = week.post(&cards).await;
    assert!(week.fake.is_v2(digest));

    // This process posted it: no edit is even tried.
    style.store(false, Ordering::SeqCst);
    assert_eq!(week.refresh(&cards).await, 0);
    assert!(edits(&week.fake).is_empty());
    assert_eq!(week.fake.count(Op::Flags), 0);

    // A restarted process does not know: Discord refuses the embed edit,
    // one flags read tells why, and later refreshes leave it alone.
    let restarted = switchable(&style);
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
async fn a_week_over_the_v2_budget_posts_the_redesigned_embed() {
    // Sixty runs on one day: that day's text would pass Discord's 2,000
    // bytes per text display.
    let week = Week::new(60).await;
    let cards = switchable(&Arc::new(AtomicBool::new(true)));
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
