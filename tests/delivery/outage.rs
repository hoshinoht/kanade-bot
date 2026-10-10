use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use chrono::{DateTime, Utc};
use kanade::bot::delivery::{DigestOutcome, SendOutcome};
use kanade::bot::ids::parse_id;
use kanade::bot::transport::{AmbiguousKind, FakeDiscord, Op, Outcome};
use kanade::domain::history::Origin;
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{NewRun, RunSource, RunStatus, StatusChange};
use kanade::domain::time::from_iso;
use tokio::sync::{Mutex, oneshot};

use crate::intercept::Intercept;
use crate::scenarios::{self, HOME, POST};
use crate::support::{self, Store, on_both_stores, seed_digest, with_lease};

async fn pending_notice<S: Store>(store: &S, now: DateTime<Utc>) {
    let config = scenarios::config();
    let mut ids = RandomIds;
    let run = support::service(store, &mut ids, now)
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: scenarios::week(),
            datetime: now + chrono::Duration::hours(3),
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Fixed,
        })
        .await
        .expect("run");
    support::service(store, &mut ids, now)
        .as_origin(Origin::for_tests())
        .set_status(
            &run,
            StatusChange {
                status: RunStatus::Cancelled,
                announce: true,
                via_portal: true,
            },
            &config.policy.reminders,
        )
        .await
        .expect("notice");
}

async fn replaceable_digest<S: Store>(store: &S, fake: &FakeDiscord, now: DateTime<Utc>) {
    const MESSAGE: &str = "900000000000000001";
    seed_digest(store, scenarios::week(), POST, MESSAGE, now).await;
    fake.seed_message(parse_id(POST).unwrap(), parse_id(MESSAGE).unwrap());
    with_lease(store, now, async |lease| {
        store
            .record_digest_week(lease, scenarios::previous_week(), now)
            .await
            .expect("marker");
    })
    .await;
}

fn close_on_admission(
    connected: Arc<AtomicBool>,
    close_on: usize,
) -> (impl Fn() -> bool + Send + Sync + 'static, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    // Flip between permit issuance and revalidation at the selected effect.
    let gate = move || {
        let call = observed.fetch_add(1, Ordering::SeqCst) + 1;
        if call == close_on {
            connected.store(false, Ordering::SeqCst);
        }
        connected.load(Ordering::SeqCst)
    };
    (gate, calls)
}

async fn digest_marker<S: Store>(store: &S, week: DateTime<Utc>, now: DateTime<Utc>) {
    with_lease(store, now, async |lease| {
        store
            .record_digest_week(lease, week, now)
            .await
            .expect("marker");
    })
    .await;
}

async fn pause_then_recover<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    pending_notice(store, now).await;
    let (_, reminder) = scenarios::due_countdown(store, HOME).await;
    replaceable_digest(store, &world.fake, now).await;
    let paused = Arc::new(AtomicBool::new(false));
    let gate = Arc::clone(&paused);
    let mut delivery = scenarios::delivery(store, &world, &world.fake)
        .with_claim_gate(move || gate.load(std::sync::atomic::Ordering::Relaxed));

    let report = delivery.tick_at(now).await.expect("paused tick");
    assert_eq!(report.digest.outcome, DigestOutcome::Paused);
    assert!(report.notices.sends.is_empty());
    assert!(report.dispatch.sends.is_empty());
    assert_eq!(
        (world.fake.count(Op::Create), world.fake.count(Op::Delete)),
        (0, 0)
    );
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 1);
    assert_eq!(
        scenarios::reminder_row(store, &reminder).await.sent_at,
        None
    );
    let digests = store.load_digests().await.unwrap();
    assert!(digests.digests.iter().all(|row| row.retired_at.is_none()));
    assert_eq!(
        digests
            .last_digest_week
            .as_deref()
            .and_then(|week| from_iso(week).ok()),
        Some(scenarios::previous_week())
    );
    assert_eq!(
        store
            .list_checkpoints(Some(scenarios::week()))
            .await
            .unwrap()
            .len(),
        1,
        "maintenance/materialisation still runs while claims are paused"
    );

    paused.store(true, std::sync::atomic::Ordering::Relaxed);
    let report = delivery
        .tick_at(now + chrono::Duration::minutes(1))
        .await
        .expect("recovered tick");
    assert_eq!(report.notices.sends.len(), 1);
    assert_eq!(report.dispatch.sends.len(), 1);
    assert_eq!(report.digest.outcome, DigestOutcome::Attempted);
    assert_eq!(world.fake.count(Op::Delete), 1);
    assert_eq!(world.fake.count(Op::Create), 3);
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 0);
    assert!(
        scenarios::reminder_row(store, &reminder)
            .await
            .sent_at
            .is_some()
    );
    let digests = store.load_digests().await.unwrap();
    assert!(digests.digests.iter().any(|row| {
        row.week_start == scenarios::week()
            && row.retired_at.is_none()
            && row.message_id != "900000000000000001"
    }));
}

#[tokio::test]
async fn paused_claims_resume_normally_on_memory_and_sqlite() {
    on_both_stores!(pause_then_recover);
}

async fn notice_admission_rechecks_after_render<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    pending_notice(store, now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, calls) = close_on_admission(connected, 2);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery.tick_at(now).await.expect("tick");
    assert!(report.notices.sends.is_empty());
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 1);
    assert_eq!(
        (world.fake.count(Op::Create), world.fake.count(Op::Delete)),
        (0, 0)
    );
    assert!(calls.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn closing_at_the_notice_admission_boundary_prevents_the_claim() {
    on_both_stores!(notice_admission_rechecks_after_render);
}

async fn admitted_notice_claim_and_drain_settle_after_close_on_store<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    pending_notice(store, now).await;
    let (_, reminder) = scenarios::due_countdown(store, HOME).await;
    let connected = Arc::new(AtomicBool::new(true));
    let gate = Arc::clone(&connected);
    let (owner_entered, wait_owner) = oneshot::channel();
    let (release_owner, owner_release) = oneshot::channel();
    let owner_entered = Arc::new(StdMutex::new(Some(owner_entered)));
    let owner_release = Arc::new(Mutex::new(Some(owner_release)));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let entered = Arc::clone(&owner_entered);
    let release = Arc::clone(&owner_release);
    let admission_hook = move || {
        let call = observed.fetch_add(1, Ordering::SeqCst) + 1;
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Box::pin(async move {
            if call == 1 {
                if let Some(entered) = entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                if let Some(release) = release.lock().await.take() {
                    let _ = release.await;
                }
            }
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    };
    let mut delivery = scenarios::delivery(store, &world, &world.fake)
        .with_claim_gate(move || gate.load(Ordering::SeqCst))
        .with_admission_hook(admission_hook);

    let report = {
        let tick = delivery.tick_at(now);
        tokio::pin!(tick);
        tokio::select! {
            _ = wait_owner => {}
            result = &mut tick => panic!("notice operation did not reach the admission barrier: {result:?}"),
        }
        assert_eq!(
            world.fake.count(Op::Create),
            0,
            "claim starts after the barrier"
        );
        assert_eq!(store.pending_notices().await.unwrap().notices.len(), 1);
        connected.store(false, Ordering::SeqCst);
        release_owner.send(()).unwrap();
        tick.await.expect("owned notice settles")
    };

    assert_eq!(world.fake.count(Op::Create), 1);
    assert_eq!(report.notices.sends.len(), 1);
    assert!(report.notices.sends[0].drained);
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 0);
    assert_eq!(
        scenarios::reminder_row(store, &reminder).await.sent_at,
        None
    );
}

#[tokio::test]
async fn admitted_notice_claim_and_drain_settle_after_close() {
    on_both_stores!(admitted_notice_claim_and_drain_settle_after_close_on_store);
}

async fn stale_notice_admission_rechecks_before_retirement<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    pending_notice(store, now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, _) = close_on_admission(connected, 2);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery
        .tick_at(now + chrono::Duration::hours(7))
        .await
        .expect("tick");
    assert_eq!(report.notices.stale, 0);
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 1);
}

#[tokio::test]
async fn closing_at_stale_notice_retirement_leaves_it_pending() {
    on_both_stores!(stale_notice_admission_rechecks_before_retirement);
}

async fn reminder_admission_rechecks_after_planning<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    let (_, reminder) = scenarios::due_countdown(store, HOME).await;
    digest_marker(store, scenarios::week(), now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, calls) = close_on_admission(connected, 4);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery.tick_at(now).await.expect("tick");
    assert!(report.dispatch.sends.is_empty());
    assert_eq!(
        scenarios::reminder_row(store, &reminder).await.sent_at,
        None
    );
    assert_eq!(
        (world.fake.count(Op::Create), world.fake.count(Op::Delete)),
        (0, 0)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn closing_at_the_reminder_admission_boundary_prevents_the_claim() {
    on_both_stores!(reminder_admission_rechecks_after_planning);
}

async fn stale_reminder_admission_rechecks_before_retirement<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    let reminder = stale_countdown(store, now).await;
    digest_marker(store, scenarios::week(), now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, calls) = close_on_admission(connected, 4);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery
        .tick_at(now + chrono::Duration::hours(7))
        .await
        .expect("tick");
    assert_eq!(report.dispatch.retired, 0);
    assert_eq!(
        scenarios::reminder_row(store, &reminder).await.sent_at,
        None
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn closing_at_stale_reminder_retirement_leaves_it_pending() {
    on_both_stores!(stale_reminder_admission_rechecks_before_retirement);
}

async fn digest_claim_rechecks_after_planning<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    digest_marker(store, scenarios::previous_week(), now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, calls) = close_on_admission(connected, 4);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery.tick_at(now).await.expect("tick");
    assert_eq!(report.digest.outcome, DigestOutcome::Paused);
    assert_eq!(world.fake.count(Op::Create), 0);
    assert_eq!(
        store
            .load_digests()
            .await
            .unwrap()
            .last_digest_week
            .as_deref()
            .and_then(|week| from_iso(week).ok()),
        Some(scenarios::previous_week())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn closing_at_the_digest_claim_boundary_prevents_the_claim() {
    on_both_stores!(digest_claim_rechecks_after_planning);
}

async fn digest_delete_rechecks_at_the_remote_boundary<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    replaceable_digest(store, &world.fake, now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, calls) = close_on_admission(connected, 4);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery.tick_at(now).await.expect("tick");
    assert_eq!(report.digest.outcome, DigestOutcome::Paused);
    assert_eq!(
        (world.fake.count(Op::Delete), world.fake.count(Op::Create)),
        (0, 0)
    );
    assert!(
        store
            .load_digests()
            .await
            .unwrap()
            .digests
            .iter()
            .any(|row| row.message_id == "900000000000000001" && row.retired_at.is_none())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn closing_at_the_digest_delete_boundary_preserves_the_old_card() {
    on_both_stores!(digest_delete_rechecks_at_the_remote_boundary);
}

async fn admitted_digest_delete_settles_then_defers_create<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    replaceable_digest(store, &world.fake, now).await;
    let (owner_entered, owner_wait) = oneshot::channel();
    let (owner_release, release_owner) = oneshot::channel();
    let owner_entered = Arc::new(StdMutex::new(Some(owner_entered)));
    let release_owner = Arc::new(Mutex::new(Some(release_owner)));
    let admission_calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&admission_calls);
    let admission_entered = Arc::clone(&owner_entered);
    let admission_release = Arc::clone(&release_owner);
    let admission_hook = move || {
        let call = observed.fetch_add(1, Ordering::SeqCst) + 1;
        let entered = Arc::clone(&admission_entered);
        let release = Arc::clone(&admission_release);
        Box::pin(async move {
            if call == 2 {
                if let Some(entered) = entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                if let Some(release) = release.lock().await.take() {
                    let _ = release.await;
                }
            }
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    };
    let (delete_started, wait_delete) = oneshot::channel();
    let (finish_delete, release_delete) = oneshot::channel();
    let delete_started = Arc::new(StdMutex::new(Some(delete_started)));
    let release_delete = Arc::new(Mutex::new(Some(release_delete)));
    let transport = Intercept::new(&world.fake).on_delete(move |outcome| {
        let entered = Arc::clone(&delete_started);
        let release_delete = Arc::clone(&release_delete);
        Box::pin(async move {
            if let Some(entered) = entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            if let Some(release) = release_delete.lock().await.take() {
                let _ = release.await;
            }
            outcome
        })
    });
    let connected = Arc::new(AtomicBool::new(true));
    let gate = Arc::clone(&connected);
    let mut delivery = scenarios::delivery(store, &world, &transport)
        .with_claim_gate(move || gate.load(Ordering::SeqCst))
        .with_admission_hook(admission_hook);

    let report = {
        let tick = delivery.tick_at(now);
        tokio::pin!(tick);
        tokio::select! {
            _ = owner_wait => {}
            result = &mut tick => panic!("owner was not acquired before the barrier: {result:?}"),
        }
        connected.store(false, Ordering::SeqCst);
        owner_release.send(()).unwrap();
        tokio::select! {
            _ = wait_delete => {}
            result = &mut tick => panic!("owned delete did not reach Discord: {result:?}"),
        }
        finish_delete.send(()).unwrap();
        tick.await.expect("settled delete")
    };

    assert_eq!(report.digest.outcome, DigestOutcome::Paused);
    assert_eq!(
        (world.fake.count(Op::Delete), world.fake.count(Op::Create)),
        (1, 0)
    );
    let digests = store.load_digests().await.unwrap();
    assert!(
        digests
            .digests
            .iter()
            .any(|row| { row.message_id == "900000000000000001" && row.retired_at.is_some() })
    );
}

#[tokio::test]
async fn admitted_digest_delete_settles_but_close_denies_replacement_create() {
    on_both_stores!(admitted_digest_delete_settles_then_defers_create);
}

async fn confirmed_digest_delete_needs_a_fresh_create_admission<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    replaceable_digest(store, &world.fake, now).await;
    let connected = Arc::new(AtomicBool::new(true));
    let (gate, calls) = close_on_admission(connected, 5);
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_claim_gate(gate);

    let report = delivery.tick_at(now).await.expect("tick");
    assert_eq!(report.digest.outcome, DigestOutcome::Paused);
    assert_eq!(
        (world.fake.count(Op::Delete), world.fake.count(Op::Create)),
        (1, 0)
    );
    assert!(
        store
            .load_digests()
            .await
            .unwrap()
            .digests
            .iter()
            .any(|row| { row.message_id == "900000000000000001" && row.retired_at.is_some() })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn replacement_create_is_separately_admitted_after_confirmed_delete() {
    on_both_stores!(confirmed_digest_delete_needs_a_fresh_create_admission);
}

async fn stale_work_waits_until_recovery<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    pending_notice(store, now).await;
    let reminder = stale_countdown(store, now).await;
    let paused = Arc::new(AtomicBool::new(false));
    let gate = Arc::clone(&paused);
    let mut delivery = scenarios::delivery(store, &world, &world.fake)
        .with_claim_gate(move || gate.load(std::sync::atomic::Ordering::Relaxed));
    let late = now + chrono::Duration::hours(7);

    let report = delivery.tick_at(late).await.expect("paused stale tick");
    assert_eq!(report.digest.outcome, DigestOutcome::Paused);
    assert_eq!((report.notices.stale, report.dispatch.retired), (0, 0));
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 1);
    assert_eq!(
        scenarios::reminder_row(store, &reminder).await.sent_at,
        None
    );
    assert_eq!(world.fake.count(Op::Create), 0);

    paused.store(true, std::sync::atomic::Ordering::Relaxed);
    let report = delivery.tick_at(late).await.expect("recovered stale tick");
    assert_eq!(report.notices.stale, 1);
    assert_eq!(report.dispatch.retired, 1);
    assert_eq!(store.pending_notices().await.unwrap().notices.len(), 0);
    let row = scenarios::reminder_row(store, &reminder).await;
    assert!(row.sent_at.is_some());
    assert!(row.message_id.is_none());
    assert_eq!(world.fake.count(Op::Create), 0);
}

async fn stale_countdown<S: Store>(store: &S, now: DateTime<Utc>) -> String {
    let mut ids = RandomIds;
    let run = support::service(store, &mut ids, now)
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: scenarios::week(),
            datetime: now + chrono::Duration::hours(14),
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Fixed,
        })
        .await
        .expect("future run");
    support::service(store, &mut ids, now)
        .as_origin(Origin::for_tests())
        .add_reminder(
            &run,
            "countdown_60",
            now - chrono::Duration::minutes(1),
            None,
        )
        .await
        .expect("reminder")
        .expect("new reminder")
}

#[tokio::test]
async fn pending_due_work_uses_existing_staleness_rules_after_recovery() {
    on_both_stores!(stale_work_waits_until_recovery);
}

async fn ambiguous_in_flight_send_is_not_replayed<S: Store>(store: &S) {
    let world = scenarios::world();
    let now = scenarios::now();
    scenarios::due_countdown(store, HOME).await;
    let (started, entered) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let started = Arc::new(StdMutex::new(Some(started)));
    let wait = Arc::new(Mutex::new(Some(wait)));
    let transport = Intercept::new(&world.fake).on_create(move |_| {
        let started = Arc::clone(&started);
        let wait = Arc::clone(&wait);
        Box::pin(async move {
            if let Some(started) = started.lock().unwrap().take() {
                let _ = started.send(());
            }
            if let Some(wait) = wait.lock().await.take() {
                let _ = wait.await;
            }
            Outcome::Ambiguous(AmbiguousKind::Timeout)
        })
    });
    let connected = Arc::new(AtomicBool::new(true));
    let gate = Arc::clone(&connected);
    let mut delivery = scenarios::delivery(store, &world, &transport)
        .with_claim_gate(move || gate.load(std::sync::atomic::Ordering::Relaxed));
    let report = {
        let tick = delivery.tick_at(now);
        tokio::pin!(tick);
        tokio::select! {
            _ = entered => {}
            result = &mut tick => panic!("send did not hang after its claim: {result:?}"),
        }
        connected.store(false, std::sync::atomic::Ordering::Relaxed);
        release.send(()).unwrap();
        tick.await.expect("indeterminate tick")
    };
    assert_eq!(report.dispatch.sends[0].outcome, SendOutcome::Uncertain);
    assert_eq!(world.fake.count(Op::Create), 1);

    delivery
        .tick_at(now + chrono::Duration::minutes(1))
        .await
        .expect("disconnected tick");
    connected.store(true, std::sync::atomic::Ordering::Relaxed);
    delivery
        .tick_at(now + chrono::Duration::minutes(2))
        .await
        .expect("reconnected tick");
    assert_eq!(
        world.fake.count(Op::Create),
        1,
        "ambiguous claim is never replayed"
    );
}

#[tokio::test]
async fn a_hanging_send_stays_indeterminate_across_disconnect() {
    on_both_stores!(ambiguous_in_flight_send_is_not_replayed);
}
