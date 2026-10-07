//! Reminder header pre-generation: the daily batch at the configured time
//! (24 h ahead, no cap), catch-up for cards the batch had not seen, the
//! startup batch, time changes, busy and failure retries, the digest's week
//! rules and the disabled default.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, NaiveTime, TimeDelta, TimeZone, Utc};
use kanade::bot::delivery::cards::{
    CardKit, CardRecord, DigestPhraseStore, HeadingRewrite, ReminderCardStore,
};
use kanade::bot::delivery::{
    HEADER_HORIZON, HeaderPregen, MAX_ATTEMPTS_PER_KEY, MAX_BUSY_RETRIES, MAX_CATCHUP_PER_TICK,
    PREGEN_DEADLINE, PregenReport, SEEN_HORIZON,
};
use kanade::chat::nudge::{
    NudgeRewriter, RewriteAttempt, RewriteDetail, RewriteFailure, RewriteOutcome, RewritePrompt,
    RewriteSink, SharedRewriter, StoreRewriteSink,
};
use kanade::domain::history::Origin;
use kanade::domain::ids::RandomIds;
use kanade::domain::model_log::{RewriteFilter, RewriteLogStore, RewriteStage};
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

/// A worker whose clock and batch time the test moves.
struct Rig {
    clock: Arc<Mutex<DateTime<Utc>>>,
    time: Arc<Mutex<NaiveTime>>,
    worker: HeaderPregen<MemoryScheduleStore>,
}

impl Rig {
    fn new(
        store: &Arc<MemoryScheduleStore>,
        cards: &CardKit,
        at: DateTime<Utc>,
        time: NaiveTime,
    ) -> Self {
        let clock = Arc::new(Mutex::new(at));
        let batch_time = Arc::new(Mutex::new(time));
        let now = Arc::clone(&clock);
        let read = Arc::clone(&batch_time);
        let worker = HeaderPregen::new(
            Arc::clone(store),
            Arc::new(world().roster.clone()),
            cards.clone(),
            scenarios::config().policy,
            Arc::new(move || *now.lock().unwrap()),
        )
        .with_time(Arc::new(move || *read.lock().unwrap()));
        Self {
            clock,
            time: batch_time,
            worker,
        }
    }

    fn at(&self, at: DateTime<Utc>) -> &HeaderPregen<MemoryScheduleStore> {
        *self.clock.lock().unwrap() = at;
        &self.worker
    }
}

/// `hh:mm` in Kuala Lumpur (UTC+8) on `day` September 2026.
fn kl(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap() - TimeDelta::hours(8)
}

fn clock(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
}

/// The cards with a rewrite log, so tests can read each row's stage.
fn logged(rewriter: &Arc<Scripted>, logs: &Arc<MemoryScheduleStore>) -> CardKit {
    let mut cards = rewriting(rewriter);
    cards.heading.log = Some(Arc::new(StoreRewriteSink::new(
        Arc::clone(logs),
        Arc::new(now),
    )));
    cards
}

async fn stages(logs: &MemoryScheduleStore) -> Vec<(String, RewriteStage)> {
    let mut rows: Vec<_> = logs
        .list_rewrites(&RewriteFilter {
            limit: 100,
            ..RewriteFilter::default()
        })
        .await
        .expect("list")
        .items
        .into_iter()
        .map(|row| (row.context.unwrap_or_default(), row.stage))
        .collect();
    rows.sort();
    rows
}

#[test]
fn the_window_and_budgets_are_pinned() {
    assert_eq!(HEADER_HORIZON, TimeDelta::hours(24));
    assert_eq!(SEEN_HORIZON, TimeDelta::days(8));
    assert_eq!(PREGEN_DEADLINE, std::time::Duration::from_secs(30));
    assert_eq!(MAX_CATCHUP_PER_TICK, 4);
    assert_eq!(MAX_ATTEMPTS_PER_KEY, 3);
    assert_eq!(MAX_BUSY_RETRIES, 30);
}

#[tokio::test]
async fn the_batch_at_the_configured_time_covers_the_next_24_hours_only() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let batch = kl(10, 21, 0);
    let soon = countdown_at(&store, batch + TimeDelta::hours(23)).await;
    let later = countdown_at(&store, batch + TimeDelta::hours(25)).await;
    let past = countdown_at(&store, batch - TimeDelta::minutes(1)).await;
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let rig = Rig::new(&store, &logged(&rewriter, &logs), batch, clock(21, 0));

    let report = rig.at(batch).pass().await;
    assert!(report.batch, "{report:?}");
    assert_eq!(report.stored, 1, "{report:?}");
    assert_eq!(heading(&store, &soon).await, Some("Waku waku!".into()));
    assert_eq!(heading(&store, &later).await, None, "25 h ahead");
    assert_eq!(
        heading(&store, &past).await,
        None,
        "due cards are the send's"
    );

    // Within 24 h two hours later, but the batch saw it: the next batch's.
    let between = rig.at(batch + TimeDelta::hours(2)).pass().await;
    assert_eq!((between.batch, between.stored), (false, 0));
    assert_eq!(heading(&store, &later).await, None);
    let next = rig.at(batch + TimeDelta::days(1)).pass().await;
    assert_eq!((next.batch, next.stored), (true, 1));
    assert_eq!(heading(&store, &later).await, Some("Waku waku!".into()));
    assert_eq!(rewriter.calls(), 2);
    assert_eq!(
        stages(&logs).await,
        [(later, RewriteStage::Batch), (soon, RewriteStage::Batch)]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn catch_up_rewrites_only_cards_the_last_batch_had_not_seen() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let batch = kl(10, 3, 0);
    let known = countdown_at(&store, batch + TimeDelta::hours(30)).await;
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let rig = Rig::new(&store, &logged(&rewriter, &logs), batch, clock(3, 0));
    assert!(rig.at(batch).pass().await.batch);
    // A run added after the batch, firing in 5 h: caught up at once.
    let added = countdown_at(&store, batch + TimeDelta::hours(6)).await;
    let tick = rig.at(batch + TimeDelta::hours(1)).pass().await;
    assert_eq!((tick.batch, tick.stored), (false, 1));
    assert_eq!(heading(&store, &added).await, Some("Waku waku!".into()));
    // The card the batch saw 30 h out waits for the next batch even once due.
    let later = rig.at(batch + TimeDelta::hours(7)).pass().await;
    assert_eq!((later.batch, later.stored), (false, 0));
    assert_eq!(heading(&store, &known).await, None);
    assert_eq!(stages(&logs).await, [(added, RewriteStage::Catchup)]);
}

#[tokio::test]
async fn a_start_after_the_days_batch_time_runs_a_batch_at_once() {
    let store = Arc::new(MemoryScheduleStore::new());
    let key = countdown_at(&store, kl(10, 22, 0)).await;
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let cards = rewriting(&rewriter);
    // 09:00 has passed at 20:00: this process has run no batch yet.
    let missed = Rig::new(&store, &cards, kl(10, 20, 0), clock(9, 0));
    let report = missed.worker.pass().await;
    assert_eq!((report.batch, report.stored), (true, 1));
    assert_eq!(heading(&store, &key).await, Some("Waku waku!".into()));
    assert!(!missed.worker.pass().await.batch, "once a day");
    // 21:00 has not: no batch until then.
    let ahead = Rig::new(&store, &cards, kl(10, 20, 0), clock(21, 0));
    assert!(!ahead.worker.pass().await.batch);
    assert!(ahead.at(kl(10, 21, 0)).pass().await.batch);
}

#[tokio::test]
async fn a_changed_time_applies_from_its_next_occurrence() {
    let store = Arc::new(MemoryScheduleStore::new());
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let rig = Rig::new(&store, &rewriting(&rewriter), kl(10, 20, 0), NaiveTime::MIN);
    assert!(rig.worker.pass().await.batch, "00:00 has passed today");
    // Moved to 19:00, already past today: tomorrow's 19:00 is the next.
    *rig.time.lock().unwrap() = clock(19, 0);
    assert!(!rig.at(kl(10, 20, 1)).pass().await.batch);
    assert!(!rig.at(kl(11, 18, 59)).pass().await.batch);
    assert!(rig.at(kl(11, 19, 0)).pass().await.batch);
    // Moved to 22:00 at 20:00: tonight's 22:00 is the next.
    *rig.time.lock().unwrap() = clock(22, 0);
    assert!(!rig.at(kl(11, 20, 0)).pass().await.batch);
    assert!(rig.at(kl(11, 22, 0)).pass().await.batch);
    assert!(!rig.at(kl(11, 23, 0)).pass().await.batch);
}

#[tokio::test]
async fn a_batch_has_no_cap_but_catch_up_does() {
    let store = Arc::new(MemoryScheduleStore::new());
    let mut keys = Vec::new();
    for hour in 1..=6 {
        keys.push(countdown_at(&store, kl(10, 21, 0) + TimeDelta::hours(hour)).await);
    }
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let rig = Rig::new(&store, &rewriting(&rewriter), kl(10, 21, 0), clock(21, 0));
    let batch = rig.worker.pass().await;
    assert_eq!((batch.batch, batch.stored, batch.deferred), (true, 6, 0));
    for key in &keys {
        assert!(heading(&store, key).await.is_some());
    }
    // Six runs added after the batch: four per catch-up tick, earliest first.
    let mut added = Vec::new();
    for hour in 1..=6 {
        added.push(countdown_at(&store, kl(10, 21, 30) + TimeDelta::hours(hour)).await);
    }
    let first = rig.at(kl(10, 21, 1)).pass().await;
    assert_eq!((first.stored, first.deferred), (MAX_CATCHUP_PER_TICK, 2));
    for key in &added[..4] {
        assert!(heading(&store, key).await.is_some());
    }
    let second = rig.at(kl(10, 21, 2)).pass().await;
    assert_eq!((second.stored, second.deferred), (2, 0));
    assert_eq!(
        rig.at(kl(10, 21, 3)).pass().await,
        PregenReport::default(),
        "nothing left"
    );
    assert_eq!(rewriter.calls(), 12);
}

/// Busy the first `busy` calls (a governor refusal), then a valid phrase.
struct Busy {
    busy: usize,
    calls: AtomicUsize,
}

impl NudgeRewriter for Busy {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        self.rewrite_detailed(prompt, deadline).await.result
    }

    async fn rewrite_detailed(&self, _: &RewritePrompt, _: Duration) -> RewriteOutcome {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.busy {
            return RewriteOutcome {
                result: Err(RewriteFailure::Unavailable),
                detail: RewriteDetail {
                    code: Some("busy"),
                    ..RewriteDetail::default()
                },
            };
        }
        RewriteOutcome::plain(Ok("Waku waku!".into()))
    }
}

#[tokio::test]
async fn a_busy_permit_retries_on_later_ticks_without_spending_attempts() {
    let store = Arc::new(MemoryScheduleStore::new());
    let key = countdown_at(&store, kl(10, 23, 0)).await;
    let busy = Arc::new(Busy {
        busy: MAX_ATTEMPTS_PER_KEY as usize + 1,
        calls: AtomicUsize::new(0),
    });
    let cards = CardKit {
        heading: HeadingRewrite {
            rewriter: Some(SharedRewriter(busy.clone())),
            ..rewriting(&Scripted::new(Script::Fail)).heading
        },
        ..kit(None)
    };
    let rig = Rig::new(&store, &cards, kl(10, 21, 0), clock(21, 0));
    let first = rig.worker.pass().await;
    assert_eq!((first.batch, first.failed), (true, 1));
    for minute in 1..=MAX_ATTEMPTS_PER_KEY + 1 {
        let retry = rig.at(kl(10, 21, minute)).pass().await;
        assert!(!retry.batch);
        assert_eq!(retry.failed + retry.stored, 1, "retried every tick");
    }
    assert_eq!(heading(&store, &key).await, Some("Waku waku!".into()));
    assert_eq!(
        busy.calls.load(Ordering::SeqCst),
        MAX_ATTEMPTS_PER_KEY as usize + 2
    );
}

#[tokio::test]
async fn a_stored_line_is_never_rewritten() {
    let store = Arc::new(MemoryScheduleStore::new());
    let key = countdown_at(&store, kl(10, 23, 0)).await;
    store
        .save_card_record(
            &key,
            &CardRecord {
                kind: "countdown_15".into(),
                heading: Some("Onward!".into()),
            },
            now(),
        )
        .await
        .expect("seed stored by a send");
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    let rig = Rig::new(&store, &rewriting(&rewriter), kl(10, 21, 0), clock(21, 0));
    assert!(rig.worker.pass().await.batch);
    *rig.time.lock().unwrap() = clock(21, 30);
    rig.at(kl(10, 21, 30)).pass().await;
    rig.at(kl(10, 21, 31)).pass().await;
    assert_eq!(rewriter.calls(), 0);
    assert_eq!(heading(&store, &key).await, Some("Onward!".into()));
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

/// A rewrite cut by the worker's stop still writes its one row (`shutdown`,
/// with its latency) and stores nothing; the key stays the batch's to retry.
#[tokio::test(start_paused = true)]
async fn a_rewrite_cut_by_the_worker_stop_is_logged_as_shutdown() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let key = countdown_at(&store, now() + TimeDelta::hours(1)).await;
    let rewriter = Scripted::new(Script::Hang);
    let worker = pregen(&store, &world, &logged(&rewriter, &logs), now());
    let (stop, stopped) = watch::channel(false);
    let running = worker.run(stopped);
    tokio::pin!(running);
    tokio::select! {
        () = &mut running => panic!("the worker ended on its own"),
        () = tokio::time::sleep(Duration::from_secs(1)) => {}
    }
    stop.send_replace(true);
    running.await;
    assert_eq!(heading(&store, &key).await, None, "nothing stored");
    let rows = logs
        .list_rewrites(&RewriteFilter {
            limit: 10,
            ..RewriteFilter::default()
        })
        .await
        .expect("list")
        .items;
    let cut: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.context.as_deref(),
                row.stage,
                row.verdict.as_str(),
                row.code.as_deref(),
                row.latency_ms,
            )
        })
        .collect();
    assert_eq!(
        cut,
        [(
            Some(key.as_str()),
            RewriteStage::Batch,
            "unavailable",
            Some("shutdown"),
            Some(1000)
        )]
    );
    // No attempt was spent: the next pass retries it as the batch's.
    assert_eq!(worker.pass().await.failed, 1, "the hang now times out");
    assert_eq!(rewriter.calls(), 2);
}

/// Stops the worker while logging, i.e. after the model call returned.
struct StopOnLog {
    inner: StoreRewriteSink<MemoryScheduleStore>,
    stop: watch::Sender<bool>,
}

impl RewriteSink for StopOnLog {
    fn record(&self, attempt: RewriteAttempt) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.stop.send_replace(true);
        self.inner.record(attempt)
    }
}

/// A rewrite that failed on its own is a failure even when the stop fires
/// before the worker reads the result: the attempt is spent, not retried free.
#[tokio::test]
async fn a_failure_finished_before_the_stop_still_counts() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let key = countdown_at(&store, now() + TimeDelta::hours(1)).await;
    let rewriter = Scripted::new(Script::Fail);
    let (stop, stopped) = watch::channel(false);
    let mut cards = rewriting(&rewriter);
    cards.heading.log = Some(Arc::new(StopOnLog {
        inner: StoreRewriteSink::new(Arc::clone(&logs), Arc::new(now)),
        stop,
    }));
    let worker = pregen(&store, &world, &cards, now());
    worker.run(stopped).await;
    let rows = logs
        .list_rewrites(&RewriteFilter {
            limit: 10,
            ..RewriteFilter::default()
        })
        .await
        .expect("list")
        .items;
    assert_eq!(rows.len(), 1);
    assert_ne!(rows[0].code.as_deref(), Some("shutdown"), "{:?}", rows[0]);
    assert_eq!(heading(&store, &key).await, None);
    // One attempt spent: the budget runs out one call early.
    for _ in 1..MAX_ATTEMPTS_PER_KEY {
        worker.pass().await;
    }
    assert_eq!(worker.pass().await, PregenReport::default(), "budget spent");
    assert_eq!(rewriter.calls(), MAX_ATTEMPTS_PER_KEY as usize);
}

#[tokio::test]
async fn before_its_first_batch_a_process_catches_up_only_until_the_batch_time() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let early = countdown_at(&store, kl(10, 20, 45)).await;
    let late = countdown_at(&store, kl(10, 23, 0)).await;
    let rewriter = Scripted::new(Script::Reply("Waku waku!"));
    // Started at 20:00 with the batch at 21:00: yesterday's batch covered
    // the cards due by 21:00, so only those are caught up now.
    let rig = Rig::new(
        &store,
        &logged(&rewriter, &logs),
        kl(10, 20, 0),
        clock(21, 0),
    );
    let first = rig.worker.pass().await;
    assert_eq!((first.batch, first.stored), (false, 1), "{first:?}");
    assert_eq!(heading(&store, &early).await, Some("Waku waku!".into()));
    assert_eq!(heading(&store, &late).await, None, "left for the batch");
    assert_eq!(rig.at(kl(10, 20, 30)).pass().await.stored, 0);
    let batch = rig.at(kl(10, 21, 0)).pass().await;
    assert_eq!((batch.batch, batch.stored), (true, 1), "{batch:?}");
    assert_eq!(heading(&store, &late).await, Some("Waku waku!".into()));
    let mut expected = vec![(early, RewriteStage::Catchup), (late, RewriteStage::Batch)];
    expected.sort();
    assert_eq!(stages(&logs).await, expected);
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
    let early = week() - HEADER_HORIZON - TimeDelta::minutes(1);
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
            PregenReport {
                batch: true,
                ..PregenReport::default()
            }
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
