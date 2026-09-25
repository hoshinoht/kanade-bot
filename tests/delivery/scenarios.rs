//! Exactly-once / no-replay scenarios, each on the memory and SQLite stores.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::bot::delivery::{
    AdminAlert, AlertRecorder, DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, DigestOutcome,
    FixedClock, SendOutcome, StoreRef,
};
use kanade::bot::events::{ReactionRouter, RsvpAnswer, RsvpReaction};
use kanade::bot::transport::{
    AmbiguousKind, Call, DiscordTransport, FakeDiscord, Op, RejectionKind, Step,
};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::notify::{AttemptState, EffectKind};
use kanade::domain::schedule::{
    NewRun, ReminderPolicy, RsvpState, RunSource, RunStatus, SchedulePolicy,
};
use kanade::domain::scheduler::SchedulerService;
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::id::Id;

use crate::intercept::Intercept;
use crate::support::{self, Store, TempDir, on_both_stores, seed_digest, with_lease};

pub(crate) const HOME: &str = "222";
pub(crate) const POST: &str = "555";

/// Thursday 2026-09-10 20:00 in Kuala Lumpur; its boss week began at the
/// Wednesday reset, 2026-09-08T16:00Z.
pub(crate) fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap()
}

pub(crate) fn week() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 8, 16, 0, 0).unwrap()
}

pub(crate) fn previous_week() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 16, 0, 0).unwrap()
}

pub(crate) fn config() -> DeliveryConfig {
    DeliveryConfig {
        instance_id: support::INSTANCE.into(),
        policy: SchedulePolicy::new(
            ReminderPolicy {
                zone: chrono_tz::Asia::Kuala_Lumpur,
                ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60, 15],
            },
            Weekday::Wed,
            NaiveTime::MIN,
        ),
        post_channel_id: Some(POST.into()),
        quiet_mode: false,
        max_sends_per_tick: DEFAULT_MAX_SENDS_PER_TICK,
    }
}

pub(crate) struct World {
    pub roster: Roster,
    pub channels: BTreeSet<String>,
    pub fake: FakeDiscord,
    pub alerts: AlertRecorder,
}

pub(crate) fn world() -> World {
    let mut roster = Roster::new();
    for user in ["1001", "1002"] {
        roster.upsert(Member {
            user_id: user.into(),
            has_role: true,
            ping_level: PingLevel::All,
            ..Member::default()
        });
    }
    World {
        roster,
        channels: [HOME, POST].into_iter().map(str::to_owned).collect(),
        fake: support::fake(),
        alerts: AlertRecorder::new(),
    }
}

pub(crate) type TestDelivery<'a, S, T> = Delivery<'a, S, RandomIds, T, AlertRecorder>;

pub(crate) fn delivery<'a, S: Store, T: DiscordTransport>(
    store: &'a S,
    world: &'a World,
    transport: &'a T,
) -> TestDelivery<'a, S, T> {
    Delivery::new(
        store,
        RandomIds,
        transport,
        &world.alerts,
        &world.roster,
        &world.channels,
        config(),
    )
}

/// A run starting in 14 minutes in `channel` with its 15-minute countdown due.
pub(crate) async fn due_countdown<S: Store>(store: &S, channel: &str) -> (String, String) {
    due_countdown_at(store, channel, 60).await
}

/// As [`due_countdown`], the countdown due `seconds_ago` (orders due rows).
pub(crate) async fn due_countdown_at<S: Store>(
    store: &S,
    channel: &str,
    seconds_ago: i64,
) -> (String, String) {
    let mut ids = RandomIds;
    let mut service = support::service(store, &mut ids, now());
    let run = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(channel.into()),
            week_start: week(),
            datetime: now() + chrono::Duration::minutes(14),
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Fixed,
        })
        .await
        .expect("run");
    let reminder = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_reminder(
            &run,
            "countdown_15",
            now() - chrono::Duration::seconds(seconds_ago),
            None,
        )
        .await
        .expect("reminder")
        .expect("new reminder");
    (run, reminder)
}

pub(crate) async fn reminder_row<S: Store>(
    store: &S,
    id: &str,
) -> kanade::domain::schedule::Reminder {
    support::snapshot(store)
        .await
        .reminders
        .into_iter()
        .find(|row| row.id == id)
        .expect("reminder row")
}

pub(crate) fn tick_times() -> [DateTime<Utc>; 3] {
    [0, 30, 60].map(|seconds| now() + chrono::Duration::seconds(seconds))
}

async fn ambiguous_send_is_sent_once<S: Store>(store: &S) {
    let world = world();
    let (_, reminder) = due_countdown(store, HOME).await;
    world.fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    let mut delivery = delivery(store, &world, &world.fake);
    let mut outcomes = Vec::new();
    for at in tick_times() {
        let report = delivery.tick_at(at).await.expect("tick");
        outcomes.extend(report.dispatch.sends.into_iter().map(|send| send.outcome));
    }
    assert_eq!(
        outcomes,
        vec![
            SendOutcome::Uncertain,
            SendOutcome::Suppressed,
            SendOutcome::Suppressed
        ]
    );
    assert_eq!(world.fake.count(Op::Create), 1, "never resent");
    assert_eq!(reminder_row(store, &reminder).await.sent_at, None);
}

#[tokio::test]
async fn ambiguous_send_is_one_transport_call_across_three_ticks() {
    on_both_stores!(ambiguous_send_is_sent_once);
}

async fn unsent_is_retried<S: Store>(store: &S, kind: RejectionKind) {
    let world = world();
    let (_, reminder) = due_countdown(store, HOME).await;
    world.fake.script(Op::Create, Step::Reject(kind.clone()));
    let mut delivery = delivery(store, &world, &world.fake);
    let mut outcomes = Vec::new();
    for at in tick_times() {
        let report = delivery.tick_at(at).await.expect("tick");
        outcomes.extend(report.dispatch.sends.into_iter().map(|send| send.outcome));
    }
    let [first, SendOutcome::Bound(message)] = outcomes.as_slice() else {
        panic!("released, then bound once: {outcomes:?}");
    };
    assert_eq!(*first, SendOutcome::Released(kind));
    assert_eq!(world.fake.count(Op::Create), 2);
    let row = reminder_row(store, &reminder).await;
    assert_eq!(row.message_id, Some(message.get().to_string()));
    assert!(world.alerts.alerts().is_empty(), "not an operator problem");
}

#[tokio::test]
async fn not_sent_and_rate_limited_are_retried_next_tick_exactly_once() {
    on_both_stores!(unsent_is_retried, RejectionKind::NotSent);
    on_both_stores!(unsent_is_retried, RejectionKind::RateLimited);
}

async fn rejected_is_retired<S: Store>(store: &S) {
    let world = world();
    let (_, reminder) = due_countdown(store, HOME).await;
    world
        .fake
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let mut delivery = delivery(store, &world, &world.fake);
    for at in tick_times() {
        delivery.tick_at(at).await.expect("tick");
    }
    assert_eq!(world.fake.count(Op::Create), 1, "never retried");
    let row = reminder_row(store, &reminder).await;
    assert!(row.sent_at.is_some() && row.message_id.is_none());
    assert_eq!(
        world.alerts.alerts(),
        vec![AdminAlert::SendRejected {
            effect: EffectKind::Reminder,
            channel_id: HOME.into(),
            reason: RejectionKind::MissingPermissions,
        }]
    );
}

#[tokio::test]
async fn definite_rejection_is_retired_with_an_alert() {
    on_both_stores!(rejected_is_retired);
}

/// Returns the reminder id; Discord has the message, the journal an intent.
pub(crate) async fn post_then_die<S: Store>(store: &S, world: &World) -> String {
    let (_, reminder) = due_countdown(store, HOME).await;
    // Posts, then never returns: the process "dies" between Discord
    // accepting the message and the journal binding it.
    let dying = Intercept::new(&world.fake).on_create(|_| Box::pin(std::future::pending()));
    let mut delivery = delivery(store, world, &dying);
    let crashed = tokio::time::timeout(Duration::from_millis(200), delivery.tick_at(now())).await;
    assert!(crashed.is_err(), "the tick never finished");
    assert_eq!(world.fake.count(Op::Create), 1);
    reminder
}

async fn after_restart<S: Store>(store: &S, reminder: &str) {
    let world = world();
    let mut delivery = delivery(store, &world, &world.fake);
    let recovery = delivery.start(now()).await.expect("recover");
    assert_eq!(recovery.indeterminate.len(), 1);
    let attempt = store
        .load_attempt(&recovery.indeterminate[0])
        .await
        .expect("load")
        .expect("attempt");
    assert_eq!(attempt.state, AttemptState::Indeterminate);
    for at in tick_times() {
        let report = delivery.tick_at(at).await.expect("tick");
        assert!(
            report
                .dispatch
                .sends
                .iter()
                .all(|send| send.outcome == SendOutcome::Suppressed)
        );
    }
    assert_eq!(
        world.fake.count(Op::Create),
        0,
        "never resent after restart"
    );
    assert_eq!(reminder_row(store, reminder).await.sent_at, None);
}

#[tokio::test]
async fn crash_between_delivery_and_bind_is_never_resent() {
    let memory = MemoryScheduleStore::new();
    let reminder = post_then_die(&memory, &world()).await;
    after_restart(&memory, &reminder).await;

    let dir = TempDir::new();
    let sqlite = dir.open().await;
    let reminder = post_then_die(&sqlite, &world()).await;
    sqlite.close().await.expect("close");
    let reopened = dir.open().await;
    after_restart(&reopened, &reminder).await;
    reopened.close().await.expect("close");
}

async fn fallback_binds_and_maps_reactions<S: Store>(store: &S) {
    let world = world();
    let (run, reminder) = due_countdown(store, "444").await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    let [send] = report.sends.as_slice() else {
        panic!("one send");
    };
    let SendOutcome::Bound(message) = send.outcome else {
        panic!("bound: {:?}", send.outcome);
    };
    assert_eq!(send.intent.channel_id, POST);
    let calls = world.fake.calls();
    assert!(matches!(&calls[0], Call::Create { channel, .. } if channel.get() == 555));
    assert_eq!(
        world.alerts.alerts(),
        vec![AdminAlert::HomeChannelUnavailable {
            home_channel_id: Some("444".into()),
            used_channel_id: POST.into(),
            run_ids: vec![run.clone()],
        }]
    );
    assert_eq!(
        reminder_row(store, &reminder).await.message_id,
        Some(message.get().to_string())
    );
    assert_eq!(world.fake.count(Op::AddReaction), 2, "✅ and ❌ added");

    assert_eq!(
        store.runs_for_message(message).await.expect("index"),
        vec![run.clone()]
    );
    let sink = SchedulerService::new(StoreRef(store), RandomIds, FixedClock(now()));
    let mut router = ReactionRouter::new(StoreRef(store), sink);
    let results = router
        .route(&RsvpReaction {
            channel_id: Id::new(555),
            message_id: message,
            user_id: Id::new(1001),
            answer: RsvpAnswer::Yes,
            added: true,
        })
        .await
        .expect("route");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].state, Some(RsvpState::Yes));
}

#[tokio::test]
async fn fallback_channel_is_bound_and_its_card_maps_reactions() {
    on_both_stores!(fallback_binds_and_maps_reactions);
}

async fn bounded<S: Store>(store: &S) {
    let world = world();
    due_countdown(store, HOME).await;
    due_countdown(store, HOME).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.config.max_sends_per_tick = 1;
    let first = delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert_eq!((first.sends.len(), first.deferred), (1, 1));
    let second = delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert_eq!((second.sends.len(), second.deferred), (1, 0));
    assert_eq!(world.fake.count(Op::Create), 2);
}

#[tokio::test]
async fn work_per_tick_is_bounded() {
    on_both_stores!(bounded);
}

async fn quiet<S: Store>(store: &S) {
    let world = world();
    due_countdown(store, HOME).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.config.quiet_mode = true;
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let calls = world.fake.calls();
    let Call::Create { message, .. } = &calls[0] else {
        panic!("a post");
    };
    assert!(
        !message
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("<@")
    );
    assert_eq!(
        serde_json::to_value(&message.allowed_mentions).unwrap(),
        serde_json::json!({ "parse": [] })
    );
}

#[tokio::test]
async fn quiet_mode_posts_mention_nobody() {
    on_both_stores!(quiet);
}

/// A card for this week exists; the marker says last week was the last post.
pub(crate) async fn seed_replaceable<S: Store>(store: &S, world: &World) {
    with_lease(store, now(), async |lease| {
        store
            .record_digest_week(lease, previous_week(), now())
            .await
            .expect("marker");
    })
    .await;
    seed_digest(store, week(), POST, "7001", now()).await;
    world.fake.seed_message(Id::new(555), Id::new(7001));
}

async fn replacement_after_confirmed_deletion<S: Store>(store: &S) {
    let world = world();
    seed_replaceable(store, &world).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.post_week_digest(now()).await.expect("digest");
    let message = report.message_id().expect("posted");
    let ops: Vec<Op> = world.fake.calls().iter().map(Call::op).collect();
    assert_eq!(ops, vec![Op::Delete, Op::Create]);
    let log = store.load_digests().await.expect("log");
    assert_eq!(
        log.last_digest_week.as_deref(),
        Some("2026-09-08T16:00:00+00:00")
    );
    let [card] = log.digests.as_slice() else {
        panic!("one card per week: {:?}", log.digests);
    };
    assert_eq!(
        (card.message_id.as_str(), card.retired_at),
        (message.as_str(), None)
    );
}

#[tokio::test]
async fn digest_is_replaced_only_after_confirmed_deletion() {
    on_both_stores!(replacement_after_confirmed_deletion);
}

async fn ambiguous_deletion_suppresses<S: Store>(store: &S) {
    let world = world();
    seed_replaceable(store, &world).await;
    world.fake.script(
        Op::Delete,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.post_week_digest(now()).await.expect("digest");
    assert!(matches!(
        report.outcome,
        DigestOutcome::ReplacementSuppressed(_)
    ));
    assert_eq!(world.fake.count(Op::Create), 0);
    let log = store.load_digests().await.expect("log");
    assert_eq!(log.digests[0].message_id, "7001");
    assert_eq!(log.digests[0].retired_at, None);
    assert!(matches!(
        world.alerts.alerts().as_slice(),
        [AdminAlert::DigestReplacementSuppressed { message_id, .. }] if message_id == "7001"
    ));

    // An ambiguous deletion that did land is confirmed by a fetch.
    world.fake.script(
        Op::Delete,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    let report = delivery.post_week_digest(now()).await.expect("digest");
    assert!(report.message_id().is_some());
    assert_eq!(world.fake.count(Op::Presence), 2);
}

#[tokio::test]
async fn ambiguous_digest_deletion_suppresses_the_replacement() {
    on_both_stores!(ambiguous_deletion_suppresses);
}

async fn rollback<S: Store>(store: &S) {
    let world = world();
    with_lease(store, now(), async |lease| {
        store
            .record_digest_week(lease, week(), now())
            .await
            .expect("marker");
    })
    .await;
    let mut delivery = delivery(store, &world, &world.fake);
    let earlier = now() - chrono::Duration::days(7);
    let report = delivery.tick_at(earlier).await.expect("tick");
    assert_eq!(report.digest.outcome, DigestOutcome::ClockRolledBack);
    assert_eq!(world.fake.count(Op::Create), 0);
    assert_eq!(
        world.alerts.alerts(),
        vec![AdminAlert::DigestClockRollback {
            current_week: previous_week(),
            last_digest_week: week(),
        }]
    );
    let log = store.load_digests().await.expect("log");
    assert_eq!(
        log.last_digest_week.as_deref(),
        Some("2026-09-08T16:00:00+00:00")
    );
}

#[tokio::test]
async fn clock_rollback_posts_nothing() {
    on_both_stores!(rollback);
}
