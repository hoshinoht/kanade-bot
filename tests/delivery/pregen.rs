//! Reminder header pre-generation: the 12 h horizon, the per-pass cap,
//! failure retries, the digest's week rules and the disabled default.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use kanade::bot::delivery::cards::{DigestPhraseStore, ReminderCardStore};
use kanade::bot::delivery::{
    MAX_ATTEMPTS_PER_KEY, MAX_REWRITES_PER_PASS, PREGEN_DEADLINE, PREGEN_HORIZON, PregenReport,
};
use kanade::domain::history::Origin;
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{DedupeKey, DeliveryJournal, DeliveryTarget};
use kanade::domain::schedule::RunStatus;
use kanade::infrastructure::store::MemoryScheduleStore;
use tokio::sync::watch;

use crate::cards::{Script, Scripted, before_due, created, kit, pregen, rewriting, run, world};
use crate::scenarios::{self, now, previous_week, week};
use crate::support::{self, seed_digest, with_lease};

/// A Kalos countdown firing at `at`; returns its record key.
async fn countdown_at(store: &MemoryScheduleStore, at: DateTime<Utc>) -> String {
    let id = run(
        store,
        &["XKalos"],
        &["1001"],
        at + TimeDelta::minutes(15),
        RunStatus::Planned,
    )
    .await;
    let mut ids = RandomIds;
    let reminder = support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .add_reminder(&id, "countdown_15", at, None)
        .await
        .expect("reminder")
        .expect("new reminder");
    DedupeKey::native(&[DeliveryTarget::Reminder(reminder)])
        .expect("key")
        .as_str()
        .to_owned()
}

async fn heading(store: &MemoryScheduleStore, key: &str) -> Option<String> {
    store
        .card_record(key)
        .await
        .expect("record")
        .and_then(|record| record.heading)
}

#[test]
fn the_window_and_budgets_are_pinned() {
    assert_eq!(PREGEN_HORIZON, TimeDelta::hours(12));
    assert_eq!(PREGEN_DEADLINE, std::time::Duration::from_secs(30));
    assert_eq!(MAX_REWRITES_PER_PASS, 4);
    assert_eq!(MAX_ATTEMPTS_PER_KEY, 3);
}

#[tokio::test]
async fn only_cards_firing_within_the_horizon_are_pregenerated() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let at = now();
    let inside = countdown_at(&store, at + PREGEN_HORIZON).await;
    let outside = countdown_at(&store, at + PREGEN_HORIZON + TimeDelta::minutes(1)).await;
    let past = countdown_at(&store, at - TimeDelta::minutes(1)).await;
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let report = pregen(&store, &world, &rewriting(&rewriter), at)
        .pass()
        .await;
    assert_eq!(report.stored, 1, "{report:?}");
    assert_eq!(rewriter.calls(), 1);
    assert_eq!(heading(&store, &inside).await, Some("Waku waku!".into()));
    assert_eq!(heading(&store, &outside).await, None);
    assert_eq!(
        heading(&store, &past).await,
        None,
        "due cards are the send's"
    );
}

#[tokio::test]
async fn each_pass_rewrites_at_most_its_cap_earliest_first() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let mut keys = Vec::new();
    for hour in 1..=6 {
        keys.push(countdown_at(&store, now() + TimeDelta::hours(hour)).await);
    }
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let worker = pregen(&store, &world, &rewriting(&rewriter), now());
    let first = worker.pass().await;
    assert_eq!((first.stored, first.deferred), (MAX_REWRITES_PER_PASS, 2));
    for key in &keys[..4] {
        assert!(heading(&store, key).await.is_some());
    }
    for key in &keys[4..] {
        assert_eq!(heading(&store, key).await, None);
    }
    let second = worker.pass().await;
    assert_eq!((second.stored, second.deferred), (2, 0));
    assert_eq!(worker.pass().await, PregenReport::default(), "nothing left");
    assert_eq!(rewriter.calls(), 6);
}

#[tokio::test(start_paused = true)]
async fn failed_rewrites_store_nothing_and_stop_after_the_attempt_budget() {
    for script in [
        Script::Fail,
        Script::Hang,
        Script::Reply("confirmed!"),
        Script::Reply("20:14"),
    ] {
        let store = Arc::new(MemoryScheduleStore::new());
        let world = world();
        let key = countdown_at(&store, now() - TimeDelta::minutes(1)).await;
        let rewriter = Scripted::new(script);
        let cards = rewriting(&rewriter);
        let worker = pregen(&store, &world, &cards, before_due());
        for _ in 0..MAX_ATTEMPTS_PER_KEY + 2 {
            worker.pass().await;
        }
        assert_eq!(rewriter.calls(), MAX_ATTEMPTS_PER_KEY as usize);
        assert_eq!(heading(&store, &key).await, None, "nothing stored");
        let mut delivery = scenarios::delivery(&*store, &world, &world.fake).with_cards(cards);
        delivery.dispatch_reminders(now()).await.expect("dispatch");
        assert!(
            created(&world.fake)
                .pop()
                .and_then(|message| message.content)
                .is_some_and(|content| content.starts_with("⏰ Onward! · **XKalos**"))
        );
        assert_eq!(heading(&store, &key).await, Some("Onward!".into()));
        assert_eq!(rewriter.calls(), MAX_ATTEMPTS_PER_KEY as usize);
    }
}

#[tokio::test]
async fn without_a_rewriter_or_persona_nothing_is_pregenerated() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let key = countdown_at(&store, now() + TimeDelta::hours(1)).await;
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let mut no_persona = rewriting(&rewriter);
    no_persona.heading.persona = None;
    for cards in [kit(None), no_persona] {
        let worker = pregen(&store, &world, &cards, now());
        assert_eq!(worker.pass().await, PregenReport::default());
        // The worker returns at once, before any pass.
        let (_stop, stopped) = watch::channel(false);
        worker.run(stopped).await;
    }
    assert_eq!(rewriter.calls(), 0);
    assert_eq!(heading(&store, &key).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_running_worker_stops_mid_rewrite() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let key = countdown_at(&store, now() + TimeDelta::hours(1)).await;
    let rewriter = Scripted::new(Script::Hang);
    let worker = pregen(&store, &world, &rewriting(&rewriter), now());
    let (stop, stopped) = watch::channel(false);
    let running = worker.run(stopped);
    tokio::pin!(running);
    tokio::select! {
        () = &mut running => panic!("the worker ended on its own"),
        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
    }
    assert_eq!(rewriter.calls(), 1, "the first pass is mid-rewrite");
    stop.send_replace(true);
    running.await;
    assert_eq!(heading(&store, &key).await, None);
}

#[tokio::test]
async fn the_digest_phrase_is_pregenerated_only_for_an_unposted_coming_week() {
    let before_reset = week() - TimeDelta::minutes(1);
    let digest_key = DedupeKey::native(&[DeliveryTarget::Digest(week())]).expect("digest key");

    // Too early: the reset is beyond the horizon.
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let cards = rewriting(&rewriter);
    let early = week() - PREGEN_HORIZON - TimeDelta::minutes(1);
    assert_eq!(pregen(&store, &world, &cards, early).pass().await.stored, 0);
    assert_eq!(
        pregen(&store, &world, &cards, before_reset)
            .pass()
            .await
            .stored,
        1
    );
    assert_eq!(
        store
            .digest_phrase(digest_key.as_str())
            .await
            .expect("phrase"),
        Some("Waku waku!".into())
    );

    // A (legacy, phrase-less) digest already posted for that week keeps its
    // line; so does a week the marker already covers.
    let legacy = Arc::new(MemoryScheduleStore::new());
    seed_digest(&*legacy, week(), "222", "7001", previous_week()).await;
    let marked = Arc::new(MemoryScheduleStore::new());
    with_lease(&*marked, week(), async |lease| {
        marked
            .record_digest_week(lease, week(), week())
            .await
            .expect("digest marker");
    })
    .await;
    for store in [legacy, marked] {
        assert_eq!(
            pregen(&store, &world, &cards, before_reset).pass().await,
            PregenReport::default()
        );
        assert_eq!(
            store
                .digest_phrase(digest_key.as_str())
                .await
                .expect("phrase"),
            None
        );
    }
    assert_eq!(rewriter.calls(), 1);
}
