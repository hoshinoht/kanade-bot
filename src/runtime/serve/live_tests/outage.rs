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
        delivery::{DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, SendOutcome, SendReport},
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

/// What a gated delivery borrows besides its store and transport: a ready
/// connection, an empty roster and the one home channel.
struct Gate {
    policy: SchedulePolicy,
    connection: ConnectionStatus,
    members: Roster,
    channels: BTreeSet<String>,
    alerts: LogAlerts,
}

impl Gate {
    fn new(policy: SchedulePolicy) -> Self {
        let connection = ConnectionStatus::new();
        connection.mark_ready_for_test();
        Self {
            policy,
            connection,
            members: Roster::new(),
            channels: [HOME_C.to_string()].into_iter().collect(),
            alerts: LogAlerts,
        }
    }

    fn delivery<'a, T: DiscordTransport>(
        &'a self,
        store: &'a SqliteStore,
        transport: &'a T,
        post_channel_id: Option<String>,
    ) -> Delivery<'a, SqliteStore, RandomIds, T, LogAlerts> {
        let config = DeliveryConfig {
            instance_id: "cancel-safety-test".into(),
            policy: self.policy.clone(),
            post_channel_id,
            quiet_mode: false,
            max_sends_per_tick: DEFAULT_MAX_SENDS_PER_TICK,
            max_notice_age: crate::domain::notify::DEFAULT_MAX_NOTICE_AGE,
            run_lengths: crate::domain::settings::RunLengths::default(),
            freeze_ended: true,
        };
        let connection = self.connection.clone();
        Delivery::new(
            store,
            RandomIds,
            transport,
            &self.alerts,
            &self.members,
            &self.channels,
            config,
        )
        .with_admission_gate(move || connection.delivery_eligibility())
    }
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

/// Drives `effect` until its hold reports the barrier; it must not finish first.
async fn until_held<F>(effect: &mut Pin<Box<F>>, reached: oneshot::Receiver<()>, what: &str)
where
    F: Future,
    F::Output: std::fmt::Debug,
{
    tokio::select! {
        biased;
        result = effect => panic!("{what} returned through the barrier: {result:?}"),
        reached = tokio::time::timeout(std::time::Duration::from_secs(5), reached) => {
            reached.expect(what).expect("barrier signal");
        }
    }
}

struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The fake transport, pausing after the `blocked` op's remote effect until
/// the test resumes it (never, here: the tests drop the paused future).
struct BlockedTransport {
    fake: Arc<FakeDiscord>,
    blocked: Op,
    hold: Arc<Hold>,
    _resume: oneshot::Sender<()>,
    in_flight: Arc<AtomicUsize>,
}

impl BlockedTransport {
    fn new(fake: &Arc<FakeDiscord>, blocked: Op) -> (Self, oneshot::Receiver<()>) {
        let (hold, reached, resume) = Hold::new();
        let transport = Self {
            fake: Arc::clone(fake),
            blocked,
            hold,
            _resume: resume,
            in_flight: Arc::new(AtomicUsize::new(0)),
        };
        (transport, reached)
    }

    fn assert_dropped(&self) {
        assert_eq!(
            self.in_flight.load(Ordering::SeqCst),
            0,
            "transport future was dropped"
        );
    }

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

/// Which of the two claimed reminders the store still holds.
async fn held_reminder(store: &SqliteStore, reminders: [&str; 2]) -> DeliveryTarget {
    let held = store.load_view().await.unwrap().targets().clone();
    reminders
        .into_iter()
        .map(|id| DeliveryTarget::Reminder(id.to_owned()))
        .find(|target| held.contains(target))
        .expect("the claimed reminder remains held")
}

/// A retry after a cancelled tick suppresses the held reminder, binds the
/// other one, and leaves the held claim in place.
async fn assert_retry_skips_held(store: &SqliteStore, retry: &[SendReport], held: &DeliveryTarget) {
    assert!(retry.iter().any(|send| {
        send.intent.targets.contains(held) && send.outcome == SendOutcome::Suppressed
    }));
    assert!(retry.iter().any(|send| {
        !send.intent.targets.contains(held) && matches!(send.outcome, SendOutcome::Bound(_))
    }));
    assert!(store.load_view().await.unwrap().targets().contains(held));
}

#[tokio::test]
async fn cancelling_after_a_committed_claim_releases_only_the_slot() {
    let harness = Harness::new();
    let now = auth::system_now();
    let store = open_recovered_store(&harness, now).await;
    let gate = Gate::new(policy(&harness));
    let first = add_due_reminder(&store, &gate.policy, now).await;
    let second = add_due_reminder(&store, &gate.policy, now).await;
    let (hold, reached, _resume) = Hold::new();
    let mut delivery = gate
        .delivery(&store, &*harness.fake, None)
        .with_claim_result_hook(hold_hook(hold));
    let mut tick = Box::pin(delivery.dispatch_reminders(now));
    until_held(&mut tick, reached, "claim").await;
    assert_eq!(
        harness.fake.count(Op::Create),
        0,
        "claim precedes transport"
    );
    drop(tick);
    drop(delivery);

    let held = held_reminder(&store, [first.as_str(), second.as_str()]).await;
    assert!(
        gate.connection.delivery_eligibility().is_some(),
        "slot released"
    );

    let mut retry = gate.delivery(&store, &*harness.fake, None);
    let report = retry.dispatch_reminders(now).await.unwrap();
    assert_retry_skips_held(&store, &report.sends, &held).await;
    assert_eq!(
        harness.fake.count(Op::Create),
        1,
        "only the other target posts"
    );

    drop(retry);
    store::close(store, std::time::Duration::ZERO).await;
}

#[tokio::test]
async fn cancelling_a_blocked_create_keeps_its_target_held_and_admits_another() {
    let harness = Harness::new();
    let now = auth::system_now();
    let store = open_recovered_store(&harness, now).await;
    let gate = Gate::new(policy(&harness));
    let first = add_due_reminder(&store, &gate.policy, now).await;
    let second = add_due_reminder(&store, &gate.policy, now).await;
    let (transport, reached) = BlockedTransport::new(&harness.fake, Op::Create);
    let mut delivery = gate.delivery(&store, &transport, None);
    let mut tick = Box::pin(delivery.dispatch_reminders(now));
    until_held(&mut tick, reached, "create").await;
    assert_eq!(
        harness.fake.count(Op::Create),
        1,
        "remote effect may have happened"
    );
    drop(tick);
    drop(delivery);
    transport.assert_dropped();

    let held = held_reminder(&store, [first.as_str(), second.as_str()]).await;
    assert!(
        gate.connection.delivery_eligibility().is_some(),
        "slot released"
    );

    let mut retry = gate.delivery(&store, &transport, None);
    let report = retry.dispatch_reminders(now).await.unwrap();
    assert_retry_skips_held(&store, &report.sends, &held).await;
    assert_eq!(
        harness.fake.count(Op::Create),
        2,
        "only a different target retries"
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
    let gate = Gate::new(policy(&harness));
    let week = week_of(&gate.policy, now);
    let old_claim = seed_digest(
        &store,
        week,
        week - chrono::Duration::weeks(1),
        "900000000000000001",
        now,
    )
    .await;
    let reminder = add_due_reminder(&store, &gate.policy, now).await;
    harness.fake.seed_message(
        crate::bot::ids::parse_id(HOME_C.to_string().as_str()).unwrap(),
        crate::bot::ids::parse_id("900000000000000001").unwrap(),
    );
    let (transport, reached) = BlockedTransport::new(&harness.fake, Op::Delete);
    let mut delivery = gate.delivery(&store, &transport, Some(HOME_C.to_string()));
    let mut delete = Box::pin(delivery.post_week_digest(now));
    until_held(&mut delete, reached, "digest delete").await;
    assert_eq!(harness.fake.count(Op::Delete), 1);
    assert_eq!(
        harness.fake.count(Op::Create),
        0,
        "replacement has not posted"
    );
    drop(delete);
    drop(delivery);
    transport.assert_dropped();

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
    assert!(
        gate.connection.delivery_eligibility().is_some(),
        "slot released"
    );

    let mut retry = gate.delivery(&store, &transport, Some(HOME_C.to_string()));
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
