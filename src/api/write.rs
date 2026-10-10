//! The API's single scheduler writer: one `SchedulerService` behind an async
//! mutex, so admin mutations are serialised; reads never take it. The port is
//! object-safe so `ApiState` stays non-generic.

use std::{collections::BTreeSet, fmt, future::Future, pin::Pin, sync::Arc};

#[cfg(any(test, feature = "test-support"))]
use std::sync::{Mutex as StdMutex, OnceLock};

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::Notify;

use super::{auth::Clock as ApiClockFn, state::ReadStore};
use crate::domain::{
    drafts::{DraftStatus, ProposalStore, StoredDraft},
    history::{
        Actor, ChangeHistory, Expect, HeldReminders, Origin, RevertMode, RevertOutcome, Surface,
    },
    ids::RandomIds,
    members::Roster,
    notify::DeclineNoticeStore,
    ownership::OwnerPin,
    proposals::Approver,
    requests::{NoFreezes, RequestSpec},
    schedule::{
        FixedEditChoices, FixedEditRequest, NewFixedRun, RsvpState, SchedulePolicy, SettleRun,
        StatusChange,
    },
    scheduler::{
        Approved, Clock, DeclineNoticeContext, DeclineRsvpResult, DraftError, IdSource,
        ProposalApproved, ProposalError, ProposalPreview, Rejected, RequestError, RequestPreview,
        ScheduleStore, SchedulerResult, SchedulerService, StoreError,
    },
};

pub type WriteFuture<'a, T> = Pin<Box<dyn Future<Output = SchedulerResult<T>> + Send + 'a>>;
/// A request or proposal decision (their own error types).
pub type DecideFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// No freeze store exists yet: nobody is frozen.
const GATE: NoFreezes = NoFreezes;

/// What every write reads besides its arguments.
pub struct WriteContext {
    pub policy: SchedulePolicy,
    /// Members and watched channels, for participant and channel checks.
    pub directory: Roster,
}

/// The admin fixed-PATCH's complete, normalised identity. It is deliberately
/// opaque: only that handler may replace the scheduler's derived edit digest.
#[derive(Clone)]
pub struct FixedPatchReplay(String);

impl FixedPatchReplay {
    pub(crate) fn from_normalized(identity: impl fmt::Debug) -> Self {
        Self(format!("{identity:?}"))
    }

    fn identity(&self) -> &str {
        &self.0
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
struct FixedPatchLookupGateInner {
    request_id: String,
    lost_channel: Option<String>,
    reached: Arc<Notify>,
    release: Arc<Notify>,
}

#[cfg(any(test, feature = "test-support"))]
fn fixed_patch_lookup_gate() -> &'static StdMutex<Option<FixedPatchLookupGateInner>> {
    static GATE: OnceLock<StdMutex<Option<FixedPatchLookupGateInner>>> = OnceLock::new();
    GATE.get_or_init(|| StdMutex::new(None))
}

/// Test-only gate immediately after the fixed-PATCH handler's initial key
/// lookup. It is consumed once, so a first writer can pass while its retry is held.
#[cfg(any(test, feature = "test-support"))]
pub struct FixedPatchLookupGate(FixedPatchLookupGateInner);

#[cfg(any(test, feature = "test-support"))]
impl FixedPatchLookupGate {
    pub fn install(request_id: impl Into<String>) -> Self {
        Self::install_inner(request_id.into(), None)
    }

    /// Simulate a watched-channel cache loss only in the held handler's
    /// strict validation path; replay normalization must recover it.
    pub fn install_lost_channel(
        request_id: impl Into<String>,
        channel_id: impl Into<String>,
    ) -> Self {
        Self::install_inner(request_id.into(), Some(channel_id.into()))
    }

    fn install_inner(request_id: String, lost_channel: Option<String>) -> Self {
        let gate = FixedPatchLookupGateInner {
            request_id,
            lost_channel,
            reached: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        let mut slot = fixed_patch_lookup_gate()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(slot.is_none(), "one fixed-PATCH lookup gate at a time");
        *slot = Some(gate.clone());
        Self(gate)
    }

    pub async fn reached(&self) {
        self.0.reached.notified().await;
    }

    pub fn release(&self) {
        self.0.release.notify_one();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for FixedPatchLookupGate {
    fn drop(&mut self) {
        let mut slot = fixed_patch_lookup_gate()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|active| active.request_id == self.0.request_id)
        {
            *slot = None;
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub async fn hold_fixed_patch_after_lookup(origin: &Origin) -> Option<String> {
    let gate = {
        let mut slot = fixed_patch_lookup_gate()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|gate| origin.request_id.as_deref() == Some(&gate.request_id))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.release.notified().await;
        gate.lost_channel
    } else {
        None
    }
}

/// One run edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunWrite {
    Move {
        to: DateTime<Utc>,
    },
    /// The origin's member moves their own run (public portal): the same
    /// move, refused inside the commit unless it is theirs to move now.
    MemberMove {
        to: DateTime<Utc>,
    },
    Swap {
        with_id: String,
        version: u64,
    },
    Status(StatusChange),
    /// A portal answer (source `chat`, status re-derived, a status pin kept);
    /// `None` clears whatever the member answered.
    Rsvp {
        user_id: String,
        answer: Option<RsvpState>,
    },
    Participants {
        add: Vec<String>,
        remove: Vec<String>,
    },
    Reset,
}

pub trait Writer: Send + Sync {
    fn run<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        write: RunWrite,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()>;

    /// An RSVP with its durable decline candidate. The caller owns the
    /// best-effort post-commit S2 retraction.
    fn rsvp<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        user_id: &'a str,
        answer: Option<RsvpState>,
        decline: DeclineNoticeContext,
    ) -> WriteFuture<'a, DeclineRsvpResult<bool>>;

    /// The origin's member answers their own run (public portal), refused
    /// inside the commit unless they are on it and it is live in this or next
    /// boss week.
    fn member_answer<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        answer: RsvpState,
        decline: DeclineNoticeContext,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, DeclineRsvpResult<bool>>;

    /// A new weekly timing and its runs in the materialised weeks, in one
    /// commit (one change record and outbox).
    fn add_fixed<'a>(
        &'a self,
        origin: Origin,
        new: NewFixedRun,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, String>;

    fn edit_fixed<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        request: FixedEditRequest,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()>;

    /// Pin a weekly timing's new owner (`api::ownership`), refused inside
    /// the commit unless `pin` holds on the committed timing.
    fn pin_owner<'a>(
        &'a self,
        origin: Origin,
        fixed_id: &'a str,
        pin: OwnerPin<'a>,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()>;

    /// Fixed-PATCH only: preserve the full request identity before the handler
    /// reduces it to a diff against the current row.
    fn edit_fixed_patch<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        request: FixedEditRequest,
        replay: FixedPatchReplay,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()>;

    /// Re-check a keyed fixed PATCH that became a no-op or whose recorded
    /// request must answer before the current-row diff is examined.
    fn verify_fixed_patch_replay<'a>(
        &'a self,
        origin: Origin,
        replay: FixedPatchReplay,
    ) -> WriteFuture<'a, ()>;

    /// Cancels the timing's live runs in the materialised weeks.
    fn retire_fixed<'a>(
        &'a self,
        origin: Origin,
        fixed_id: &'a str,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, usize>;

    /// Materialise the current and next two boss weeks (a new timing's runs).
    fn materialise<'a>(
        &'a self,
        origin: Origin,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, Vec<String>>;

    /// A completion prompt's Done / Didn't happen (`domain::completion`):
    /// the run becomes the asked status unannounced only if, on the
    /// committed state, it is still live, still at the start the presser
    /// saw and they are on it or staff; `false` when not settled.
    fn settle_run<'a>(
        &'a self,
        origin: Origin,
        settle: SettleRun,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, bool>;

    /// A rollback by the origin's admin (`Surface::Rollback`), or its preview,
    /// which writes nothing and ignores the request id. `held` is re-read on
    /// every commit attempt.
    fn rollback<'a>(
        &'a self,
        origin: Origin,
        request: RollbackRequest,
        held: &'a dyn ReadStore,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, RevertOutcome>;

    /// Previews write nothing and never wait for the writer.
    fn preview_request<'a>(
        &'a self,
        actor: &'a Actor,
        id: &'a str,
        choices: Option<FixedEditChoices>,
        ctx: &'a WriteContext,
        now: DateTime<Utc>,
    ) -> DecideFuture<'a, RequestPreview, RequestError>;

    fn preview_proposal<'a>(
        &'a self,
        id: &'a str,
        approver: Option<&'a str>,
        ctx: &'a WriteContext,
        now: DateTime<Utc>,
    ) -> DecideFuture<'a, ProposalPreview, ProposalError>;

    fn approve_request<'a>(
        &'a self,
        actor: &'a Actor,
        id: &'a str,
        version: u64,
        choices: Option<FixedEditChoices>,
        ctx: &'a WriteContext,
    ) -> DecideFuture<'a, Approved, RequestError>;

    fn reject_request<'a>(
        &'a self,
        actor: &'a Actor,
        id: &'a str,
        version: u64,
        reason: &'a str,
    ) -> DecideFuture<'a, Rejected, RequestError>;

    /// A member's request, keyed by `request_id` (an exact retry answers the
    /// stored request); `true` when this call created it.
    fn submit_request<'a>(
        &'a self,
        member: &'a str,
        title: &'a str,
        spec: RequestSpec,
        request_id: String,
        ctx: &'a WriteContext,
    ) -> DecideFuture<'a, (StoredDraft, bool), RequestError>;

    /// The requester withdraws their request at `version`.
    fn withdraw_request<'a>(
        &'a self,
        member: &'a str,
        id: &'a str,
        version: u64,
    ) -> DecideFuture<'a, StoredDraft, RequestError>;

    /// `edit`: the portal's "edit, then approve" time.
    fn approve_proposal<'a>(
        &'a self,
        id: &'a str,
        approver: &'a Approver,
        edit: Option<DateTime<Utc>>,
        ctx: &'a WriteContext,
    ) -> DecideFuture<'a, ProposalApproved, ProposalError>;

    fn reject_proposal<'a>(
        &'a self,
        id: &'a str,
        approver: &'a Approver,
    ) -> DecideFuture<'a, StoredDraft, ProposalError>;
}

/// Which recorded changes a rollback undoes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackSelection {
    Seqs(Vec<u64>),
    /// Every later change touching the week, only within it.
    Week {
        week: DateTime<Utc>,
        revision: u64,
    },
    Actor {
        actor: Actor,
        since: DateTime<Utc>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollbackRequest {
    pub selection: RollbackSelection,
    pub mode: RevertMode,
    pub preview: bool,
}

/// The held reminders, read through the API's store on each call.
struct StoreHeld<'a>(&'a dyn ReadStore);

impl HeldReminders for StoreHeld<'_> {
    fn held_reminders(&self) -> impl Future<Output = Result<BTreeSet<String>, StoreError>> + Send {
        self.0.held_reminders()
    }
}

/// The scheduler clock over the API's pinned-or-system clock.
pub struct ApiClock(pub ApiClockFn);

impl Clock for ApiClock {
    fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}

/// A fixed instant for one preview.
struct Pinned(DateTime<Utc>);

impl Clock for Pinned {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

pub struct SchedulerWriter<S, I, C> {
    service: Mutex<SchedulerService<S, I, C>>,
    /// The same store, for previews outside the writer lock.
    reader: S,
    attendance: crate::domain::attendance::AttendancePolicy,
    run_ends: Option<crate::domain::completion::RunEndsSource>,
}

impl<S: ScheduleStore + Clone, I: IdSource, C: Clock> SchedulerWriter<S, I, C> {
    pub fn new(service: SchedulerService<S, I, C>) -> Self {
        Self {
            reader: service.store().clone(),
            attendance: service.attendance(),
            run_ends: service.run_ends_source().cloned(),
            service: Mutex::new(service),
        }
    }

    fn previewer(&self, now: DateTime<Utc>) -> SchedulerService<S, RandomIds, Pinned> {
        let service = SchedulerService::new(self.reader.clone(), RandomIds, Pinned(now))
            .with_attendance(self.attendance);
        match &self.run_ends {
            Some(source) => service.with_run_ends(source.clone()),
            None => service,
        }
    }
}

impl<S, I, C> Writer for SchedulerWriter<S, I, C>
where
    S: ScheduleStore + DeclineNoticeStore + ChangeHistory + ProposalStore + Clone + Send + Sync,
    // Rollbacks borrow the whole service across awaits.
    I: IdSource + Send + Sync,
    C: Clock + Send + Sync,
{
    fn run<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        write: RunWrite,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            // v4 parity: a Discord swap carries no "(via portal)" mark.
            let via_portal = origin.surface != Surface::Discord;
            let member = origin.actor.id().to_owned();
            let handle = service.as_origin(origin).expecting(expect);
            // Notices are already in the store's outbox, written with the change.
            match write {
                RunWrite::Move { to } => handle.amend_run(run_id, to, &ctx.policy).await.map(drop),
                RunWrite::MemberMove { to } => handle
                    .member_amend_run(&member, run_id, to, &ctx.policy)
                    .await
                    .map(drop),
                RunWrite::Swap { with_id, version } => handle
                    .swap_run_slots_at_version(run_id, &with_id, Some(version), &ctx.policy)
                    .await
                    .map(drop),
                RunWrite::Status(change) => handle
                    .set_status(run_id, change, &ctx.policy.reminders)
                    .await
                    .map(drop),
                RunWrite::Rsvp { user_id, answer } => handle
                    .portal_answer(run_id, &user_id, answer)
                    .await
                    .map(drop),
                RunWrite::Participants { add, remove } => handle
                    .swap_participants(run_id, &remove, &add, via_portal, &ctx.directory)
                    .await
                    .map(drop),
                RunWrite::Reset => handle.reset_to_fixed(run_id, &ctx.policy).await.map(drop),
            }
        })
    }

    fn rsvp<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        user_id: &'a str,
        answer: Option<RsvpState>,
        decline: DeclineNoticeContext,
    ) -> WriteFuture<'a, DeclineRsvpResult<bool>> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .expecting(expect)
                .portal_answer_with_decline(run_id, user_id, answer, decline)
                .await
        })
    }

    fn member_answer<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        answer: RsvpState,
        decline: DeclineNoticeContext,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, DeclineRsvpResult<bool>> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            let member = origin.actor.id().to_owned();
            service
                .as_origin(origin)
                .expecting(expect)
                .member_answer_with_decline(run_id, &member, answer, decline, &ctx.policy)
                .await
        })
    }

    fn add_fixed<'a>(
        &'a self,
        origin: Origin,
        new: NewFixedRun,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, String> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .add_fixed_run_materialised(new, &ctx.policy)
                .await
        })
    }

    fn edit_fixed<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        request: FixedEditRequest,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .expecting(expect)
                .apply_fixed_edit(&request, &ctx.directory, &ctx.policy)
                .await
                .map(|_| ())
        })
    }

    fn pin_owner<'a>(
        &'a self,
        origin: Origin,
        fixed_id: &'a str,
        pin: OwnerPin<'a>,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .pin_fixed_owner(fixed_id, pin, &ctx.directory, &ctx.policy)
                .await
                .map(drop)
        })
    }

    fn edit_fixed_patch<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        request: FixedEditRequest,
        replay: FixedPatchReplay,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .expecting(expect)
                .apply_fixed_patch_edit(&request, replay.identity(), &ctx.directory, &ctx.policy)
                .await
                .map(|_| ())
        })
    }

    fn verify_fixed_patch_replay<'a>(
        &'a self,
        origin: Origin,
        replay: FixedPatchReplay,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .verify_fixed_patch_replay(replay.identity())
                .await
        })
    }

    fn retire_fixed<'a>(
        &'a self,
        origin: Origin,
        fixed_id: &'a str,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, usize> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            let now = service.clock().now();
            let weeks = ctx
                .policy
                .materialised_weeks(now)
                .map_err(crate::domain::schedule::ScheduleError::from)?;
            service
                .as_origin(origin)
                .retire_fixed_run(fixed_id, &weeks, &ctx.policy.reminders)
                .await
        })
    }

    fn materialise<'a>(
        &'a self,
        origin: Origin,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, Vec<String>> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .materialise_weeks(&ctx.policy)
                .await
        })
    }

    fn settle_run<'a>(
        &'a self,
        origin: Origin,
        settle: SettleRun,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, bool> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .settle_run(settle, &ctx.policy.reminders)
                .await
        })
    }

    fn rollback<'a>(
        &'a self,
        origin: Origin,
        request: RollbackRequest,
        held: &'a dyn ReadStore,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, RevertOutcome> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            let (admin, request_id) = (origin.actor.id().to_owned(), origin.request_id);
            let (held, reminders, mode) = (StoreHeld(held), &ctx.policy.reminders, request.mode);
            match (request.selection, request.preview) {
                (RollbackSelection::Seqs(seqs), true) => {
                    service
                        .preview_revert_changes(&seqs, mode, reminders, &held)
                        .await
                }
                (RollbackSelection::Seqs(seqs), false) => {
                    service
                        .revert_changes(&admin, request_id, &seqs, mode, reminders, &held)
                        .await
                }
                (RollbackSelection::Week { week, revision }, true) => {
                    service
                        .preview_restore_week(week, revision, mode, reminders, &held)
                        .await
                }
                (RollbackSelection::Week { week, revision }, false) => {
                    service
                        .restore_week_to(&admin, request_id, week, revision, mode, reminders, &held)
                        .await
                }
                (RollbackSelection::Actor { actor, since }, true) => {
                    service
                        .preview_revert_by_actor(&actor, since, mode, reminders, &held)
                        .await
                }
                (RollbackSelection::Actor { actor, since }, false) => {
                    service
                        .revert_by_actor(&admin, request_id, &actor, since, mode, reminders, &held)
                        .await
                }
            }
        })
    }

    fn preview_request<'a>(
        &'a self,
        actor: &'a Actor,
        id: &'a str,
        choices: Option<FixedEditChoices>,
        ctx: &'a WriteContext,
        now: DateTime<Utc>,
    ) -> DecideFuture<'a, RequestPreview, RequestError> {
        Box::pin(async move {
            self.previewer(now)
                .preview_request(actor, id, choices, &ctx.policy, &ctx.directory, &GATE)
                .await
        })
    }

    fn preview_proposal<'a>(
        &'a self,
        id: &'a str,
        approver: Option<&'a str>,
        ctx: &'a WriteContext,
        now: DateTime<Utc>,
    ) -> DecideFuture<'a, ProposalPreview, ProposalError> {
        Box::pin(async move {
            self.previewer(now)
                .preview_proposal(id, approver, &ctx.policy, &ctx.directory)
                .await
        })
    }

    fn approve_request<'a>(
        &'a self,
        actor: &'a Actor,
        id: &'a str,
        version: u64,
        choices: Option<FixedEditChoices>,
        ctx: &'a WriteContext,
    ) -> DecideFuture<'a, Approved, RequestError> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .approve_request(
                    actor,
                    id,
                    version,
                    choices,
                    &ctx.policy,
                    &ctx.directory,
                    &GATE,
                )
                .await
        })
    }

    fn reject_request<'a>(
        &'a self,
        actor: &'a Actor,
        id: &'a str,
        version: u64,
        reason: &'a str,
    ) -> DecideFuture<'a, Rejected, RequestError> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service.reject_request(actor, id, version, reason).await
        })
    }

    fn submit_request<'a>(
        &'a self,
        member: &'a str,
        title: &'a str,
        spec: RequestSpec,
        request_id: String,
        ctx: &'a WriteContext,
    ) -> DecideFuture<'a, (StoredDraft, bool), RequestError> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            // Under the writer lock, so a concurrent retry cannot slip between.
            let replay = service
                .store()
                .recorded_draft_request(&Actor::member(member), &request_id)
                .await?
                .is_some();
            let request = service
                .submit_request(
                    member,
                    title,
                    spec,
                    Some(request_id),
                    &ctx.policy,
                    &ctx.directory,
                    &GATE,
                )
                .await?;
            Ok((request, !replay))
        })
    }

    fn withdraw_request<'a>(
        &'a self,
        member: &'a str,
        id: &'a str,
        version: u64,
    ) -> DecideFuture<'a, StoredDraft, RequestError> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            // An admin edit bumps the version while it still waits: withdraw
            // the request as it now is.
            let mut version = version;
            loop {
                match service.withdraw_request(member, id, version).await {
                    Err(RequestError::Draft(DraftError::Stale {
                        status: DraftStatus::Submitted,
                        version: now,
                        ..
                    })) if now != version => version = now,
                    other => return other,
                }
            }
        })
    }

    fn approve_proposal<'a>(
        &'a self,
        id: &'a str,
        approver: &'a Approver,
        edit: Option<DateTime<Utc>>,
        ctx: &'a WriteContext,
    ) -> DecideFuture<'a, ProposalApproved, ProposalError> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .approve_proposal_at(id, approver, edit, &ctx.policy, &ctx.directory)
                .await
        })
    }

    fn reject_proposal<'a>(
        &'a self,
        id: &'a str,
        approver: &'a Approver,
    ) -> DecideFuture<'a, StoredDraft, ProposalError> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service.reject_proposal(id, approver).await
        })
    }
}

/// A shared writer.
pub type SharedWriter = Arc<dyn Writer>;
