//! The scheduler tick (v4 `BossBot.tick`): materialise on a boss-week
//! rollover (taking the week's automatic history checkpoint), mark finished
//! runs done, recount attendance (v5 mode only), expire past-week drafts and
//! proposals past their TTL, drain the notice outbox, post the weekly
//! digest, then dispatch due reminders. One clock reading and one journal
//! lease per tick.
//!
//! v5 deviation (user decision): the digest posts only when the current boss
//! week is after the last recorded one. A clock that reads an earlier week
//! alerts and posts nothing; v4 compared for equality and re-posted.

use std::fmt;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::{future::Future, pin::Pin};

use chrono::{DateTime, TimeDelta, Utc};

#[cfg(feature = "test-support")]
type AdmissionHook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
#[cfg(test)]
type ClaimResultHook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

use super::alerts::{AdminAlert, AlertSink, AlertThrottle};
use super::card_records;
use super::cards::{CardContext, CardKit, ReminderCardStore};
use super::executor::{Executor, Replacement, SendFailure, SendOutcome, SendReport};
use super::notices::NoticeReport;
use super::ports::{FixedClock, IdsRef, StoreRef};
use super::render::{render, unrendered};
use crate::bot::{
    gateway::{DeliveryEligibility, DeliveryOperation},
    transport::DiscordTransport,
};
use crate::domain::drafts::ProposalStore;
use crate::domain::history::{
    Actor, CheckpointKind, Checkpoints, NewCheckpoint, Origin, Surface, auto_checkpoint_name,
};
use crate::domain::members::Directory;
use crate::domain::notify::{
    ChannelDirectory, DeliveryJournal, DeliverySettings, DigestAction, DigestPostInput,
    DispatchInput, JournalError, Lease, NoticeOutbox, Queued, RecordReason, Recovery,
    SendDisposition, WeekReset, plan_digest_post, plan_digest_tick, plan_dispatch,
};
use crate::domain::schedule::SchedulePolicy;
use crate::domain::schedule::ScheduleSnapshot;
use crate::domain::scheduler::{
    Clock, IdSource, ScheduleStore, SchedulerError, SchedulerService, Scope, StoreError,
};
use crate::domain::time::{DateOutOfRange, from_iso};

/// The lease operation kind every tick runs under.
pub const TICK_OPERATION: &str = "scheduler_tick";
/// Posts per tick; the rest wait for the next tick.
pub const DEFAULT_MAX_SENDS_PER_TICK: usize = 20;

/// Guild settings the tick reads; the caller may change them between ticks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryConfig {
    /// Names this process's leases.
    pub instance_id: String,
    pub policy: SchedulePolicy,
    pub post_channel_id: Option<String>,
    pub quiet_mode: bool,
    /// Claimed sends per tick for dispatch; the notice drain has its own
    /// cap of the same size.
    pub max_sends_per_tick: usize,
    /// Outbox notices older than this at drain time are retired `stale`
    /// unsent ([`crate::domain::notify::DEFAULT_MAX_NOTICE_AGE`]).
    pub max_notice_age: TimeDelta,
}

impl DeliveryConfig {
    fn reset(&self) -> WeekReset {
        WeekReset {
            zone: self.policy.zone(),
            weekday: self.policy.reset_weekday,
            time: self.policy.reset_time,
        }
    }

    pub(super) fn settings(&self) -> DeliverySettings<'_> {
        DeliverySettings {
            post_channel_id: self.post_channel_id.as_deref(),
            quiet_mode: self.quiet_mode,
            attendance: self.policy.attendance,
        }
    }
}

/// A tick step failed; later steps of that tick did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryError {
    Journal(JournalError),
    Scheduler(SchedulerError),
    Date(DateOutOfRange),
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => error.fmt(f),
            Self::Scheduler(error) => error.fmt(f),
            Self::Date(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DeliveryError {}

impl From<JournalError> for DeliveryError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}

impl From<SchedulerError> for DeliveryError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl From<StoreError> for DeliveryError {
    fn from(error: StoreError) -> Self {
        Self::Scheduler(error.into())
    }
}

impl From<DateOutOfRange> for DeliveryError {
    fn from(error: DateOutOfRange) -> Self {
        Self::Date(error)
    }
}

/// Per-send error isolation (v4 `send_card` parity): a failed send is
/// already alerted and reported as [`SendOutcome::Failed`]; only lease loss
/// or an unavailable backend aborts the tick.
pub(super) fn settle(
    result: Result<SendOutcome, SendFailure>,
) -> Result<SendOutcome, DeliveryError> {
    match result {
        Ok(outcome) => Ok(outcome),
        Err(failure) if failure.aborts() => Err(failure.error.into()),
        Err(failure) => Ok(SendOutcome::Failed(failure.error.to_string())),
    }
}

/// What the digest step did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DigestOutcome {
    UpToDate,
    /// Delivery is paused until the gateway's current generation is usable.
    Paused,
    Recorded(RecordReason),
    /// The clock is behind the last posted week (deviation).
    ClockRolledBack,
    /// There was nowhere to post; nothing was stamped.
    NoChannel,
    /// The week's old card was not confirmed deleted.
    ReplacementSuppressed(String),
    /// A post was attempted; see the send report.
    Attempted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestReport {
    pub current_week: DateTime<Utc>,
    /// Active cards of earlier weeks retired this tick.
    pub retired: usize,
    pub outcome: DigestOutcome,
    pub send: Option<SendReport>,
}

impl DigestReport {
    /// The posted message id, as v4's step result.
    pub fn message_id(&self) -> Option<String> {
        match &self.send {
            Some(SendReport {
                outcome: SendOutcome::Bound(id),
                ..
            }) => Some(id.get().to_string()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchReport {
    /// Due rows retired without a message.
    pub retired: usize,
    pub sends: Vec<SendReport>,
    /// Sends not attempted because the per-tick cap was reached. The cap
    /// counts claimed sends only; suppressed, held and vanished ones are free.
    pub deferred: usize,
    /// Rows left queued with nowhere to post.
    pub queued: Vec<Queued>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TickReport {
    pub now: DateTime<Utc>,
    /// Runs created by a rollover materialisation.
    pub materialised: Vec<String>,
    pub done: Vec<String>,
    /// v5 attendance: runs whose status the recount changed (e.g. at risk
    /// once the unknown window opened); always empty in v4-compat mode.
    pub recounted: Vec<String>,
    /// Outbox notices sent (or held) this tick.
    pub notices: NoticeReport,
    pub digest: DigestReport,
    pub dispatch: DispatchReport,
}

/// The tick and its collaborators. `members` and `channels` are the live
/// roster and the channels the bot can post in.
pub struct Delivery<'a, S, I, T, A> {
    pub store: &'a S,
    pub ids: I,
    pub transport: &'a T,
    pub alerts: &'a A,
    pub members: &'a (dyn Directory + Sync),
    pub channels: &'a (dyn ChannelDirectory + Sync),
    pub config: DeliveryConfig,
    /// Catalog, art and the day-of heading rewrite for cards.
    pub cards: CardKit,
    /// The boss week last materialised by this process; `None` materialises
    /// on the first tick (v4 kept it in config; rematerialising is idempotent).
    materialised_week: Option<DateTime<Utc>>,
    /// Rate-limits repeated alerts for this process (one hour per key).
    throttle: AlertThrottle,
    claim_gate: Arc<dyn Fn() -> Option<DeliveryEligibility> + Send + Sync>,
    #[cfg(test)]
    claim_result_hook: Option<ClaimResultHook>,
    #[cfg(feature = "test-support")]
    admission_hook: Option<AdmissionHook>,
}

impl<'a, S, I, T, A> Delivery<'a, S, I, T, A>
where
    S: ScheduleStore
        + DeliveryJournal
        + NoticeOutbox
        + Checkpoints
        + ProposalStore
        + ReminderCardStore
        + Sync,
    I: IdSource,
    T: DiscordTransport,
    A: AlertSink,
{
    pub fn new(
        store: &'a S,
        ids: I,
        transport: &'a T,
        alerts: &'a A,
        members: &'a (dyn Directory + Sync),
        channels: &'a (dyn ChannelDirectory + Sync),
        config: DeliveryConfig,
    ) -> Self {
        Self {
            store,
            ids,
            transport,
            alerts,
            members,
            channels,
            config,
            cards: CardKit::default(),
            materialised_week: None,
            throttle: AlertThrottle::new(),
            claim_gate: Arc::new(|| Some(DeliveryEligibility::unguarded())),
            #[cfg(test)]
            claim_result_hook: None,
            #[cfg(feature = "test-support")]
            admission_hook: None,
        }
    }

    /// Render cards with this catalog, art and heading rewrite.
    #[must_use]
    pub fn with_cards(mut self, cards: CardKit) -> Self {
        self.cards = cards;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_claim_result_hook(
        mut self,
        hook: impl Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    ) -> Self {
        self.claim_result_hook = Some(Arc::new(hook));
        self
    }

    /// Gate claims while preserving the tick's maintenance work.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_claim_gate(mut self, gate: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        let gate = Arc::new(gate);
        self.claim_gate = Arc::new(move || {
            if !gate() {
                return None;
            }
            let validator = Arc::clone(&gate);
            Some(DeliveryEligibility::checked(move || validator()))
        });
        self
    }

    /// Pause after ownership is acquired and before its effect begins.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_admission_hook(
        mut self,
        hook: impl Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    ) -> Self {
        self.admission_hook = Some(Arc::new(hook));
        self
    }

    /// Supply a synchronized admission token at each claim/delete boundary.
    #[must_use]
    pub(crate) fn with_admission_gate(
        mut self,
        gate: impl Fn() -> Option<DeliveryEligibility> + Send + Sync + 'static,
    ) -> Self {
        self.claim_gate = Arc::new(gate);
        self
    }

    pub(super) async fn admit(&self) -> Option<DeliveryOperation> {
        let operation = (self.claim_gate)()?.admit()?;
        #[cfg(test)]
        let mut operation = operation;
        #[cfg(test)]
        if let Some(hook) = &self.claim_result_hook {
            let hook = Arc::clone(hook);
            operation.set_claim_result_hook(move || hook());
        }
        #[cfg(feature = "test-support")]
        if let Some(hook) = &self.admission_hook {
            hook().await;
        }
        Some(operation)
    }

    fn card_context<'s>(&'s self, schedule: &'s ScheduleSnapshot) -> CardContext<'s> {
        CardContext {
            schedule,
            attendance: self.config.policy.attendance,
            zone: self.config.policy.zone(),
            quiet: self.config.quiet_mode,
            members: self.members,
            catalog: self.cards.catalog.as_deref(),
        }
    }

    /// Once per process, after taking ownership of the store: attempts a
    /// previous process left in flight become indeterminate (never resent).
    ///
    /// # Errors
    /// The journal failed.
    pub async fn start(&self, now: DateTime<Utc>) -> Result<Recovery, DeliveryError> {
        Ok(self.store.recover_on_start(now).await?)
    }

    fn service(
        &mut self,
        now: DateTime<Utc>,
    ) -> SchedulerService<StoreRef<'a, S>, IdsRef<'_, I>, FixedClock> {
        SchedulerService::new(StoreRef(self.store), IdsRef(&mut self.ids), FixedClock(now))
            .with_attendance(self.config.policy.attendance)
    }

    pub(super) fn executor<'b>(&'b self, lease: &'b Lease) -> Executor<'b, S, T, A> {
        Executor {
            journal: self.store,
            transport: self.transport,
            alerts: self.alerts,
            throttle: &self.throttle,
            lease,
        }
    }

    pub(super) async fn leased<R>(
        &mut self,
        now: DateTime<Utc>,
        work: impl AsyncFnOnce(&mut Self, &Lease) -> Result<R, DeliveryError>,
    ) -> Result<R, DeliveryError> {
        let lease = self
            .store
            .begin_lease(&self.config.instance_id, TICK_OPERATION, now)
            .await?;
        let result = work(self, &lease).await;
        let ended = self.store.end_lease(&lease, now).await;
        let value = result?;
        ended?;
        Ok(value)
    }

    /// One tick at the clock's current reading.
    ///
    /// # Errors
    /// The first failing step; the lease is ended regardless.
    pub async fn tick(&mut self, clock: &impl Clock) -> Result<TickReport, DeliveryError> {
        self.tick_at(clock.now()).await
    }

    /// One tick at `now`.
    ///
    /// # Errors
    /// The first failing step; the lease is ended regardless.
    pub async fn tick_at(&mut self, now: DateTime<Utc>) -> Result<TickReport, DeliveryError> {
        self.leased(now, async move |this: &mut Self, lease: &Lease| {
            let current = this.config.reset().current_week(now)?;
            let materialised = if this.materialised_week == Some(current) {
                Vec::new()
            } else {
                this.materialise_now(now).await?
            };
            let done = this
                .service(now)
                .as_origin(tick_origin())
                .mark_done()
                .await?;
            let recounted = this.recount_attendance(now).await;
            this.expire_drafts(now).await;
            this.expire_proposals(now).await;
            let notices = this.notices_in(lease, now).await?;
            let digest = this.digest_in(lease, now).await?;
            let dispatch = this.dispatch_in(lease, now).await?;
            Ok(TickReport {
                now,
                materialised,
                done,
                recounted,
                notices,
                digest,
                dispatch,
            })
        })
        .await
    }

    async fn materialise_now(&mut self, now: DateTime<Utc>) -> Result<Vec<String>, DeliveryError> {
        let policy = self.config.policy.clone();
        let created = self
            .service(now)
            .as_origin(tick_origin())
            .materialise_weeks(&policy)
            .await?;
        let week = self.config.reset().current_week(now)?;
        // The week's automatic checkpoint, at the head right after
        // materialising; one per week, so a restart or retry is a no-op. A
        // failure never aborts the tick: it is alerted, and leaving the week
        // unmarked makes the next tick materialise (idempotently) and retry.
        let date = week.with_timezone(&self.config.policy.zone()).date_naive();
        let checkpoint = self
            .store
            .create_checkpoint(NewCheckpoint {
                name: auto_checkpoint_name(date),
                kind: CheckpointKind::Auto,
                week,
                created_at: now,
                created_by: Actor::system("delivery"),
            })
            .await;
        match checkpoint {
            Ok(_) => self.materialised_week = Some(week),
            Err(error) => {
                let alert = AdminAlert::CheckpointFailed {
                    week_start: week,
                    detail: error.to_string(),
                };
                if self.throttle.admit(&alert, now) {
                    self.alerts.alert(alert);
                }
            }
        }
        Ok(created)
    }

    /// Materialise the current and next two boss weeks (v4 `materialise_weeks`).
    ///
    /// # Errors
    /// The scheduler failed.
    pub async fn materialise(&mut self, now: DateTime<Utc>) -> Result<Vec<String>, DeliveryError> {
        self.materialise_now(now).await
    }

    /// v5 attendance: re-derive live runs' statuses at `now` so unknown
    /// answers turn a run at risk as its window opens. A failure never aborts
    /// the tick: it is alerted (throttled) and the next tick retries.
    async fn recount_attendance(&mut self, now: DateTime<Utc>) -> Vec<String> {
        match self
            .service(now)
            .as_origin(tick_origin())
            .recount_attendance()
            .await
        {
            Ok(changed) => changed,
            Err(error) => {
                let alert = AdminAlert::AttendanceRecountFailed {
                    detail: error.to_string(),
                };
                if self.throttle.admit(&alert, now) {
                    self.alerts.alert(alert);
                }
                Vec::new()
            }
        }
    }

    /// Expire past-week drafts. A failure never aborts the tick: it is
    /// alerted (throttled) and the next tick retries, as checkpoints do.
    async fn expire_drafts(&mut self, now: DateTime<Utc>) {
        let policy = self.config.policy.clone();
        if let Err(error) = self.service(now).expire_due_drafts(&policy).await {
            let alert = AdminAlert::DraftExpiryFailed {
                detail: error.to_string(),
            };
            if self.throttle.admit(&alert, now) {
                self.alerts.alert(alert);
            }
        }
    }

    /// Expire proposals past their TTL; nothing is posted for them. A
    /// failure is alerted like draft expiry and the next tick retries.
    async fn expire_proposals(&mut self, now: DateTime<Utc>) {
        if let Err(error) = self.service(now).expire_due_proposals().await {
            let alert = AdminAlert::DraftExpiryFailed {
                detail: error.to_string(),
            };
            if self.throttle.admit(&alert, now) {
                self.alerts.alert(alert);
            }
        }
    }

    /// Retire runs whose slot has passed.
    ///
    /// # Errors
    /// The scheduler failed.
    pub async fn mark_done(&mut self, now: DateTime<Utc>) -> Result<Vec<String>, DeliveryError> {
        Ok(self
            .service(now)
            .as_origin(tick_origin())
            .mark_done()
            .await?)
    }

    /// The digest step alone, under its own lease.
    ///
    /// # Errors
    /// A journal, store or date failure.
    pub async fn post_week_digest(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<DigestReport, DeliveryError> {
        self.leased(now, async move |this: &mut Self, lease: &Lease| {
            this.digest_in(lease, now).await
        })
        .await
    }

    /// The dispatch step alone, under its own lease.
    ///
    /// # Errors
    /// A journal or store failure.
    pub async fn dispatch_reminders(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<DispatchReport, DeliveryError> {
        self.leased(now, async move |this: &mut Self, lease: &Lease| {
            this.dispatch_in(lease, now).await
        })
        .await
    }

    /// v4 `_post_week_digest` with the monotone-week deviation.
    async fn digest_in(
        &self,
        lease: &Lease,
        now: DateTime<Utc>,
    ) -> Result<DigestReport, DeliveryError> {
        let reset = self.config.reset();
        let current_week = reset.current_week(now)?;
        let mut report = DigestReport {
            current_week,
            retired: 0,
            outcome: DigestOutcome::UpToDate,
            send: None,
        };
        let Some(mut operation) = self.admit().await else {
            report.outcome = DigestOutcome::Paused;
            return Ok(report);
        };
        if !operation.begin() {
            report.outcome = DigestOutcome::Paused;
            return Ok(report);
        }
        let retired = self
            .store
            .retire_digests_before(lease, current_week, now)
            .await;
        operation.settle();
        report.retired = retired?;
        let log = self.store.load_digests().await?;
        let last = log
            .last_digest_week
            .as_deref()
            .and_then(|text| from_iso(text).ok());
        if let Some(last) = last {
            if last > current_week {
                self.executor(lease).raise(
                    AdminAlert::DigestClockRollback {
                        current_week,
                        last_digest_week: last,
                    },
                    now,
                );
                report.outcome = DigestOutcome::ClockRolledBack;
                return Ok(report);
            }
            if last == current_week {
                return Ok(report);
            }
        }
        let plan = plan_digest_tick(
            &reset,
            now,
            log.last_digest_week.as_deref(),
            self.config.post_channel_id.as_deref(),
        )?;
        match plan.action {
            DigestAction::UpToDate => return Ok(report),
            DigestAction::Record(reason) => {
                let Some(mut operation) = self.admit().await else {
                    report.outcome = DigestOutcome::Paused;
                    return Ok(report);
                };
                if !operation.begin() {
                    report.outcome = DigestOutcome::Paused;
                    return Ok(report);
                }
                let recorded = self
                    .store
                    .record_digest_week(lease, current_week, now)
                    .await;
                operation.settle();
                recorded?;
                report.outcome = DigestOutcome::Recorded(reason);
                return Ok(report);
            }
            DigestAction::Post => {}
        }
        let week = self.store.load(&Scope::Weeks(vec![current_week])).await?;
        let runs = &week.runs;
        let view = self.store.load_view().await?;
        let post = plan_digest_post(&DigestPostInput {
            week_start: current_week,
            current_week,
            zone: reset.zone,
            runs,
            digests: &log.digests,
            explicit_channel: None,
            settings: self.config.settings(),
            channels: self.channels,
            journal: &view,
        })?;
        let Some(post) = post else {
            report.outcome = DigestOutcome::NoChannel;
            return Ok(report);
        };
        let executor = self.executor(lease);
        // A suppressed send may already have posted: never delete for it.
        if post.send.disposition == SendDisposition::Send
            && let Some(old) = &post.replaces
        {
            let Some(permit) = self.admit().await else {
                report.outcome = DigestOutcome::Paused;
                return Ok(report);
            };
            match executor.replace_digest_admitted(permit, old, now).await? {
                Some(Replacement::Suppressed(reason)) => {
                    report.outcome = DigestOutcome::ReplacementSuppressed(reason);
                    return Ok(report);
                }
                Some(Replacement::Retired) => {}
                None => {
                    report.outcome = DigestOutcome::Paused;
                    return Ok(report);
                }
            }
        }
        let message = match post.send.disposition {
            SendDisposition::Send => {
                render(
                    &post.send.intent,
                    &self.card_context(&week),
                    None,
                    self.cards.art.as_ref(),
                )
                .await
            }
            SendDisposition::Suppressed => unrendered(),
        };
        let Some(permit) = self.admit().await else {
            report.outcome = DigestOutcome::Paused;
            return Ok(report);
        };
        let result = match executor
            .execute_admitted(permit, &post.send, &message, None, post.record_week, now)
            .await
        {
            Ok(Some(outcome)) => Ok(outcome),
            Ok(None) => {
                report.outcome = DigestOutcome::Paused;
                return Ok(report);
            }
            Err(failure) => Err(failure),
        };
        let outcome = settle(result)?;
        report.outcome = DigestOutcome::Attempted;
        report.send = Some(SendReport {
            intent: post.send.intent,
            outcome,
        });
        Ok(report)
    }

    /// v4 `dispatch_reminders`: retire hopeless rows, then post the rest.
    async fn dispatch_in(
        &mut self,
        lease: &Lease,
        now: DateTime<Utc>,
    ) -> Result<DispatchReport, DeliveryError> {
        let schedule = self.store.load(&Scope::All).await?;
        let view = self.store.load_view().await?;
        let plan = plan_dispatch(&DispatchInput {
            now,
            schedule: &schedule,
            members: self.members,
            channels: self.channels,
            journal: &view,
            settings: self.config.settings(),
        });
        let mut report = DispatchReport {
            queued: plan.queued,
            ..DispatchReport::default()
        };
        for retirement in &plan.retire {
            let Some(mut operation) = self.admit().await else {
                return Ok(report);
            };
            if !operation.begin() {
                return Ok(report);
            }
            let retired = self
                .service(now)
                .as_origin(tick_origin())
                .mark_reminder_sent(&retirement.reminder_id, None)
                .await;
            operation.settle();
            retired?;
            report.retired += 1;
        }
        let limit = self.config.max_sends_per_tick;
        let executor = self.executor(lease);
        let mut claimed = 0;
        for send in plan.sends {
            // Only claimed sends use the cap; suppressed ones cost nothing.
            if send.disposition == SendDisposition::Send && claimed >= limit {
                report.deferred += 1;
                continue;
            }
            // Suppressed sends post nothing: no record, rewrite or art read.
            let message = match send.disposition {
                SendDisposition::Send => {
                    let ctx = self.card_context(&schedule);
                    let record =
                        card_records::prepare(self.store, &self.cards, &ctx, &send.intent, now)
                            .await;
                    let heading = record.as_ref().and_then(|record| record.heading.as_deref());
                    render(&send.intent, &ctx, heading, self.cards.art.as_ref()).await
                }
                SendDisposition::Suppressed => unrendered(),
            };
            let Some(permit) = self.admit().await else {
                break;
            };
            let result = executor
                .execute_admitted(permit, &send, &message, None, None, now)
                .await;
            let result = match result {
                Ok(Some(outcome)) => Ok(outcome),
                Ok(None) => break,
                Err(failure) => Err(failure),
            };
            if matches!(&result, Err(failure) if failure.attempt.is_some())
                || result.as_ref().is_ok_and(SendOutcome::claimed)
            {
                claimed += 1;
            }
            let outcome = settle(result)?;
            report.sends.push(SendReport {
                intent: send.intent,
                outcome,
            });
        }
        Ok(report)
    }
}

/// Every change the tick makes is Kanade's own.
fn tick_origin() -> Origin {
    Origin::new(Actor::system("delivery"), Surface::DeliveryTick)
}
