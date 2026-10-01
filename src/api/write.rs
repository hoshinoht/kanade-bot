//! The API's single scheduler writer: one `SchedulerService` behind an async
//! mutex, so admin mutations are serialised; reads never take it. The port is
//! object-safe so `ApiState` stays non-generic.

use std::{collections::BTreeSet, future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use super::{auth::Clock as ApiClockFn, state::ReadStore};
use crate::domain::{
    drafts::{ProposalStore, StoredDraft},
    history::{
        Actor, ChangeHistory, Expect, HeldReminders, Origin, RevertMode, RevertOutcome, Surface,
    },
    ids::RandomIds,
    members::Roster,
    notify::DeclineNoticeStore,
    proposals::Approver,
    requests::NoFreezes,
    schedule::{
        FixedEditChoices, FixedEditRequest, NewFixedRun, RsvpState, SchedulePolicy, StatusChange,
    },
    scheduler::{
        Approved, Clock, DeclineNoticeContext, DeclineRsvpResult, IdSource, ProposalApproved,
        ProposalError, ProposalPreview, Rejected, RequestError, RequestPreview, ScheduleStore,
        SchedulerResult, SchedulerService, StoreError,
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

/// One run edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunWrite {
    Move {
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
}

impl<S: ScheduleStore + Clone, I: IdSource, C: Clock> SchedulerWriter<S, I, C> {
    pub fn new(service: SchedulerService<S, I, C>) -> Self {
        Self {
            reader: service.store().clone(),
            attendance: service.attendance(),
            service: Mutex::new(service),
        }
    }

    fn previewer(&self, now: DateTime<Utc>) -> SchedulerService<S, RandomIds, Pinned> {
        SchedulerService::new(self.reader.clone(), RandomIds, Pinned(now))
            .with_attendance(self.attendance)
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
            let handle = service.as_origin(origin).expecting(expect);
            // Notices are already in the store's outbox, written with the change.
            match write {
                RunWrite::Move { to } => handle.amend_run(run_id, to, &ctx.policy).await.map(drop),
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

    fn add_fixed<'a>(
        &'a self,
        origin: Origin,
        new: NewFixedRun,
        _: &'a WriteContext,
    ) -> WriteFuture<'a, String> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service.as_origin(origin).add_fixed_run(new).await
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
