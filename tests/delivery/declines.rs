//! Durable decline notice delivery and retraction over both stores.

use chrono::Duration;
use kanade::bot::mentions::allow_users;
use kanade::bot::transport::{AmbiguousKind, Call, Op, RejectionKind, Step};
use kanade::domain::history::{ChangeHistory, ChangeMeta, Origin};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::DeclineNotice;
use kanade::domain::schedule::{
    Change, ChangeSet, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
};
use kanade::domain::scheduler::{DeclineNoticeContext, Scope};
use kanade::domain::settings::MessageStyle;

use crate::intercept::Intercept;
use crate::redesign::notices::styled;
use crate::scenarios::{HOME, delivery, now, week, world};
use crate::support::{Store, on_both_stores};

async fn seed<S: Store>(store: &S) -> (String, String) {
    let run_id = "decline-run".to_owned();
    let user_id = "1001".to_owned();
    store
        .commit_with_decline_notices(
            0,
            ChangeSet {
                changes: vec![Change::PutRun(Run {
                    id: run_id.clone(),
                    fixed_run_id: None,
                    channel_id: Some(HOME.into()),
                    week_start: week(),
                    datetime: now() + Duration::minutes(14),
                    bosses: vec!["Kalos".into()],
                    participants: vec![user_id.clone(), "1002".into()],
                    status: RunStatus::Planned,
                    source: RunSource::Amend,
                    attendance: Vec::new(),
                    status_pin: None,
                })],
            },
            meta(),
            vec![DeclineNotice::candidate(
                &run_id,
                &user_id,
                Some(HOME.into()),
                Some("700000000000000009".into()),
                "Decliner",
                now(),
            )],
            Vec::new(),
        )
        .await
        .expect("seed candidate");
    (run_id, user_id)
}

fn meta() -> ChangeMeta {
    ChangeMeta {
        origin: Origin::for_tests(),
        at: now(),
        notices: Vec::new(),
        refs: Vec::new(),
        request_digest: None,
        expect: Default::default(),
        outbox: Vec::new(),
    }
}

async fn decline_history_uses_rsvp_clock<S: Store + ChangeHistory>(store: &S) {
    let (run_id, _) = seed(store).await;
    let mut ids = RandomIds;
    let result = crate::support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .apply_reaction_with_decline(
            &run_id,
            "1002",
            "❌",
            true,
            DeclineNoticeContext {
                channel_id: Some(HOME.into()),
                reference_id: None,
                display_name: "Second decliner".into(),
            },
        )
        .await
        .expect("committed decline");
    assert!(result.declined);

    let head = store.history_head().await.expect("history head");
    let record = store
        .load_change(head.seq)
        .await
        .expect("load decline history")
        .expect("decline record");
    assert_eq!(record.at, now(), "history records the RSVP clock");
}

#[tokio::test]
async fn committed_decline_history_uses_the_rsvp_clock() {
    on_both_stores!(decline_history_uses_rsvp_clock);
}

async fn seed_second_candidate<S: Store>(store: &S, run_id: &str) -> String {
    let revision = store.load(&Scope::All).await.expect("snapshot").revision;
    let user_id = "1002".to_owned();
    store
        .commit_with_decline_notices(
            revision,
            ChangeSet {
                changes: vec![Change::PutRsvp(Rsvp {
                    run_id: run_id.into(),
                    user_id: user_id.clone(),
                    state: RsvpState::No,
                    source: RsvpSource::Reaction,
                    at: now(),
                })],
            },
            meta(),
            vec![DeclineNotice::candidate(
                run_id,
                &user_id,
                Some(HOME.into()),
                None,
                "Second decliner",
                now(),
            )],
            Vec::new(),
        )
        .await
        .expect("second candidate");
    user_id
}

async fn posts_v4_text_as_a_reply<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.drain_decline_notices(now()).await.expect("drain");
    assert_eq!(report.sends.len(), 1);
    let calls = world.fake.calls();
    let [
        Call::Create {
            channel, message, ..
        },
    ] = calls.as_slice()
    else {
        panic!("one create: {:?}", world.fake.calls());
    };
    assert_eq!(channel.get(), HOME.parse::<u64>().unwrap());
    assert_eq!(message.reply_to.expect("reply").get(), 700000000000000009);
    assert_eq!(
        message.content.as_deref(),
        Some(
            "<@1002> Decliner can't make **Kalos** (Thu 10 Sep 20:14) — reschedule? `/amend run_id:decliner to:...`"
        )
    );
    assert!(
        store
            .decline_notice(&run, &user)
            .await
            .unwrap()
            .unwrap()
            .message_id
            .is_some()
    );
}

#[tokio::test]
async fn decline_notice_uses_exact_text_and_source_reply() {
    on_both_stores!(posts_v4_text_as_a_reply);
}

/// The decline notice in `style`, posted as a reply to the decline; the
/// redesign leads with the decliner and tells the rest in subtext, pinging
/// the same people.
async fn posts_in_style<S: Store>(store: &S, style: MessageStyle) {
    let world = world();
    seed(store).await;
    let mut delivery = delivery(store, &world, &world.fake).with_cards(styled(style));
    delivery.drain_decline_notices(now()).await.expect("drain");
    let calls = world.fake.calls();
    let [Call::Create { message, .. }] = calls.as_slice() else {
        panic!("one create: {calls:?}");
    };
    assert_eq!(message.reply_to.expect("reply").get(), 700000000000000009);
    let expected = match style {
        MessageStyle::Classic => {
            "<@1002> Decliner can't make **Kalos** (Thu 10 Sep 20:14) — reschedule? \
             `/amend run_id:decliner to:...`"
        }
        // The run is at 12:14Z, fourteen minutes from now.
        MessageStyle::Redesigned => {
            "❌ **Decliner** can't make Kalos <t:1789042440:R>. Eh? Reschedule, or find a \
             stand-in?\n-# for <@1002> · `/amend run_id:decliner` · `/swap`"
        }
    };
    assert_eq!(message.content.as_deref(), Some(expected));
    assert_eq!(message.allowed_mentions, allow_users(&["1002".to_owned()]));
}

#[tokio::test]
async fn decline_notice_follows_the_live_message_style() {
    on_both_stores!(posts_in_style, MessageStyle::Classic);
    on_both_stores!(posts_in_style, MessageStyle::Redesigned);
}

async fn ambiguous_never_reposts<S: Store>(store: &S) {
    let world = world();
    seed(store).await;
    world.fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.drain_decline_notices(now()).await.expect("first");
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("replay");
    assert_eq!(world.fake.count(Op::Create), 1);
}

#[tokio::test]
async fn ambiguous_decline_send_is_never_reclaimed() {
    on_both_stores!(ambiguous_never_reposts);
}

async fn confirmed_retraction_retires_exact_binding<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.drain_decline_notices(now()).await.expect("send");
    assert!(
        delivery
            .retract_decline_notice(&run, &user, now() + Duration::seconds(1))
            .await
            .expect("retract")
    );
    let notice = store.decline_notice(&run, &user).await.unwrap().unwrap();
    assert!(notice.message_id.is_none());
    assert!(!notice.retract_pending);
    assert_eq!(world.fake.count(Op::Delete), 1);
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("replay");
    assert_eq!(
        world.fake.count(Op::Create),
        1,
        "a cleared binding is not reposted"
    );
}

async fn committed_retraction_survives_a_missing_post_commit_port<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.drain_decline_notices(now()).await.expect("bind");
    let revision = store.load(&Scope::All).await.expect("snapshot").revision;
    store
        .commit_with_decline_notices(
            revision,
            ChangeSet {
                changes: vec![Change::PutRsvp(Rsvp {
                    run_id: run.clone(),
                    user_id: user.clone(),
                    state: RsvpState::Yes,
                    source: RsvpSource::Chat,
                    at: now() + Duration::seconds(1),
                })],
            },
            meta(),
            Vec::new(),
            vec![(run.clone(), user.clone())],
        )
        .await
        .expect("commits RSVP and retraction together");
    assert!(
        store
            .decline_notice(&run, &user)
            .await
            .unwrap()
            .unwrap()
            .retract_pending
    );
    // No live port ran; recovery is sufficient to delete the bound message.
    delivery
        .drain_decline_notices(now() + Duration::seconds(2))
        .await
        .expect("recovery delete");
    assert_eq!(world.fake.count(Op::Delete), 1);
}

#[tokio::test]
async fn committed_retraction_survives_an_absent_post_commit_port() {
    on_both_stores!(committed_retraction_survives_a_missing_post_commit_port);
}

#[tokio::test]
async fn confirmed_delete_retires_the_bound_decline_attempt() {
    on_both_stores!(confirmed_retraction_retires_exact_binding);
}

async fn unsent_race_resolves_without_repost<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let transport = Intercept::new(&world.fake).on_create(|_| {
        let run = run.clone();
        let user = user.clone();
        Box::pin(async move {
            store
                .mark_decline_retract_pending(&run, &user)
                .await
                .expect("mark race");
            kanade::bot::transport::Outcome::DefinitelyRejected(RejectionKind::NotSent)
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    delivery
        .drain_decline_notices(now())
        .await
        .expect("send race");
    let notice = store.decline_notice(&run, &user).await.unwrap().unwrap();
    assert!(!notice.retract_pending, "the proven-unsent race resolved");
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("replay");
    assert_eq!(world.fake.count(Op::Create), 1, "no replacement post");
}

#[tokio::test]
async fn in_flight_retraction_after_an_unsent_create_is_resolved() {
    on_both_stores!(unsent_race_resolves_without_repost);
}

async fn bind_then_confirmed_delete_race<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let transport = Intercept::new(&world.fake).on_create(|outcome| {
        let run = run.clone();
        let user = user.clone();
        Box::pin(async move {
            store
                .mark_decline_retract_pending(&run, &user)
                .await
                .expect("mark race");
            outcome
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery.drain_decline_notices(now()).await.expect("race");
    assert_eq!(report.retracted, 1);
    assert_eq!(world.fake.count(Op::Create), 1);
    assert_eq!(world.fake.count(Op::Delete), 1);
    assert!(
        store
            .decline_notice(&run, &user)
            .await
            .unwrap()
            .unwrap()
            .message_id
            .is_none()
    );
}

#[tokio::test]
async fn in_flight_retraction_binds_then_confirms_delete() {
    on_both_stores!(bind_then_confirmed_delete_race);
}

async fn bind_then_ambiguous_delete_race<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    world.fake.script(
        Op::Delete,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    world.fake.script(
        Op::Delete,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    let transport = Intercept::new(&world.fake).on_create(|outcome| {
        let run = run.clone();
        let user = user.clone();
        Box::pin(async move {
            store
                .mark_decline_retract_pending(&run, &user)
                .await
                .expect("mark race");
            outcome
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery.drain_decline_notices(now()).await.expect("race");
    assert_eq!(report.retracted, 0);
    assert_eq!(world.fake.count(Op::Delete), 2, "one identical retry");
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("replay");
    assert_eq!(
        world.fake.count(Op::Create),
        1,
        "bound claim suppresses replacement"
    );
}

#[tokio::test]
async fn in_flight_retraction_binds_then_retries_ambiguous_delete_once() {
    on_both_stores!(bind_then_ambiguous_delete_race);
}

async fn ambiguous_send_race<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let transport = Intercept::new(&world.fake).on_create(|_| {
        let run = run.clone();
        let user = user.clone();
        Box::pin(async move {
            store
                .mark_decline_retract_pending(&run, &user)
                .await
                .expect("mark race");
            kanade::bot::transport::Outcome::Ambiguous(AmbiguousKind::Timeout)
        })
    });
    let mut delivery = delivery(store, &world, &transport);
    delivery.drain_decline_notices(now()).await.expect("race");
    let notice = store.decline_notice(&run, &user).await.unwrap().unwrap();
    assert!(notice.retract_pending);
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("replay");
    assert_eq!(
        world.fake.count(Op::Create),
        1,
        "ambiguous send is never replaced"
    );
}

#[tokio::test]
async fn in_flight_retraction_after_an_ambiguous_create_never_reposts() {
    on_both_stores!(ambiguous_send_race);
}

async fn cooldown_survives_recovery<S: Store>(store: &S) {
    let (run, user) = seed(store).await;
    store
        .recover_on_start(now() + Duration::minutes(1))
        .await
        .expect("recover");
    assert!(
        store
            .decline_notice_on_cooldown(&run, &user, now() + Duration::hours(5))
            .await
            .unwrap()
    );
    assert!(
        !store
            .decline_notice_on_cooldown(&run, &user, now() + Duration::hours(6))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn decline_cooldown_survives_restart_recovery() {
    on_both_stores!(cooldown_survives_recovery);
}

async fn recovery_drains_unbound_and_bound_pending_once<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery
        .drain_decline_notices(now())
        .await
        .expect("bind first");
    let second = seed_second_candidate(store, &run).await;
    store
        .mark_decline_retract_pending(&run, &user)
        .await
        .expect("mark bound");
    store
        .recover_on_start(now() + Duration::minutes(1))
        .await
        .expect("recover");
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("recovery tick");
    delivery
        .drain_decline_notices(now() + Duration::minutes(2))
        .await
        .expect("replay tick");
    assert_eq!(
        world.fake.count(Op::Create),
        2,
        "one initial and one recovered candidate"
    );
    assert_eq!(world.fake.count(Op::Delete), 1, "one recovered retraction");
    assert!(
        store
            .decline_notice(&run, &second)
            .await
            .unwrap()
            .unwrap()
            .message_id
            .is_some()
    );
}

#[tokio::test]
async fn restart_recovery_drains_unbound_and_bound_retraction_without_duplicates() {
    on_both_stores!(recovery_drains_unbound_and_bound_pending_once);
}

async fn shutdown_deadline_cancels_the_actual_decline_drain<S: Store>(store: &S) {
    let world = world();
    seed(store).await;
    let hold = world.fake.hold(Op::Create);
    let mut delivery = delivery(store, &world, &world.fake);
    let mut drain = Box::pin(delivery.drain_decline_notices(now()));
    tokio::select! {
        () = hold.entered() => {}
        _ = &mut drain => panic!("the held drain completed"),
    }
    // This is the same cancellation a bounded shutdown applies to the actual
    // drain future. Dropping a wrapper would detach it instead.
    drop(drain);
    store
        .recover_on_start(now() + Duration::seconds(1))
        .await
        .expect("recover");
    delivery
        .drain_decline_notices(now() + Duration::minutes(1))
        .await
        .expect("replay");
    assert_eq!(
        world.fake.count(Op::Create),
        0,
        "recovery never replays the cancelled claim"
    );
}

#[tokio::test]
async fn shutdown_deadline_cancels_the_actual_decline_drain_before_store_close() {
    on_both_stores!(shutdown_deadline_cancels_the_actual_decline_drain);
}
