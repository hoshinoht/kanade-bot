//! Durable decline notice delivery and retraction on the SQLite store.

use chrono::Duration;
use kanade::bot::mentions::allow_users;
use kanade::bot::transport::{
    AmbiguousKind, Call, FakeDiscord, MessageId, Op, Outcome, RejectionKind,
};
use kanade::domain::history::{ChangeHistory, ChangeMeta, Origin};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::DeclineNotice;
use kanade::domain::schedule::{
    Change, ChangeSet, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
};
use kanade::domain::scheduler::{DeclineNoticeContext, Scope};
use kanade::infrastructure::store::SqliteStore;

use crate::intercept::Intercept;
use crate::scenarios::{HOME, delivery, now, week, world};
use crate::support::Store;

fn later() -> chrono::DateTime<chrono::Utc> {
    now() + Duration::minutes(1)
}

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
    decline_history_uses_rsvp_clock(&SqliteStore::open_temp().await).await;
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

async fn posts_the_notice_as_a_reply<S: Store>(store: &S) {
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
    // The run is at 12:14Z, fourteen minutes from now.
    assert_eq!(
        message.content.as_deref(),
        Some(
            "❌ **Decliner** can't make Kalos <t:1789042440:R>. Eh? Reschedule, or find a \
             stand-in?\n-# for <@1002> · `/amend run_id:decliner` · `/swap`"
        )
    );
    assert_eq!(message.allowed_mentions, allow_users(&["1002".to_owned()]));
    assert!(notice(store, &run, &user).await.message_id.is_some());
}

#[tokio::test]
async fn decline_notice_uses_exact_text_and_source_reply() {
    posts_the_notice_as_a_reply(&SqliteStore::open_temp().await).await;
}

async fn ambiguous_never_reposts<S: Store>(store: &S) {
    let world = world();
    seed(store).await;
    world
        .fake
        .script(Op::Create, crate::support::timed_out(true));
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.drain_decline_notices(now()).await.expect("first");
    delivery
        .drain_decline_notices(later())
        .await
        .expect("replay");
    assert_eq!(world.fake.count(Op::Create), 1);
}

#[tokio::test]
async fn ambiguous_decline_send_is_never_reclaimed() {
    ambiguous_never_reposts(&SqliteStore::open_temp().await).await;
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
        .drain_decline_notices(later())
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
    assert!(notice(store, &run, &user).await.retract_pending);
    // No live port ran; recovery is sufficient to delete the bound message.
    delivery
        .drain_decline_notices(now() + Duration::seconds(2))
        .await
        .expect("recovery delete");
    assert_eq!(world.fake.count(Op::Delete), 1);
}

#[tokio::test]
async fn committed_retraction_survives_an_absent_post_commit_port() {
    committed_retraction_survives_a_missing_post_commit_port(&SqliteStore::open_temp().await).await;
}

#[tokio::test]
async fn confirmed_delete_retires_the_bound_decline_attempt() {
    confirmed_retraction_retires_exact_binding(&SqliteStore::open_temp().await).await;
}

/// The stored notice of `user` on `run`.
async fn notice<S: Store>(store: &S, run: &str, user: &str) -> DeclineNotice {
    store.decline_notice(run, user).await.unwrap().unwrap()
}

/// Creates through `fake` that mark the notice's retraction pending while
/// in flight (the member takes the decline back mid-send), then report
/// `replaced` (`None` keeps Discord's outcome).
fn retracted_mid_send<'a, S: Store>(
    fake: &'a FakeDiscord,
    store: &'a S,
    run: &str,
    user: &str,
    replaced: Option<Outcome<MessageId>>,
) -> Intercept<'a> {
    let (run, user) = (run.to_owned(), user.to_owned());
    Intercept::new(fake).on_create(move |outcome| {
        let (run, user, replaced) = (run.clone(), user.clone(), replaced.clone());
        Box::pin(async move {
            store
                .mark_decline_retract_pending(&run, &user)
                .await
                .expect("mark race");
            replaced.unwrap_or(outcome)
        })
    })
}

async fn unsent_race_resolves_without_repost<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let unsent = Outcome::DefinitelyRejected(RejectionKind::NotSent);
    let transport = retracted_mid_send(&world.fake, store, &run, &user, Some(unsent));
    let mut delivery = delivery(store, &world, &transport);
    delivery
        .drain_decline_notices(now())
        .await
        .expect("send race");
    let notice = notice(store, &run, &user).await;
    assert!(!notice.retract_pending, "the proven-unsent race resolved");
    delivery
        .drain_decline_notices(later())
        .await
        .expect("replay");
    assert_eq!(world.fake.count(Op::Create), 1, "no replacement post");
}

#[tokio::test]
async fn in_flight_retraction_after_an_unsent_create_is_resolved() {
    unsent_race_resolves_without_repost(&SqliteStore::open_temp().await).await;
}

async fn bind_then_confirmed_delete_race<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let transport = retracted_mid_send(&world.fake, store, &run, &user, None);
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery.drain_decline_notices(now()).await.expect("race");
    assert_eq!(report.retracted, 1);
    assert_eq!(world.fake.count(Op::Create), 1);
    assert_eq!(world.fake.count(Op::Delete), 1);
    assert!(notice(store, &run, &user).await.message_id.is_none());
}

#[tokio::test]
async fn in_flight_retraction_binds_then_confirms_delete() {
    bind_then_confirmed_delete_race(&SqliteStore::open_temp().await).await;
}

async fn bind_then_ambiguous_delete_race<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    for _ in 0..2 {
        world
            .fake
            .script(Op::Delete, crate::support::timed_out(false));
    }
    let transport = retracted_mid_send(&world.fake, store, &run, &user, None);
    let mut delivery = delivery(store, &world, &transport);
    let report = delivery.drain_decline_notices(now()).await.expect("race");
    assert_eq!(report.retracted, 0);
    assert_eq!(world.fake.count(Op::Delete), 2, "one identical retry");
    delivery
        .drain_decline_notices(later())
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
    bind_then_ambiguous_delete_race(&SqliteStore::open_temp().await).await;
}

async fn ambiguous_send_race<S: Store>(store: &S) {
    let world = world();
    let (run, user) = seed(store).await;
    let ambiguous = Outcome::Ambiguous(AmbiguousKind::Timeout);
    let transport = retracted_mid_send(&world.fake, store, &run, &user, Some(ambiguous));
    let mut delivery = delivery(store, &world, &transport);
    delivery.drain_decline_notices(now()).await.expect("race");
    assert!(notice(store, &run, &user).await.retract_pending);
    delivery
        .drain_decline_notices(later())
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
    ambiguous_send_race(&SqliteStore::open_temp().await).await;
}

async fn cooldown_survives_recovery<S: Store>(store: &S) {
    let (run, user) = seed(store).await;
    store.recover_on_start(later()).await.expect("recover");
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
    cooldown_survives_recovery(&SqliteStore::open_temp().await).await;
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
    store.recover_on_start(later()).await.expect("recover");
    delivery
        .drain_decline_notices(later())
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
    assert!(notice(store, &run, &second).await.message_id.is_some());
}

#[tokio::test]
async fn restart_recovery_drains_unbound_and_bound_retraction_without_duplicates() {
    recovery_drains_unbound_and_bound_pending_once(&SqliteStore::open_temp().await).await;
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
        .drain_decline_notices(later())
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
    shutdown_deadline_cancels_the_actual_decline_drain(&SqliteStore::open_temp().await).await;
}
