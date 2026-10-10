//! Failure paths: the per-tick cap, per-send error isolation, bind and
//! replacement refusals, cancelled ticks, reaction failures and alert
//! throttling, each on the memory and SQLite stores.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::TimeDelta;
use kanade::bot::delivery::{ALERT_WINDOW, AdminAlert, AlertThrottle, DigestOutcome, SendOutcome};
use kanade::bot::transport::{AmbiguousKind, Op, Outcome, RejectionKind, Step};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{AttemptState, DeliveryTarget, EffectKind};

use crate::intercept::Intercept;
use crate::scenarios::{
    HOME, delivery, due_countdown, due_countdown_at, now, post_then_die, reminder_row,
    seed_replaceable, tick_times, world,
};
use crate::support::{self, Store, on_both_stores, with_lease};

fn ambiguous() -> Step {
    Step::Ambiguous {
        kind: AmbiguousKind::Timeout,
        applied: true,
    }
}

async fn cap_counts_claims_only<S: Store>(store: &S) {
    let world = world();
    due_countdown_at(store, HOME, 180).await;
    due_countdown_at(store, HOME, 120).await;
    world.fake.script(Op::Create, ambiguous());
    world.fake.script(Op::Create, ambiguous());
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let (_, fresh) = due_countdown_at(store, HOME, 60).await;
    delivery.config.max_sends_per_tick = 2;
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    let outcomes: Vec<&SendOutcome> = report.sends.iter().map(|send| &send.outcome).collect();
    assert!(
        matches!(
            outcomes.as_slice(),
            [
                SendOutcome::Suppressed,
                SendOutcome::Suppressed,
                SendOutcome::Bound(_)
            ]
        ),
        "{outcomes:?}"
    );
    assert_eq!(report.deferred, 0);
    assert!(reminder_row(store, &fresh).await.message_id.is_some());
}

#[tokio::test]
async fn held_sends_do_not_use_the_per_tick_cap() {
    on_both_stores!(cap_counts_claims_only);
}

async fn failure_is_isolated<S: Store>(store: &S) {
    let world = world();
    let (_, first) = due_countdown_at(store, HOME, 120).await;
    let (_, second) = due_countdown_at(store, HOME, 60).await;
    let once = AtomicBool::new(false);
    // The first post is refused, and meanwhile its row was bound elsewhere,
    // so `retire_rejected` fails after the claim.
    let transport = Intercept::new(&world.fake).on_create(|outcome| {
        let first = first.clone();
        let fire = !once.swap(true, Ordering::SeqCst);
        Box::pin(async move {
            if !fire {
                return outcome;
            }
            let mut ids = RandomIds;
            support::service(store, &mut ids, now())
                .as_origin(kanade::domain::history::Origin::for_tests())
                .mark_reminder_sent(&first, Some("999"))
                .await
                .expect("bound elsewhere");
            Outcome::DefinitelyRejected(RejectionKind::MissingPermissions)
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery
        .dispatch_reminders(now())
        .await
        .expect("tick continues");
    assert!(
        matches!(
            report
                .sends
                .iter()
                .map(|send| &send.outcome)
                .collect::<Vec<_>>()
                .as_slice(),
            [SendOutcome::Failed(_), SendOutcome::Bound(_)]
        ),
        "{:?}",
        report.sends
    );
    assert!(reminder_row(store, &second).await.message_id.is_some());
    let alerts = world.alerts.alerts();
    let [
        AdminAlert::JournalFailure {
            attempt: Some(attempt),
            effect: EffectKind::Reminder,
            ..
        },
    ] = alerts.as_slice()
    else {
        panic!("one journal alert: {alerts:?}");
    };
    // Held until restart recovery; never resent.
    let record = store
        .load_attempt(attempt)
        .await
        .expect("load")
        .expect("attempt");
    assert_eq!(record.state, AttemptState::Intent);
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert_eq!(world.fake.count(Op::Create), 2);
}

#[tokio::test]
async fn one_failed_send_is_alerted_and_the_tick_continues() {
    on_both_stores!(failure_is_isolated);
}

async fn bind_failure<S: Store>(store: &S) {
    let world = world();
    let (_, reminder) = due_countdown(store, HOME).await;
    let transport = Intercept::new(&world.fake).on_create(|outcome| {
        let reminder = reminder.clone();
        Box::pin(async move {
            let mut ids = RandomIds;
            support::service(store, &mut ids, now())
                .as_origin(kanade::domain::history::Origin::for_tests())
                .mark_reminder_sent(&reminder, None)
                .await
                .expect("row changes under the send");
            outcome
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert_eq!(report.sends[0].outcome, SendOutcome::Uncertain);
    let view = store.load_view().await.expect("view");
    assert!(
        view.targets()
            .contains(&DeliveryTarget::Reminder(reminder.clone()))
    );
    for at in tick_times() {
        delivery.tick_at(at).await.expect("tick");
    }
    assert_eq!(world.fake.count(Op::Create), 1, "never resent");
}

#[tokio::test]
async fn bind_failure_is_indeterminate_and_never_resent() {
    on_both_stores!(bind_failure);
}

async fn invalid_channel<S: Store>(store: &S) {
    let mut world = world();
    world.channels.insert("general".into());
    let (_, reminder) = due_countdown(store, "general").await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert_eq!(
        report.sends[0].outcome,
        SendOutcome::Rejected(RejectionKind::Invalid)
    );
    assert_eq!(world.fake.count(Op::Create), 0);
    assert!(reminder_row(store, &reminder).await.sent_at.is_some());
    assert_eq!(
        world.alerts.alerts(),
        vec![AdminAlert::SendRejected {
            effect: EffectKind::Reminder,
            channel_id: "general".into(),
            reason: RejectionKind::Invalid,
        }]
    );
}

#[tokio::test]
async fn invalid_channel_id_is_retired_as_rejected() {
    on_both_stores!(invalid_channel);
}

async fn replacement_refused<S: Store>(store: &S) {
    let world = world();
    seed_replaceable(store, &world).await;
    // Someone else retires the card after Discord confirms the deletion.
    let transport = Intercept::new(&world.fake).on_delete(|outcome| {
        Box::pin(async move {
            let card = store.load_digests().await.expect("log").digests.remove(0);
            with_lease(store, now(), async |lease| {
                store
                    .retire_for_replacement(lease, &card, now())
                    .await
                    .expect("retired elsewhere");
            })
            .await;
            outcome
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery.post_week_digest(now()).await.expect("digest");
    assert!(matches!(
        report.outcome,
        DigestOutcome::ReplacementSuppressed(_)
    ));
    assert_eq!(world.fake.count(Op::Create), 0);
    assert!(matches!(
        world.alerts.alerts().as_slice(),
        [AdminAlert::DigestReplacementSuppressed { .. }]
    ));
}

#[tokio::test]
async fn refused_replacement_retirement_suppresses_the_post() {
    on_both_stores!(replacement_refused);
}

async fn after_cancelled_tick<S: Store>(store: &S) {
    post_then_die(store, &world()).await;
    let world = world();
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery
        .tick_at(now() + TimeDelta::seconds(30))
        .await
        .expect("tick");
    assert_eq!(report.dispatch.sends.len(), 1);
    assert_eq!(report.dispatch.sends[0].outcome, SendOutcome::Suppressed);
    assert_eq!(world.fake.count(Op::Create), 0);
}

#[tokio::test]
async fn a_tick_after_a_cancelled_one_does_not_resend() {
    on_both_stores!(after_cancelled_tick);
}

async fn reaction_failure<S: Store>(store: &S) {
    let world = world();
    let (_, reminder) = due_countdown(store, HOME).await;
    for _ in 0..2 {
        world.fake.script(
            Op::AddReaction,
            Step::Reject(RejectionKind::MissingPermissions),
        );
    }
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    let SendOutcome::Bound(message) = report.sends[0].outcome else {
        panic!("bound: {:?}", report.sends[0].outcome);
    };
    assert_eq!(
        reminder_row(store, &reminder).await.message_id,
        Some(message.get().to_string())
    );
    assert!(world.alerts.alerts().is_empty());
}

#[tokio::test]
async fn failed_reactions_still_count_as_bound() {
    on_both_stores!(reaction_failure);
}

async fn throttled<S: Store>(store: &S) {
    let world = world();
    seed_replaceable(store, &world).await;
    world.fake.set_default(
        Op::Delete,
        Some(Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        }),
    );
    let mut delivery = delivery(store, &world, &world.fake);
    for at in tick_times() {
        delivery.post_week_digest(at).await.expect("digest");
    }
    assert_eq!(world.alerts.alerts().len(), 1, "repeats within the window");
    delivery
        .post_week_digest(now() + ALERT_WINDOW)
        .await
        .expect("digest");
    assert_eq!(world.alerts.alerts().len(), 2, "again after the window");
}

#[tokio::test]
async fn repeated_alerts_are_throttled_per_window() {
    on_both_stores!(throttled);
}

#[test]
fn throttle_keys_by_kind_and_target() {
    let throttle = AlertThrottle::new();
    let rollback = |week: i64| AdminAlert::DigestClockRollback {
        current_week: now() + TimeDelta::days(week),
        last_digest_week: now(),
    };
    assert!(throttle.admit(&rollback(0), now()));
    assert!(!throttle.admit(&rollback(0), now() + TimeDelta::minutes(59)));
    assert!(throttle.admit(&rollback(7), now()), "another target");
    assert!(
        throttle.admit(&rollback(0), now() - TimeDelta::minutes(1)),
        "a clock that went backwards is not trusted to suppress"
    );
}
