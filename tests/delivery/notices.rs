//! The notice outbox drained by the tick, crash window by crash window, on
//! the memory store (a "restart" is `recover_on_start` on the same store)
//! and on SQLite (a real close and reopen).

use kanade::bot::delivery::SendOutcome;
use kanade::bot::transport::{AmbiguousKind, Op, RejectionKind, Step};
use kanade::domain::history::Origin;
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{
    AttemptState, Claim, DeliveryJournal, DeliverySettings, Receipt, plan_notice,
};
use kanade::domain::schedule::{NewRun, RunSource, RunStatus, StatusChange};
use kanade::domain::scheduler::SchedulerError;

use crate::scenarios::{HOME, POST, World, config, delivery, now, week, world};
use crate::support::{self, INSTANCE, Store, TempDir, on_both_stores};

/// A run in `HOME`, then its cancellation with a channel notice (one outbox
/// row). `request_id` makes the cancellation idempotent.
async fn cancel_with_notice<S: Store>(store: &S, request_id: &str) -> Result<(), SchedulerError> {
    let mut ids = RandomIds;
    let mut service = support::service(store, &mut ids, now());
    let run = match store.load(&kanade::domain::scheduler::Scope::All).await {
        Ok(snapshot) if !snapshot.runs.is_empty() => snapshot.runs[0].id.clone(),
        _ => service
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: None,
                channel_id: Some(HOME.into()),
                week_start: week(),
                datetime: now() + chrono::Duration::hours(3),
                bosses: vec!["Kalos".into()],
                participants: vec!["1001".into()],
                status: RunStatus::Planned,
                source: RunSource::Fixed,
            })
            .await
            .expect("run"),
    };
    let outcome = service
        .as_origin(Origin::for_tests().with_request_id(request_id))
        .set_status(
            &run,
            StatusChange {
                status: RunStatus::Cancelled,
                announce: true,
                via_portal: true,
            },
            &config().policy.reminders,
        )
        .await?;
    assert_eq!(outcome.notices.len(), 1);
    Ok(())
}

async fn pending<S: Store>(store: &S) -> usize {
    store
        .pending_notices()
        .await
        .expect("pending")
        .notices
        .len()
}

/// Claim the only pending notice as the drain would, then stop: the crash
/// after the claim. `bind` also records a delivered post before the crash.
async fn claim_then_crash<S: Store>(store: &S, world: &World, bind: bool) {
    let row = store
        .pending_notices()
        .await
        .expect("pending")
        .notices
        .remove(0);
    let settings = DeliverySettings {
        post_channel_id: Some(POST),
        quiet_mode: false,
        attendance: config().policy.attendance,
    };
    let intent =
        plan_notice(&row.notice, &world.roster, &world.channels, settings).expect("routed");
    let lease = store
        .begin_lease(INSTANCE, "scheduler_tick", now())
        .await
        .expect("lease");
    let Claim::Fresh(attempt) = store
        .claim_source(&lease, &intent, &row.source, row.ordinal, now())
        .await
        .expect("claim")
    else {
        panic!("first claim held");
    };
    if bind {
        let receipt = Receipt {
            channel_id: HOME.into(),
            message_id: "900000000000000001".into(),
        };
        store
            .bind(&lease, &attempt, &receipt, None, now())
            .await
            .expect("bind");
    }
}

async fn committed_then_crashed_posts_once<S: Store>(store: &S) {
    let world = world();
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    assert_eq!(pending(store).await, 1);
    assert_eq!(world.fake.count(Op::Create), 0, "the commit posts nothing");
    // Crash before any drain: restart recovery, then the next tick.
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.start(now()).await.expect("recover");
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(report.notices.sends.len(), 1);
    assert!(matches!(
        report.notices.sends[0].send.outcome,
        SendOutcome::Bound(_)
    ));
    assert!(report.notices.sends[0].drained);
    assert_eq!(world.fake.count(Op::Create), 1);
    for minutes in [1, 2] {
        let later = now() + chrono::Duration::minutes(minutes);
        let report = delivery.tick_at(later).await.expect("tick");
        assert!(report.notices.sends.is_empty());
    }
    assert_eq!(world.fake.count(Op::Create), 1, "exactly one send");
    assert_eq!(pending(store).await, 0);
}

#[tokio::test]
async fn a_notice_committed_before_a_crash_is_posted_exactly_once() {
    on_both_stores!(committed_then_crashed_posts_once);
}

#[tokio::test]
async fn a_notice_survives_a_real_sqlite_restart_and_posts_once() {
    let dir = TempDir::new();
    let store = dir.open().await;
    cancel_with_notice(&store, "cancel-1")
        .await
        .expect("cancel");
    // No close: the process died; the file is reopened by the next owner
    // only after this one lets go of it.
    store.close().await.expect("close");
    let store = dir.open().await;
    let world = world();
    let mut delivery = delivery(&store, &world, &world.fake);
    delivery.start(now()).await.expect("recover");
    delivery.tick_at(now()).await.expect("tick");
    delivery
        .tick_at(now() + chrono::Duration::minutes(1))
        .await
        .expect("tick");
    assert_eq!(world.fake.count(Op::Create), 1);
    drop(delivery);
    assert_eq!(pending(&store).await, 0);
    store.close().await.expect("close");
}

async fn claimed_then_crashed_is_never_resent<S: Store>(store: &S, bind: bool) {
    let world = world();
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    claim_then_crash(store, &world, bind).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let recovery = delivery.start(now()).await.expect("recover");
    assert_eq!(recovery.indeterminate.len(), usize::from(!bind));
    let report = delivery.drain_notices(now()).await.expect("drain");
    assert_eq!(report.sends.len(), 1);
    assert_eq!(report.sends[0].send.outcome, SendOutcome::Suppressed);
    assert!(
        report.sends[0].drained,
        "held by the earlier claim: drained"
    );
    delivery
        .tick_at(now() + chrono::Duration::minutes(1))
        .await
        .expect("tick");
    assert_eq!(world.fake.count(Op::Create), 0, "never sent again");
    assert_eq!(pending(store).await, 0);
}

async fn claimed_unsent_then_crashed<S: Store>(store: &S) {
    claimed_then_crashed_is_never_resent(store, false).await;
}

async fn claimed_bound_then_crashed<S: Store>(store: &S) {
    claimed_then_crashed_is_never_resent(store, true).await;
}

#[tokio::test]
async fn a_claim_that_crashed_before_draining_is_indeterminate_and_never_resent() {
    on_both_stores!(claimed_unsent_then_crashed);
}

#[tokio::test]
async fn a_delivered_notice_that_crashed_before_draining_is_not_resent() {
    on_both_stores!(claimed_bound_then_crashed);
}

#[tokio::test]
async fn a_sqlite_claim_survives_the_restart_that_orphans_its_lease() {
    let dir = TempDir::new();
    let store = dir.open().await;
    let world = world();
    cancel_with_notice(&store, "cancel-1")
        .await
        .expect("cancel");
    claim_then_crash(&store, &world, false).await;
    store.close().await.expect("close");
    let store = dir.open().await;
    let mut delivery = delivery(&store, &world, &world.fake);
    let recovery = delivery.start(now()).await.expect("recover");
    assert_eq!(
        (recovery.indeterminate.len(), recovery.orphaned_leases),
        (1, 1)
    );
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(
        report.notices.sends[0].send.outcome,
        SendOutcome::Suppressed
    );
    assert_eq!(world.fake.count(Op::Create), 0);
    drop(delivery);
    let attempt = &recovery.indeterminate[0];
    let record = store
        .load_attempt(attempt)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(record.state, AttemptState::Indeterminate);
    store.close().await.expect("close");
}

async fn ambiguous_is_never_resent<S: Store>(store: &S) {
    let world = world();
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    world.fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(report.notices.sends[0].send.outcome, SendOutcome::Uncertain);
    assert!(report.notices.sends[0].drained);
    delivery.start(now()).await.expect("restart");
    for minutes in [1, 2] {
        delivery
            .tick_at(now() + chrono::Duration::minutes(minutes))
            .await
            .expect("tick");
    }
    assert_eq!(world.fake.count(Op::Create), 1, "never resent");
    assert_eq!(pending(store).await, 0);
}

#[tokio::test]
async fn an_ambiguous_notice_is_never_resent() {
    on_both_stores!(ambiguous_is_never_resent);
}

async fn unsent_is_retried_once<S: Store>(store: &S) {
    let world = world();
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    world
        .fake
        .script(Op::Create, Step::Reject(RejectionKind::NotSent));
    let mut delivery = delivery(store, &world, &world.fake);
    let first = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(
        first.notices.sends[0].send.outcome,
        SendOutcome::Released(RejectionKind::NotSent)
    );
    assert!(
        !first.notices.sends[0].drained,
        "proven unsent: still pending"
    );
    assert_eq!(pending(store).await, 1);
    let second = delivery
        .tick_at(now() + chrono::Duration::minutes(1))
        .await
        .expect("tick");
    assert!(matches!(
        second.notices.sends[0].send.outcome,
        SendOutcome::Bound(_)
    ));
    assert_eq!(world.fake.count(Op::Create), 2, "one refused, one posted");
    assert_eq!(world.fake.messages().len(), 1);
    assert_eq!(pending(store).await, 0);
}

#[tokio::test]
async fn a_notice_proven_unsent_is_claimed_again() {
    on_both_stores!(unsent_is_retried_once);
}

async fn rejected_is_not_retried<S: Store>(store: &S) {
    let world = world();
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    world
        .fake
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(
        report.notices.sends[0].send.outcome,
        SendOutcome::Rejected(RejectionKind::MissingPermissions)
    );
    delivery
        .tick_at(now() + chrono::Duration::minutes(1))
        .await
        .expect("tick");
    assert_eq!(world.fake.count(Op::Create), 1);
    assert_eq!(pending(store).await, 0);
}

#[tokio::test]
async fn a_rejected_notice_is_not_retried() {
    on_both_stores!(rejected_is_not_retried);
}

async fn replay_enqueues_nothing<S: Store>(store: &S) {
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    let before = store.outbox_notices().await.expect("outbox");
    assert!(matches!(
        cancel_with_notice(store, "cancel-1").await,
        Err(SchedulerError::AlreadyApplied { .. })
    ));
    assert_eq!(store.outbox_notices().await.expect("outbox"), before);
}

#[tokio::test]
async fn an_already_applied_retry_adds_no_outbox_rows() {
    on_both_stores!(replay_enqueues_nothing);
}

async fn unroutable_waits<S: Store>(store: &S) {
    let mut world = world();
    cancel_with_notice(store, "cancel-1").await.expect("cancel");
    world.channels.clear();
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(
        (report.notices.unroutable, report.notices.sends.len()),
        (1, 0)
    );
    assert_eq!(pending(store).await, 1, "kept for a reachable channel");
}

#[tokio::test]
async fn a_notice_with_nowhere_to_post_stays_pending() {
    on_both_stores!(unroutable_waits);
}
