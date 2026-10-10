use super::*;

use std::{
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::sync::{Mutex, oneshot};

use crate::{
    bot::{
        delivery::{DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, SendOutcome},
        gateway::ConnectionStatus,
        transport::{
            ChannelId, DiscordTransport, HistoryPage, InteractionRef, InteractionReply,
            MessageEdit, MessageId, Op, Outcome, OutgoingMessage, Presence,
        },
    },
    domain::{
        members::Roster,
        notify::{
            Claim, DeliveryTarget, DigestInclusion, EffectKind, IntentContent, NotificationIntent,
            Receipt,
        },
    },
};

async fn add_due_reminder(
    store: &SqliteStore,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> String {
    let run = create_run(
        store,
        policy,
        now,
        HOME_C,
        now + chrono::Duration::minutes(14),
    )
    .await;
    let reminder = SchedulerService::new(store, RandomIds, FixedClock(now))
        .with_attendance(policy.attendance)
        .as_origin(Origin::for_tests())
        .add_reminder(
            &run,
            "countdown_15",
            now - chrono::Duration::minutes(1),
            None,
        )
        .await
        .unwrap();
    reminder.expect("due reminder")
}

fn delivery_config(policy: SchedulePolicy, post_channel_id: Option<String>) -> DeliveryConfig {
    DeliveryConfig {
        instance_id: "cancel-safety-test".into(),
        policy,
        post_channel_id,
        quiet_mode: false,
        max_sends_per_tick: DEFAULT_MAX_SENDS_PER_TICK,
        max_notice_age: crate::domain::notify::DEFAULT_MAX_NOTICE_AGE,
        run_lengths: crate::domain::settings::RunLengths::default(),
        freeze_ended: true,
    }
}

fn gated_delivery<'a, T: DiscordTransport>(
    store: &'a SqliteStore,
    transport: &'a T,
    alerts: &'a LogAlerts,
    members: &'a Roster,
    channels: &'a BTreeSet<String>,
    connection: ConnectionStatus,
    config: DeliveryConfig,
) -> Delivery<'a, SqliteStore, RandomIds, T, LogAlerts> {
    Delivery::new(
        store, RandomIds, transport, alerts, members, channels, config,
    )
    .with_admission_gate(move || connection.delivery_eligibility())
}

async fn open_recovered_store(harness: &Harness, now: DateTime<Utc>) -> Arc<SqliteStore> {
    let store = store::open(&harness.config.store).await.unwrap();
    super::super::tick::recover(&store, now).await.unwrap();
    store
}

struct Hold {
    entered: StdMutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl Hold {
    fn new() -> (Arc<Self>, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, reached) = oneshot::channel();
        let (resume, release) = oneshot::channel();
        (
            Arc::new(Self {
                entered: StdMutex::new(Some(entered)),
                release: Mutex::new(Some(release)),
            }),
            reached,
            resume,
        )
    }

    async fn wait(&self) {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        if let Some(release) = self.release.lock().await.take() {
            let _ = release.await;
        }
    }
}

type HookFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

fn hold_hook(hold: Arc<Hold>) -> impl Fn() -> HookFuture + Send + Sync + 'static {
    move || {
        let hold = Arc::clone(&hold);
        Box::pin(async move { hold.wait().await })
    }
}

struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct BlockedTransport {
    fake: Arc<FakeDiscord>,
    blocked: Op,
    hold: Arc<Hold>,
    in_flight: Arc<AtomicUsize>,
}

impl BlockedTransport {
    async fn pause_after_effect(&self, op: Op) {
        if self.blocked == op {
            self.in_flight.fetch_add(1, Ordering::SeqCst);
            let _in_flight = InFlight(Arc::clone(&self.in_flight));
            self.hold.wait().await;
        }
    }
}

impl DiscordTransport for BlockedTransport {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        let outcome = self.fake.create_message(channel, message).await;
        self.pause_after_effect(Op::Create).await;
        outcome
    }

    fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.edit_message(channel, message, edit)
    }

    async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        let outcome = self.fake.delete_message(channel, message).await;
        self.pause_after_effect(Op::Delete).await;
        outcome
    }

    fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.add_own_reaction(channel, message, emoji)
    }

    fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.remove_own_reaction(channel, message, emoji)
    }

    fn message_presence(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl Future<Output = Outcome<Presence>> + Send {
        self.fake.message_presence(channel, message)
    }

    fn respond(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.respond(interaction, reply)
    }

    fn defer(
        &self,
        interaction: &InteractionRef,
        ephemeral: bool,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.defer(interaction, ephemeral)
    }

    fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.complete_deferred(interaction, reply)
    }

    fn register_guild_commands(
        &self,
        guild: twilight_model::id::Id<twilight_model::id::marker::GuildMarker>,
        commands: &[twilight_model::application::command::Command],
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.register_guild_commands(guild, commands)
    }

    fn list_members(
        &self,
        guild: twilight_model::id::Id<twilight_model::id::marker::GuildMarker>,
        after: Option<twilight_model::id::Id<twilight_model::id::marker::UserMarker>>,
        limit: u16,
    ) -> impl Future<Output = Outcome<Vec<twilight_model::guild::Member>>> + Send {
        self.fake.list_members(guild, after, limit)
    }

    fn channel_messages(
        &self,
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
    ) -> impl Future<Output = Outcome<Vec<twilight_model::channel::Message>>> + Send {
        self.fake.channel_messages(channel, page, limit)
    }

    fn guild_channels(
        &self,
        guild: twilight_model::id::Id<twilight_model::id::marker::GuildMarker>,
    ) -> impl Future<Output = Outcome<Vec<twilight_model::channel::Channel>>> + Send {
        self.fake.guild_channels(guild)
    }
}

fn test_connection() -> ConnectionStatus {
    let connection = ConnectionStatus::new();
    connection.mark_ready_for_test();
    connection
}

#[tokio::test]
async fn cancelling_after_a_committed_claim_releases_only_the_slot() {
    let harness = Harness::new();
    let now = auth::system_now();
    let store = open_recovered_store(&harness, now).await;
    let policy = policy(&harness);
    let first = add_due_reminder(&store, &policy, now).await;
    let second = add_due_reminder(&store, &policy, now).await;
    let connection = test_connection();
    let members = Roster::new();
    let channels = [HOME_C.to_string()].into_iter().collect();
    let alerts = LogAlerts;
    let (hold, reached, _resume) = Hold::new();
    let mut delivery = gated_delivery(
        &store,
        &*harness.fake,
        &alerts,
        &members,
        &channels,
        connection.clone(),
        delivery_config(policy.clone(), None),
    )
    .with_claim_result_hook(hold_hook(hold));
    let mut tick = Box::pin(delivery.dispatch_reminders(now));
    tokio::select! {
        biased;
        result = &mut tick => panic!("claim returned through the barrier: {result:?}"),
        reached = tokio::time::timeout(std::time::Duration::from_secs(5), reached) => {
            reached.expect("claim-result barrier").expect("barrier signal");
        }
    }
    assert_eq!(
        harness.fake.count(Op::Create),
        0,
        "claim precedes transport"
    );
    drop(tick);
    drop(delivery);

    let target = |id: &str| DeliveryTarget::Reminder(id.to_owned());
    let held = store.load_view().await.unwrap().targets().clone();
    let held_target = [first.as_str(), second.as_str()]
        .into_iter()
        .map(target)
        .find(|target| held.contains(target))
        .expect("the committed claim remains held");
    assert!(connection.delivery_eligibility().is_some(), "slot released");

    let mut retry = gated_delivery(
        &store,
        &*harness.fake,
        &alerts,
        &members,
        &channels,
        connection.clone(),
        delivery_config(policy, None),
    );
    let report = retry.dispatch_reminders(now).await.unwrap();
    assert!(report.sends.iter().any(|send| {
        send.intent.targets.contains(&held_target) && send.outcome == SendOutcome::Suppressed
    }));
    assert!(report.sends.iter().any(|send| {
        !send.intent.targets.contains(&held_target) && matches!(send.outcome, SendOutcome::Bound(_))
    }));
    assert_eq!(
        harness.fake.count(Op::Create),
        1,
        "only the other target posts"
    );
    assert!(
        store
            .load_view()
            .await
            .unwrap()
            .targets()
            .contains(&held_target)
    );

    drop(retry);
    store::close(store, std::time::Duration::ZERO).await;
}

#[tokio::test]
async fn cancelling_a_blocked_create_keeps_its_target_held_and_admits_another() {
    let harness = Harness::new();
    let now = auth::system_now();
    let store = open_recovered_store(&harness, now).await;
    let policy = policy(&harness);
    let first = add_due_reminder(&store, &policy, now).await;
    let second = add_due_reminder(&store, &policy, now).await;
    let connection = test_connection();
    let members = Roster::new();
    let channels = [HOME_C.to_string()].into_iter().collect();
    let alerts = LogAlerts;
    let (hold, reached, _resume) = Hold::new();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let transport = BlockedTransport {
        fake: Arc::clone(&harness.fake),
        blocked: Op::Create,
        hold,
        in_flight: Arc::clone(&in_flight),
    };
    let mut delivery = gated_delivery(
        &store,
        &transport,
        &alerts,
        &members,
        &channels,
        connection.clone(),
        delivery_config(policy.clone(), None),
    );
    let mut tick = Box::pin(delivery.dispatch_reminders(now));
    tokio::select! {
        biased;
        result = &mut tick => panic!("create returned through the barrier: {result:?}"),
        reached = tokio::time::timeout(std::time::Duration::from_secs(5), reached) => {
            reached.expect("blocked create").expect("barrier signal");
        }
    }
    assert_eq!(
        harness.fake.count(Op::Create),
        1,
        "remote effect may have happened"
    );
    drop(tick);
    drop(delivery);
    assert_eq!(
        in_flight.load(Ordering::SeqCst),
        0,
        "transport future was dropped"
    );

    let target = |id: &str| DeliveryTarget::Reminder(id.to_owned());
    let held_targets = store.load_view().await.unwrap().targets().clone();
    let held_target = [first.as_str(), second.as_str()]
        .into_iter()
        .map(target)
        .find(|target| held_targets.contains(target))
        .expect("claimed reminder remains held");
    assert!(connection.delivery_eligibility().is_some(), "slot released");

    let mut retry = gated_delivery(
        &store,
        &transport,
        &alerts,
        &members,
        &channels,
        connection.clone(),
        delivery_config(policy, None),
    );
    let report = retry.dispatch_reminders(now).await.unwrap();
    assert!(report.sends.iter().any(|send| {
        send.intent.targets.contains(&held_target) && send.outcome == SendOutcome::Suppressed
    }));
    assert!(report.sends.iter().any(|send| {
        !send.intent.targets.contains(&held_target) && matches!(send.outcome, SendOutcome::Bound(_))
    }));
    assert_eq!(
        harness.fake.count(Op::Create),
        2,
        "only a different target retries"
    );
    assert!(
        store
            .load_view()
            .await
            .unwrap()
            .targets()
            .contains(&held_target)
    );

    drop(retry);
    store::close(store, std::time::Duration::ZERO).await;
}

async fn seed_digest(
    store: &SqliteStore,
    week: DateTime<Utc>,
    previous_week: DateTime<Utc>,
    message: &str,
    now: DateTime<Utc>,
) -> crate::domain::notify::AttemptId {
    let lease = store
        .begin_lease("cancel-safety-test", "seed_digest", now)
        .await
        .unwrap();
    let intent = NotificationIntent {
        effect: EffectKind::Digest,
        effect_context: Vec::new(),
        channel_id: HOME_C.to_string(),
        targets: vec![DeliveryTarget::Digest(week)],
        mentions: Vec::new(),
        content: IntentContent::Digest {
            week_start: week,
            inclusion: DigestInclusion::default(),
        },
        warnings: Vec::new(),
    };
    let Claim::Fresh(attempt) = store.claim(&lease, &intent, None, now).await.unwrap() else {
        panic!("digest seed must claim its target");
    };
    store
        .bind(
            &lease,
            &attempt,
            &Receipt {
                channel_id: HOME_C.to_string(),
                message_id: message.to_owned(),
            },
            None,
            now,
        )
        .await
        .unwrap();
    store
        .record_digest_week(&lease, previous_week, now)
        .await
        .unwrap();
    store.end_lease(&lease, now).await.unwrap();
    attempt
}

#[tokio::test]
async fn cancelling_a_blocked_digest_delete_keeps_the_old_claim_and_admits_another() {
    let harness = Harness::new();
    let now = auth::system_now();
    let store = open_recovered_store(&harness, now).await;
    let policy = policy(&harness);
    let week = week_of(&policy, now);
    let old_claim = seed_digest(
        &store,
        week,
        week - chrono::Duration::weeks(1),
        "900000000000000001",
        now,
    )
    .await;
    let reminder = add_due_reminder(&store, &policy, now).await;
    harness.fake.seed_message(
        crate::bot::ids::parse_id(HOME_C.to_string().as_str()).unwrap(),
        crate::bot::ids::parse_id("900000000000000001").unwrap(),
    );
    let connection = test_connection();
    let members = Roster::new();
    let channels = [HOME_C.to_string()].into_iter().collect();
    let alerts = LogAlerts;
    let (hold, reached, _resume) = Hold::new();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let transport = BlockedTransport {
        fake: Arc::clone(&harness.fake),
        blocked: Op::Delete,
        hold,
        in_flight: Arc::clone(&in_flight),
    };
    let mut delivery = gated_delivery(
        &store,
        &transport,
        &alerts,
        &members,
        &channels,
        connection.clone(),
        delivery_config(policy.clone(), Some(HOME_C.to_string())),
    );
    let mut delete = Box::pin(delivery.post_week_digest(now));
    tokio::select! {
        biased;
        result = &mut delete => panic!("delete returned through the barrier: {result:?}"),
        reached = tokio::time::timeout(std::time::Duration::from_secs(5), reached) => {
            reached.expect("blocked digest delete").expect("barrier signal");
        }
    }
    assert_eq!(harness.fake.count(Op::Delete), 1);
    assert_eq!(
        harness.fake.count(Op::Create),
        0,
        "replacement has not posted"
    );
    drop(delete);
    drop(delivery);
    assert_eq!(
        in_flight.load(Ordering::SeqCst),
        0,
        "transport future was dropped"
    );

    let old = store.load_attempt(&old_claim).await.unwrap().unwrap();
    assert_eq!(old.state, crate::domain::notify::AttemptState::Bound);
    assert!(old.targets.contains(&DeliveryTarget::Digest(week)));
    assert!(
        store
            .load_digests()
            .await
            .unwrap()
            .digests
            .iter()
            .any(|digest| digest.week_start == week && digest.retired_at.is_none()),
        "delete result was not settled into retirement"
    );
    assert!(connection.delivery_eligibility().is_some(), "slot released");

    let mut retry = gated_delivery(
        &store,
        &transport,
        &alerts,
        &members,
        &channels,
        connection,
        delivery_config(policy, Some(HOME_C.to_string())),
    );
    let report = retry.dispatch_reminders(now).await.unwrap();
    assert!(report.sends.iter().any(|send| {
        send.intent
            .targets
            .contains(&DeliveryTarget::Reminder(reminder.clone()))
            && matches!(send.outcome, SendOutcome::Bound(_))
    }));
    assert_eq!(harness.fake.count(Op::Create), 1, "only the reminder posts");
    assert_eq!(
        harness.fake.count(Op::Delete),
        1,
        "delete has no detached retry"
    );

    drop(retry);
    store::close(store, std::time::Duration::ZERO).await;
}

#[tokio::test]
async fn claims_pause_until_fresh_ready_reconciles_and_resume_restores_them() {
    let harness = Harness::new();
    let (mut discord, ctx) = harness.start().await;
    let fake = &harness.fake;
    drive(&mut discord, async {
        ctx.events.send(ready()).unwrap();
        ctx.events.send(guild_create(&[HOME_C])).unwrap();
        eventually!(
            "initial gateway readiness",
            ctx.connection.delivery_claims_allowed()
        );
        let initial_roster_pages = fake.count(Op::ListMembers);

        ctx.events.send(Event::GatewayClose(None)).unwrap();
        eventually!(
            "gateway disconnect",
            ctx.connection.get() == Connection::Disconnected
        );
        let policy = policy(&harness);
        add_due_reminder(&ctx.store, &policy, auth::system_now()).await;
        sleep(TICK * 4).await;
        assert_eq!(
            fake.count(Op::Create),
            0,
            "disconnected tick does not claim"
        );

        ctx.events.send(ready()).unwrap();
        eventually!("fresh READY", ctx.connection.get() == Connection::Ready);
        assert!(
            !ctx.connection.delivery_claims_allowed(),
            "the old sticky guild/roster booleans cannot open the new generation"
        );
        sleep(TICK * 4).await;
        assert_eq!(fake.count(Op::Create), 0, "pre-guild READY stays paused");

        fake.script(Op::ListMembers, Step::Reject(RejectionKind::MissingAccess));
        ctx.events.send(guild_create(&[HOME_C])).unwrap();
        eventually!(
            "fresh-generation roster fetch fails",
            fake.count(Op::ListMembers) == initial_roster_pages + 1
        );
        assert!(
            !ctx.connection.delivery_claims_allowed(),
            "a failed fresh-generation read must keep claims paused"
        );
        eventually!(
            "fresh-generation retry succeeds",
            ctx.connection.delivery_claims_allowed()
        );
        assert!(fake.count(Op::ListMembers) >= initial_roster_pages + 2);
        eventually!("pending reminder", fake.count(Op::Create) == 1);

        ctx.events.send(Event::GatewayClose(None)).unwrap();
        eventually!(
            "second gateway disconnect",
            ctx.connection.get() == Connection::Disconnected
        );
        add_due_reminder(&ctx.store, &policy, auth::system_now()).await;
        sleep(TICK * 4).await;
        assert_eq!(fake.count(Op::Create), 1, "second reminder stays pending");
        let roster_pages = fake.count(Op::ListMembers);

        ctx.events.send(Event::Resumed).unwrap();
        eventually!(
            "RESUMED readiness",
            ctx.connection.delivery_claims_allowed()
        );
        eventually!("resumed reminder", fake.count(Op::Create) == 2);
        assert_eq!(
            fake.count(Op::ListMembers),
            roster_pages,
            "RESUMED does not rescan"
        );
    })
    .await;
    finish(&harness, ctx).await;
}
