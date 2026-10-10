//! Manual header rewrites (`ManualRewrite`): every header posted this boss
//! week is rewritten and its post edited in place (no new post, no ping); a
//! rejected or failed rewrite stores and edits nothing; a later ordinary
//! refresh keeps the override; one run at a time.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::TimeDelta;
use kanade::bot::delivery::cards::{
    CardKit, HeaderHistory, HeaderOverrideStore, HeadingRewrite, ReminderCardStore,
};
use kanade::bot::delivery::{CardRefresh, ManualReport, ManualRequest, ManualRewrite, ManualStart};
use kanade::bot::transport::{FakeDiscord, MessageEdit, Op};
use kanade::chat::nudge::{
    NudgeRewriter, RewriteFailure, RewritePrompt, SharedRewriter, StoreRewriteSink,
};
use kanade::domain::model_log::{RewriteFilter, RewriteLogStore, RewriteStage};
use kanade::domain::notify::DeliveryJournal;
use kanade::domain::settings::MessageStyle;
use kanade::infrastructure::store::MemoryScheduleStore;
use tokio::sync::{Notify, watch};

use crate::cards::{answer, created, edits, kit, persona, seed_day_of, world};
use crate::scenarios::{self, HOME, World, now, previous_week};
use crate::support::{self, with_lease};

const DAY_OF_SEED: &str = "Today — {day}";

/// Answers by seed: the day-of line or the phrase; the first call may wait
/// for `release`.
struct BySeed {
    day_of: Result<&'static str, RewriteFailure>,
    phrase: &'static str,
    calls: AtomicUsize,
    gated: bool,
    started: Notify,
    release: Notify,
}

impl BySeed {
    fn new(day_of: Result<&'static str, RewriteFailure>, phrase: &'static str) -> Arc<Self> {
        Arc::new(Self {
            day_of,
            phrase,
            calls: AtomicUsize::new(0),
            gated: false,
            started: Notify::new(),
            release: Notify::new(),
        })
    }

    fn gated(day_of: &'static str) -> Arc<Self> {
        Arc::new(Self {
            gated: true,
            ..Arc::into_inner(Self::new(Ok(day_of), "Waku waku!")).unwrap()
        })
    }
}

impl NudgeRewriter for BySeed {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        _deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 && self.gated {
            self.started.notify_one();
            self.release.notified().await;
        }
        if prompt.seed() == DAY_OF_SEED {
            self.day_of.map(str::to_owned)
        } else {
            Ok(self.phrase.to_owned())
        }
    }
}

/// A day-of card and this week's digest, posted with the seeds in `style`.
struct Posted {
    store: Arc<MemoryScheduleStore>,
    logs: Arc<MemoryScheduleStore>,
    fake: Arc<FakeDiscord>,
    world: World,
    star: String,
    style: MessageStyle,
}

impl Posted {
    async fn new(style: MessageStyle) -> Self {
        let store = Arc::new(MemoryScheduleStore::new());
        let world = world();
        let fake = Arc::new(support::fake());
        let (star, _) = seed_day_of(&*store).await;
        with_lease(&*store, now(), async |lease| {
            store
                .record_digest_week(lease, previous_week(), now())
                .await
                .expect("digest week");
        })
        .await;
        let seeds = CardKit {
            style: Some(Arc::new(move || style)),
            ..kit(None)
        };
        let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(seeds);
        delivery.post_week_digest(now()).await.expect("digest");
        delivery.dispatch_reminders(now()).await.expect("dispatch");
        assert_eq!(created(&fake).len(), 2, "digest and morning card");
        Self {
            store,
            logs: Arc::new(MemoryScheduleStore::new()),
            fake,
            world,
            star,
            style,
        }
    }

    fn refresh(&self, rewriter: &Arc<BySeed>) -> CardRefresh<MemoryScheduleStore, FakeDiscord> {
        let style = self.style;
        CardRefresh {
            store: Arc::clone(&self.store),
            transport: Arc::clone(&self.fake),
            members: Arc::new(self.world.roster.clone()),
            cards: CardKit {
                heading: HeadingRewrite {
                    rewriter: Some(SharedRewriter(rewriter.clone())),
                    persona: Some(persona()),
                    words: None,
                    log: Some(Arc::new(StoreRewriteSink::new(
                        Arc::clone(&self.logs),
                        Arc::new(now),
                    ))),
                },
                style: Some(Arc::new(move || style)),
                ..kit(None)
            },
            policy: scenarios::config().policy,
            quiet: Arc::new(AtomicBool::new(false)),
            now: Arc::new(|| now() + TimeDelta::minutes(1)),
        }
    }

    fn manual(
        &self,
        rewriter: &Arc<BySeed>,
    ) -> Arc<ManualRewrite<MemoryScheduleStore, FakeDiscord>> {
        Arc::new(ManualRewrite::new(Arc::new(self.refresh(rewriter))))
    }

    async fn card_key(&self) -> String {
        let posted = self.store.posted_cards(&self.star).await.expect("posted");
        posted[0].dedupe_key.clone().expect("a real card")
    }

    async fn stages(&self) -> Vec<RewriteStage> {
        self.logs
            .list_rewrites(&RewriteFilter {
                limit: 100,
                ..RewriteFilter::default()
            })
            .await
            .expect("rewrites")
            .items
            .into_iter()
            .map(|row| row.stage)
            .collect()
    }
}

fn request() -> ManualRequest {
    ManualRequest {
        actor: "member:1004".into(),
        report_to: Some(HOME.into()),
    }
}

/// A V2 digest edit's phrase (the line under its title); `None` for any
/// other edit.
fn digest_phrase(edit: &MessageEdit) -> Option<String> {
    support::v2_digest_phrase(edit.components.as_deref()?)
}

/// Start the worker, trigger one run and wait for its report.
async fn run_once(
    manual: &Arc<ManualRewrite<MemoryScheduleStore, FakeDiscord>>,
) -> (ManualStart, ManualReport) {
    let (stop, stopped) = watch::channel(false);
    let worker = {
        let manual = Arc::clone(manual);
        tokio::spawn(async move { manual.run(stopped).await })
    };
    let mut finished = manual.finished();
    let started = manual.start(request()).await;
    tokio::time::timeout(Duration::from_secs(5), finished.changed())
        .await
        .expect("the run ends")
        .expect("open");
    let report = finished.borrow().expect("a report");
    stop.send_replace(true);
    worker.await.expect("no panic");
    (started, report)
}

#[tokio::test]
async fn a_run_edits_the_posted_card_and_digest_in_place_and_a_refresh_keeps_it() {
    let posted = Posted::new(MessageStyle::Redesigned).await;
    let rewriter = BySeed::new(Ok("Rise and shine, it's {day}!"), "Waku waku!");
    let manual = posted.manual(&rewriter);
    let (started, report) = run_once(&manual).await;
    assert_eq!(started, ManualStart::Started(2), "the card and the digest");
    assert_eq!(
        report,
        ManualReport {
            accepted: 2,
            ..ManualReport::default()
        }
    );
    let heading = "📅 **Rise and shine, it's Thu 10 Sep!**";
    let edited = edits(&posted.fake);
    assert_eq!(edited.len(), 2, "{edited:?}");
    let card = edited[0].content.as_deref().unwrap_or_default();
    assert!(card.starts_with(heading), "{edited:?}");
    // The redesigned digest pings nobody: it is edited as Components V2.
    assert_eq!(digest_phrase(&edited[1]).as_deref(), Some("Waku waku!"));
    for edit in edits(&posted.fake) {
        assert_eq!(
            serde_json::to_value(&edit.allowed_mentions).unwrap(),
            serde_json::json!({ "parse": [] }),
            "an edit notifies nobody"
        );
    }
    let posts = created(&posted.fake);
    assert_eq!(posts.len(), 3, "nothing is posted again but the summary");
    assert_eq!(
        posts[2].content.as_deref(),
        Some("✏️ Rewrote this boss week's headers: 2 accepted, 0 rejected, 0 failed.")
    );
    assert!(posts[2].attachments.is_empty());

    let key = posted.card_key().await;
    assert_eq!(
        posted.store.header_history(&key).await.expect("history"),
        HeaderHistory {
            original: Some("Today — Thu 10 Sep".into()),
            overrides: vec![(
                "Rise and shine, it's Thu 10 Sep!".into(),
                "member:1004".into()
            )],
        }
    );
    assert_eq!(
        posted.stages().await,
        [RewriteStage::Manual, RewriteStage::Manual],
        "every call is logged once as manual"
    );

    // An ordinary refresh after an answer keeps the new lines.
    answer(
        &*posted.store,
        &posted.star,
        "1002",
        true,
        now() + TimeDelta::minutes(1),
    )
    .await;
    let refresh = posted.refresh(&rewriter);
    assert_eq!(refresh.refresh(std::slice::from_ref(&posted.star)).await, 2);
    let latest = edits(&posted.fake).split_off(2);
    let card = latest[0].content.as_deref().unwrap_or_default();
    assert!(card.starts_with(heading), "{latest:?}");
    assert_eq!(digest_phrase(&latest[1]).as_deref(), Some("Waku waku!"));
    assert_eq!(
        rewriter.calls.load(Ordering::SeqCst),
        2,
        "refresh never rewrites"
    );
}

#[tokio::test]
async fn the_classic_style_rewrites_only_day_of_headings() {
    let posted = Posted::new(MessageStyle::Classic).await;
    let rewriter = BySeed::new(Ok("Rise and shine, it's {day}!"), "Waku waku!");
    let (started, report) = run_once(&posted.manual(&rewriter)).await;
    assert_eq!(started, ManualStart::Started(1));
    assert_eq!(report.accepted, 1);
    let [edit] = edits(&posted.fake).try_into().expect("one edit");
    assert_eq!(
        edit.content
            .as_deref()
            .map(|text| text.lines().next().unwrap_or_default()),
        Some("📅 **Rise and shine, it's Thu 10 Sep!**")
    );
}

/// A refresh whose card and digest edits are on the wire (old lines) when a
/// manual run stores its overrides: the manual edits wait for them and land
/// last, so the posts end with the new lines, never a stale render.
#[tokio::test]
async fn a_refresh_in_flight_never_lands_after_a_manual_edit() {
    let posted = Posted::new(MessageStyle::Redesigned).await;
    let rewriter = BySeed::new(Ok("Rise and shine, it's {day}!"), "Waku waku!");
    answer(
        &*posted.store,
        &posted.star,
        "1002",
        true,
        now() + TimeDelta::minutes(1),
    )
    .await;
    let card_edit = posted.fake.hold(Op::Edit);
    let refresh = posted.refresh(&rewriter);
    let star = posted.star.clone();
    let stale = tokio::spawn(async move { refresh.refresh(std::slice::from_ref(&star)).await });
    card_edit.entered().await;

    let manual = posted.manual(&rewriter);
    let (stop, stopped) = watch::channel(false);
    let worker = {
        let manual = Arc::clone(&manual);
        tokio::spawn(async move { manual.run(stopped).await })
    };
    let mut finished = manual.finished();
    assert_eq!(manual.start(request()).await, ManualStart::Started(2));
    // Both overrides are stored while the refresh still holds the card.
    tokio::time::timeout(Duration::from_secs(5), async {
        while rewriter.calls.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the manual run started");
    card_edit.release();
    assert_eq!(stale.await.expect("no panic"), 2, "card and digest");
    tokio::time::timeout(Duration::from_secs(5), finished.changed())
        .await
        .expect("the run ends")
        .expect("open");
    stop.send_replace(true);
    worker.await.expect("no panic");

    let edited = edits(&posted.fake);
    let last_card = edited
        .iter()
        .filter_map(|edit| edit.content.as_deref())
        .rfind(|text| text.starts_with("📅"))
        .expect("a card edit");
    let last_digest = edited
        .iter()
        .rev()
        .find_map(digest_phrase)
        .expect("a digest edit");
    assert!(
        last_card.starts_with("📅 **Rise and shine, it's Thu 10 Sep!**"),
        "{edited:?}"
    );
    assert_eq!(last_digest, "Waku waku!", "{edited:?}");
}

#[tokio::test]
async fn a_rejected_or_failed_rewrite_stores_and_edits_nothing() {
    for (day_of, expected) in [
        (
            Ok("**Wake up**, {day}"),
            ManualReport {
                rejected: 1,
                ..ManualReport::default()
            },
        ),
        (
            Err(RewriteFailure::Unavailable),
            ManualReport {
                failed: 1,
                ..ManualReport::default()
            },
        ),
    ] {
        let posted = Posted::new(MessageStyle::Classic).await;
        let rewriter = BySeed::new(day_of, "Waku waku!");
        let (_, report) = run_once(&posted.manual(&rewriter)).await;
        assert_eq!(report, expected);
        assert!(edits(&posted.fake).is_empty(), "the post is unchanged");
        let key = posted.card_key().await;
        assert!(
            posted
                .store
                .header_history(&key)
                .await
                .expect("history")
                .overrides
                .is_empty()
        );
        assert_eq!(
            posted
                .store
                .card_record(&key)
                .await
                .expect("record")
                .and_then(|record| record.heading),
            Some("Today — Thu 10 Sep".into())
        );
        assert_eq!(posted.stages().await, [RewriteStage::Manual]);
    }
}

#[tokio::test]
async fn a_second_trigger_while_a_run_is_active_is_refused() {
    let posted = Posted::new(MessageStyle::Classic).await;
    let rewriter = BySeed::gated("Rise and shine, it's {day}!");
    let manual = posted.manual(&rewriter);
    let (stop, stopped) = watch::channel(false);
    let worker = {
        let manual = Arc::clone(&manual);
        tokio::spawn(async move { manual.run(stopped).await })
    };
    let mut finished = manual.finished();
    assert_eq!(manual.start(request()).await, ManualStart::Started(1));
    rewriter.started.notified().await;
    assert_eq!(manual.start(request()).await, ManualStart::Running);
    rewriter.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), finished.changed())
        .await
        .expect("the run ends")
        .expect("open");
    assert_eq!(
        manual.start(request()).await,
        ManualStart::Started(1),
        "free again once the run ended"
    );
    stop.send_replace(true);
    worker.await.expect("no panic");
    assert_eq!(
        manual.start(request()).await,
        ManualStart::Unavailable,
        "refused once the worker stopped"
    );
}

#[tokio::test]
async fn a_stop_ends_the_call_in_flight_and_abandons_the_run() {
    let posted = Posted::new(MessageStyle::Redesigned).await;
    let rewriter = BySeed::gated("Rise and shine, it's {day}!");
    let manual = posted.manual(&rewriter);
    let (stop, stopped) = watch::channel(false);
    let worker = {
        let manual = Arc::clone(&manual);
        tokio::spawn(async move { manual.run(stopped).await })
    };
    let finished = manual.finished();
    assert_eq!(manual.start(request()).await, ManualStart::Started(2));
    rewriter.started.notified().await;
    // Never released: only the stop can end the call.
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("the worker stops at once")
        .expect("no panic");
    assert_eq!(
        *finished.borrow(),
        Some(ManualReport {
            stopped: true,
            ..ManualReport::default()
        })
    );
    assert_eq!(
        rewriter.calls.load(Ordering::SeqCst),
        1,
        "the digest is abandoned"
    );
    assert!(edits(&posted.fake).is_empty(), "nothing is edited");
    assert_eq!(created(&posted.fake).len(), 2, "no summary at shutdown");
    let key = posted.card_key().await;
    assert!(
        posted
            .store
            .header_history(&key)
            .await
            .expect("history")
            .overrides
            .is_empty()
    );
    assert_eq!(
        posted.stages().await,
        [RewriteStage::Manual],
        "the cut call is still logged"
    );
    assert_eq!(manual.start(request()).await, ManualStart::Unavailable);
}

#[tokio::test]
async fn without_a_rewriter_nothing_starts() {
    let posted = Posted::new(MessageStyle::Classic).await;
    let rewriter = BySeed::new(Ok("x {day}"), "Waku waku!");
    let mut refresh = posted.refresh(&rewriter);
    refresh.cards.heading.rewriter = None;
    let manual = ManualRewrite::new(Arc::new(refresh));
    assert_eq!(manual.start(request()).await, ManualStart::Disabled);
}
