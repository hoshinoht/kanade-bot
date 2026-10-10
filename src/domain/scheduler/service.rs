//! The single scheduler write path: load a scoped snapshot, plan, commit.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::ports::{
    AttendanceHistory, Clock, Committed, IdSource, ScheduleStore, Scope, StoreError,
};
use crate::domain::attendance::{
    self, AttendanceActor, AttendanceMode, AttendancePolicy, AttendanceRefusal,
};
use crate::domain::completion::{RunEnds, RunEndsSource};
use crate::domain::history::{
    Actor, ChangeFilter, ChangeHistory, ChangeMeta, ChangeQuery, ChangeRecord, ChangeRef,
    CheckedChange, Checkpoint, CheckpointKind, Checkpoints, EDIT_OVERRIDE, Expect, HeldReminders,
    HistoryRefusal, NewCheckpoint, Origin, PreconditionError, RevertMode, RevertOutcome,
    RevertScope, StaleField, Surface, apply_revert, changed_rows, changes_by_actor,
    changes_for_week, sha256_hex,
};
use crate::domain::members::Directory;
use crate::domain::notify::{DeclineNotice, DeclineNoticeStore};
use crate::domain::schedule::{
    self, AmendedRun, Draft, FixedEdit, FixedEditRequest, FixedField, FixedRun, FixedRunPatch,
    MemberRunRefusal, NewFixedRun, NewRun, Notice, NoticeChange, Op, OpResult, Outcome,
    ReactionResult, Reminder, ReminderPolicy, RsvpSource, RsvpState, RunState, RunStatus,
    ScheduleError, SchedulePolicy, StatusChange, WeekStart, apply_op, utc_instant,
};
use crate::domain::time::AwareDateTime;

mod fixed_patch;

/// A rule refusal or a store failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerError {
    Schedule(ScheduleError),
    Store(StoreError),
    /// The actor's request id was already recorded: the change applied
    /// before, as record `seq` at store `revision`, and is not applied again.
    AlreadyApplied {
        seq: u64,
        revision: u64,
    },
    /// A revert named records that are unknown, tampered or empty.
    History(HistoryRefusal),
    /// The actor's request id was already used, by change `seq`, for a
    /// different request; nothing was applied.
    IdempotencyMismatch {
        seq: u64,
    },
    /// Declared fields changed since the caller saw them; nothing was
    /// applied. Keep theirs, or (administrators) resubmit overriding them.
    StaleEdit {
        conflicts: Vec<StaleField>,
    },
    /// The edit's preconditions are malformed or not allowed.
    Precondition(PreconditionError),
    /// The actor may not make this change (a rule the service itself
    /// checks; other role checks belong to the API).
    Forbidden(String),
}

impl From<ScheduleError> for SchedulerError {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

impl From<StoreError> for SchedulerError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schedule(error) => error.fmt(f),
            Self::Store(error) => error.fmt(f),
            Self::AlreadyApplied { seq, revision } => write!(
                f,
                "request already applied as change {seq} (revision {revision})"
            ),
            Self::History(refusal) => refusal.fmt(f),
            Self::IdempotencyMismatch { seq } => write!(
                f,
                "request id already used by change {seq} for a different request"
            ),
            Self::StaleEdit { conflicts } => write!(
                f,
                "{} field(s) changed since they were read",
                conflicts.len()
            ),
            Self::Precondition(error) => error.fmt(f),
            Self::Forbidden(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for SchedulerError {}

pub type SchedulerResult<T> = Result<T, SchedulerError>;

/// Plans tried per operation before a revision conflict is returned.
pub const COMMIT_ATTEMPTS: usize = 3;

/// Owns the id source and clock, so `&mut self` serialises every write.
/// Each operation reads the clock once and commits one atomic change set.
///
/// Mutations are reached only through [`Self::as_origin`], so every change is
/// attributed per call and no attribution can outlive its operation.
pub struct SchedulerService<S, I, C> {
    pub(super) store: S,
    pub(super) ids: I,
    pub(super) clock: C,
    /// The attendance rules every status recount follows; set it from the
    /// guild's `SchedulePolicy.attendance` (v4-compatible by default).
    pub(super) attendance: AttendancePolicy,
    /// v5: when runs end, read once per operation; `None` keeps v4's rules
    /// (nothing frozen, slots past 2 h after their start: the vector replays).
    pub(super) run_ends: Option<RunEndsSource>,
}

/// One attributed mutation: `service.as_origin(origin).amend_run(..)`.
/// Each method consumes the handle, so it attributes exactly one operation.
pub struct Attributed<'a, S, I, C> {
    service: &'a mut SchedulerService<S, I, C>,
    origin: Origin,
    expect: Expect,
}

/// Delivery facts captured when a member declines.  The deciding RSVP commit
/// stores them with the durable candidate, rather than relying on a later
/// roster lookup for the member's name or source-message context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclineNoticeContext {
    pub channel_id: Option<String>,
    pub reference_id: Option<String>,
    pub display_name: String,
}

/// An RSVP write's observable decline side effects.  Callers invoke delivery
/// retraction only after this committed result is returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclineRsvpResult<T> {
    pub value: T,
    pub declined: bool,
    pub retract: bool,
}

impl<S: ScheduleStore, I: IdSource, C: Clock> SchedulerService<S, I, C> {
    pub fn new(store: S, ids: I, clock: C) -> Self {
        Self {
            store,
            ids,
            clock,
            attendance: AttendancePolicy::V4_COMPAT,
            run_ends: None,
        }
    }

    /// Recount statuses under `attendance` (the `SchedulePolicy`'s).
    #[must_use]
    pub fn with_attendance(mut self, attendance: AttendancePolicy) -> Self {
        self.attendance = attendance;
        self
    }

    pub fn attendance(&self) -> AttendancePolicy {
        self.attendance
    }

    /// Freeze runs past their end (user decision 2026-10-10) under `source`,
    /// read at each operation.
    #[must_use]
    pub fn with_run_ends(mut self, source: RunEndsSource) -> Self {
        self.run_ends = Some(source);
        self
    }

    /// The run ends as they stand now; `None` under v4 rules.
    pub fn run_ends(&self) -> Option<Arc<RunEnds>> {
        self.run_ends.as_ref().map(|source| Arc::new(source.now()))
    }

    pub fn run_ends_source(&self) -> Option<&RunEndsSource> {
        self.run_ends.as_ref()
    }

    /// A working copy of `snapshot` under this scheduler's rules.
    pub(super) fn draft(
        &self,
        snapshot: schedule::ScheduleSnapshot,
        ends: Option<Arc<RunEnds>>,
    ) -> Draft {
        Draft::new(snapshot)
            .with_attendance(self.attendance)
            .with_run_ends(ends)
    }

    /// The scheduler's attendance rules are the only source: a call whose
    /// `SchedulePolicy.attendance` differs is refused.
    ///
    /// # Errors
    /// [`ScheduleError::AttendanceMismatch`].
    pub(super) fn check_policy(&self, policy: &SchedulePolicy) -> Result<(), ScheduleError> {
        if policy.attendance == self.attendance {
            Ok(())
        } else {
            Err(ScheduleError::AttendanceMismatch {
                service: self.attendance,
                policy: policy.attendance,
            })
        }
    }

    /// A handle attributing its one mutation to `origin`.
    pub fn as_origin(&mut self, origin: Origin) -> Attributed<'_, S, I, C> {
        Attributed {
            service: self,
            origin,
            expect: Expect::default(),
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    /// The store, for tests that reopen it.
    pub fn into_store(self) -> S {
        self.store
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Load, plan and commit as `origin`; on a revision conflict re-load and
    /// re-plan at the same instant, up to [`COMMIT_ATTEMPTS`] times. A
    /// request id the store already recorded for this actor returns
    /// [`SchedulerError::AlreadyApplied`] instead of applying again.
    async fn transact_as<T>(
        &mut self,
        meta: ChangeMeta,
        scope: Scope,
        mut plan: impl FnMut(&mut Draft, &mut I, DateTime<Utc>) -> Result<T, ScheduleError>,
        notices: impl Fn(&T) -> (Vec<String>, Vec<Notice>),
    ) -> SchedulerResult<T> {
        check_request(&self.store, &meta).await?;
        let now = self.clock.now();
        let ends = self.run_ends();
        let mut attempt = 1;
        loop {
            let snapshot = self.store.load(&scope).await?;
            let revision = snapshot.revision;
            let mut draft = self.draft(snapshot, ends.clone());
            let value = match plan(&mut draft, &mut self.ids, now) {
                Ok(value) => value,
                Err(error) => return Err(self.explain_refusal(revision, &meta, error).await),
            };
            let changes = draft.into_changes();
            if changes.is_empty() {
                self.check_no_op(revision, &meta).await?;
                return Ok(value);
            }
            let (kinds, outbox) = notices(&value);
            let meta = ChangeMeta {
                at: now,
                notices: kinds,
                outbox,
                ..meta.clone()
            };
            match self.store.commit(revision, changes, meta).await {
                Ok(Some(committed)) if committed.replayed => {
                    return Err(already_applied(committed));
                }
                Ok(_) => return Ok(value),
                Err(StoreError::Conflict { .. }) if attempt < COMMIT_ATTEMPTS => attempt += 1,
                Err(error) => return Err(store_failure(error)),
            }
        }
    }

    /// A plan refused under declared preconditions may be refused because
    /// the row changed or went away since the caller read it: an empty
    /// commit runs the store's precondition check (writing nothing) and a
    /// stale field is reported instead of the rule refusal.
    async fn explain_refusal(
        &mut self,
        revision: u64,
        meta: &ChangeMeta,
        error: ScheduleError,
    ) -> SchedulerError {
        if meta.expect.fields.is_empty() {
            return error.into();
        }
        match self
            .store
            .commit(revision, schedule::ChangeSet::default(), meta.clone())
            .await
        {
            Err(StoreError::StaleEdit(conflicts)) => SchedulerError::StaleEdit { conflicts },
            Err(StoreError::Precondition(refusal)) => SchedulerError::Precondition(refusal),
            _ => error.into(),
        }
    }

    /// An edit that changes nothing still refuses a deleted (or never
    /// existing) target it declared; other stale fields are fine, the
    /// result is the same either way. An override of nothing is refused.
    async fn check_no_op(&mut self, revision: u64, meta: &ChangeMeta) -> SchedulerResult<()> {
        if let Some(reference) = meta.expect.overrides.first() {
            return Err(SchedulerError::Precondition(
                PreconditionError::OverrideUnchanged { seq: reference.seq },
            ));
        }
        if meta.expect.fields.is_empty() {
            return Ok(());
        }
        match self
            .store
            .commit(revision, schedule::ChangeSet::default(), meta.clone())
            .await
        {
            Err(StoreError::StaleEdit(conflicts)) => {
                let deleted: Vec<StaleField> = conflicts
                    .into_iter()
                    .filter(|conflict| conflict.target_deleted)
                    .collect();
                if deleted.is_empty() {
                    Ok(())
                } else {
                    Err(SchedulerError::StaleEdit { conflicts: deleted })
                }
            }
            Err(StoreError::Precondition(refusal)) => Err(SchedulerError::Precondition(refusal)),
            // A revision race or a replayed request is not this check's
            // concern: nothing was to be written.
            _ => Ok(()),
        }
    }

    /// v5 attendance: `member`'s patterns over their last
    /// [`attendance::RECENT_RUNS_PER_TIMING`] recorded runs per timing, and
    /// the timings to suggest "always in" on (never applied). Only an
    /// administrator or the member themselves may read them.
    ///
    /// # Errors
    /// [`SchedulerError::Forbidden`] for anyone else, or the store failed.
    pub async fn member_attendance(
        &self,
        viewer: &Actor,
        member: &str,
    ) -> SchedulerResult<attendance::MemberAttendance>
    where
        S: AttendanceHistory,
    {
        let allowed = match viewer {
            Actor::Admin { .. } => true,
            Actor::Member { id } => id == member,
            Actor::System { .. } => false,
        };
        if !allowed {
            return Err(SchedulerError::Forbidden(
                "attendance patterns are for administrators and the member".into(),
            ));
        }
        let history = self
            .store
            .member_history(member, attendance::RECENT_RUNS_PER_TIMING)
            .await?;
        let timings = self.store.load(&Scope::Weeks(Vec::new())).await?.fixed_runs;
        let suggest_standing = timings
            .iter()
            .filter(|timing| {
                timing.participants.iter().any(|user| user == member)
                    && !timing
                        .standing
                        .iter()
                        .any(|answer| answer.user_id == member)
                    && attendance::suggest_standing(&history, &timing.id)
            })
            .map(|timing| timing.id.clone())
            .collect();
        Ok(attendance::MemberAttendance {
            member: member.to_owned(),
            patterns: attendance::attendance_patterns(&history),
            history,
            suggest_standing,
        })
    }

    /// The amended runs a weekly-timing edit would move; choose for each
    /// before [`Self::apply_fixed_edit`].
    pub async fn preview_fixed_edit(
        &self,
        fixed_id: &str,
        edit: &FixedEdit,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Vec<AmendedRun>> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let draft = self.draft(self.store.load(&Scope::All).await?, self.run_ends());
        Ok(schedule::preview_fixed_edit(
            &draft, fixed_id, edit, policy, now,
        )?)
    }

    /// One run's reminders ordered by `(fire_at, kind)`.
    pub async fn reminders(&self, run_id: &str) -> SchedulerResult<Vec<Reminder>> {
        let draft = Draft::new(self.store.load(&Scope::Run(run_id.to_owned())).await?);
        Ok(draft.reminders(run_id))
    }
}

/// v4 parity (parent decision 2026-09-25): a move or weekly-timing change
/// made in Discord carries no "(via portal)" mark, and a Discord `/fixed
/// edit` announces nothing (v4's slash edit posted nothing). Every other
/// surface keeps the mark and the announcement.
fn on_surface(op: &Op<'_>, surface: Surface, mut outcome: Outcome<OpResult>) -> Outcome<OpResult> {
    if surface == Surface::Import {
        outcome.notices.clear();
        return outcome;
    }
    let discord = surface == Surface::Discord;
    match op {
        Op::ApplyFixedEdit { .. } if discord => outcome.notices.clear(),
        Op::AmendRun { .. }
        | Op::SwapRunSlots { .. }
        | Op::ApplyFixedEdit { .. }
        | Op::FixedParticipants { .. }
        | Op::AddFixedRun(_)
        | Op::AddFixedRunMaterialised { .. }
        | Op::RetireFixedRun { .. } => {
            for notice in &mut outcome.notices {
                notice.via_portal = !discord;
            }
        }
        _ => {}
    }
    outcome
}

impl<S: ScheduleStore, I: IdSource, C: Clock> Attributed<'_, S, I, C> {
    /// Declare what the caller saw of the fields this edit changes; the
    /// store refuses the commit with [`SchedulerError::StaleEdit`] when any
    /// of them changed since. With `expect.overrides` (administrators only)
    /// the edit is recorded as overriding those changes.
    #[must_use]
    pub fn expecting(mut self, expect: Expect) -> Self {
        self.expect = expect;
        self
    }

    /// Apply `op` in one attributed transaction (the single mutation path);
    /// its request digest and notices are recorded with the change.
    async fn apply(self, scope: Scope, op: Op<'_>) -> SchedulerResult<Outcome<OpResult>> {
        self.apply_guarded(scope, op, |_, _| Ok(())).await
    }

    /// [`Self::apply`] with `guard` checked on each planning attempt's draft,
    /// so a refusal and the commit read the same revision.
    async fn apply_guarded(
        self,
        scope: Scope,
        op: Op<'_>,
        guard: impl Fn(&Draft, DateTime<Utc>) -> Result<(), ScheduleError>,
    ) -> SchedulerResult<Outcome<OpResult>> {
        if let Some(policy) = op.schedule_policy() {
            self.service.check_policy(policy)?;
        }
        self.expect
            .validate(&self.origin.actor)
            .map_err(SchedulerError::Precondition)?;
        let request = edit_digest(&op, &self.expect)?;
        let overriding = !self.expect.overrides.is_empty();
        let surface = self.origin.surface;
        let meta = ChangeMeta {
            origin: self.origin.clone(),
            at: DateTime::UNIX_EPOCH,
            notices: Vec::new(),
            refs: self.expect.overrides.clone(),
            request_digest: self.origin.request_id.as_ref().map(|_| request),
            expect: self.expect,
            outbox: Vec::new(),
        };
        self.service
            .transact_as(
                meta,
                scope,
                |draft, ids, now| {
                    guard(draft, now)?;
                    apply_op(draft, ids, &op, now).map(|outcome| on_surface(&op, surface, outcome))
                },
                |outcome: &Outcome<OpResult>| {
                    let mut kinds: Vec<String> =
                        outcome.notices.iter().map(Notice::effect_kind).collect();
                    if overriding {
                        kinds.push(EDIT_OVERRIDE.to_owned());
                    }
                    (kinds, outcome.notices.clone())
                },
            )
            .await
    }

    pub async fn add_fixed_run(self, new: NewFixedRun) -> SchedulerResult<String> {
        let outcome = self
            .apply(Scope::Weeks(Vec::new()), Op::AddFixedRun(new))
            .await?;
        Ok(created(outcome.value))
    }

    /// A new weekly timing with its runs in the materialised weeks, in one
    /// change: a refused or failed commit leaves neither. The request digest
    /// is [`Self::add_fixed_run`]'s, so a key a plain create recorded still
    /// replays (its runs then come from the next materialisation).
    pub async fn add_fixed_run_materialised(
        self,
        new: NewFixedRun,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<String> {
        let outcome = self
            .apply(Scope::All, Op::AddFixedRunMaterialised { new, policy })
            .await?;
        Ok(created(outcome.value))
    }

    pub async fn create_run(self, new: NewRun) -> SchedulerResult<String> {
        let scope = Scope::Weeks(vec![new.week_start]);
        let outcome = self.apply(scope, Op::CreateRun(new)).await?;
        Ok(created(outcome.value))
    }

    /// Materialise one boss week; returns the ids of newly created runs.
    pub async fn materialise_week(
        self,
        week_start: impl AwareDateTime,
        policy: &ReminderPolicy,
    ) -> SchedulerResult<Vec<String>> {
        let scope = weeks_scope(std::slice::from_ref(&week_start))?;
        let week_start = WeekStart::new(&week_start, policy.zone).map_err(ScheduleError::from)?;
        let op = Op::MaterialiseWeek { week_start, policy };
        Ok(ids(self.apply(scope, op).await?.value))
    }

    /// Set a run's status directly; an absent run is a no-op, as v4.
    pub async fn set_run_status(self, run_id: &str, status: RunStatus) -> SchedulerResult<()> {
        let op = Op::SetRunStatus {
            run_id: run_id.to_owned(),
            status,
        };
        self.apply(Scope::Run(run_id.to_owned()), op).await?;
        Ok(())
    }

    pub async fn set_rsvp(
        self,
        run_id: &str,
        user_id: &str,
        state: RsvpState,
        source: RsvpSource,
    ) -> SchedulerResult<()> {
        let op = Op::SetRsvp {
            run_id: run_id.to_owned(),
            user_id: user_id.to_owned(),
            state,
            source,
        };
        self.apply(Scope::Run(run_id.to_owned()), op).await?;
        Ok(())
    }

    /// A portal answer (v4 `api/service.py set_rsvp`): set (source `chat`) or
    /// clear one participant's answer, then re-derive the run's status. Unlike
    /// a reaction it never ends a status pin (`docs/notes/attendance.md`), and any
    /// answer, `maybe` included, can be cleared. Returns whether the status
    /// changed.
    ///
    /// # Errors
    /// [`ScheduleError::UnknownRun`], [`ScheduleError::NotOnRun`] for someone
    /// not on the run, or as any edit (preconditions on `rsvp:<user>`).
    pub async fn portal_answer(
        self,
        run_id: &str,
        user_id: &str,
        answer: Option<RsvpState>,
    ) -> SchedulerResult<bool> {
        self.expect
            .validate(&self.origin.actor)
            .map_err(SchedulerError::Precondition)?;
        let request = digest("portal_answer", &(run_id, user_id, answer));
        let request = if self.expect.is_empty() {
            request
        } else {
            digest("expect", &(request, &self.expect.canonical()))
        };
        let overriding = !self.expect.overrides.is_empty();
        let meta = ChangeMeta {
            origin: self.origin.clone(),
            at: DateTime::UNIX_EPOCH,
            notices: Vec::new(),
            refs: self.expect.overrides.clone(),
            request_digest: self.origin.request_id.as_ref().map(|_| request),
            expect: self.expect,
            outbox: Vec::new(),
        };
        let (run, user) = (run_id.to_owned(), user_id.to_owned());
        self.service
            .transact_as(
                meta,
                Scope::Run(run_id.to_owned()),
                move |draft, _, now| {
                    let before = draft.require_run(&run)?;
                    if !before.participants.contains(&user) {
                        return Err(ScheduleError::NotOnRun(vec![user.clone()]));
                    }
                    draft.refuse_ended(&run, now)?;
                    match answer {
                        Some(state) => draft.set_rsvp(&run, &user, state, RsvpSource::Chat, now),
                        None => draft.clear_rsvp(&run, &user),
                    }
                    // derive_run_status keeps a pinned (or started) run's status.
                    let after = draft.require_run(&run)?;
                    let status =
                        schedule::derive_run_status(draft, &after, after.status, now).status;
                    if status != after.status {
                        draft.set_run_status(&run, status);
                    }
                    Ok(status != after.status)
                },
                |_| {
                    let kinds = if overriding {
                        vec![EDIT_OVERRIDE.to_owned()]
                    } else {
                        Vec::new()
                    };
                    (kinds, Vec::new())
                },
            )
            .await
    }

    pub async fn apply_reaction(
        self,
        run_id: &str,
        user_id: &str,
        emoji: &str,
        added: bool,
    ) -> SchedulerResult<ReactionResult> {
        let op = Op::ApplyReaction {
            run_id: run_id.to_owned(),
            user_id: user_id.to_owned(),
            emoji: emoji.to_owned(),
            added,
        };
        match self.apply(Scope::Run(run_id.to_owned()), op).await?.value {
            OpResult::Reaction(result) => Ok(result),
            other => unreachable!("apply_reaction returned {other:?}"),
        }
    }

    /// Update a weekly timing and push the `changed` fields onto its runs in
    /// `week_starts`, atomically. Returns how many runs were touched.
    pub async fn edit_fixed_run(
        self,
        fixed_id: &str,
        patch: FixedRunPatch,
        changed: &[FixedField],
        week_starts: &[impl AwareDateTime],
        policy: &ReminderPolicy,
    ) -> SchedulerResult<usize> {
        let scope = weeks_scope(week_starts)?;
        let op = Op::EditFixedRun {
            fixed_id: fixed_id.to_owned(),
            patch,
            changed: changed.to_vec(),
            week_starts: week_starts_of(week_starts, policy)?,
            policy,
        };
        Ok(count(self.apply(scope, op).await?.value))
    }

    /// Delete a weekly timing and cancel its live runs in `week_starts`.
    pub async fn retire_fixed_run(
        self,
        fixed_id: &str,
        week_starts: &[impl AwareDateTime],
        policy: &ReminderPolicy,
    ) -> SchedulerResult<usize> {
        let scope = weeks_scope(week_starts)?;
        let op = Op::RetireFixedRun {
            fixed_id: fixed_id.to_owned(),
            week_starts: week_starts_of(week_starts, policy)?,
            policy,
        };
        Ok(count(self.apply(scope, op).await?.value))
    }

    pub async fn ensure_reminders(
        self,
        run_id: &str,
        policy: &ReminderPolicy,
        rebuild: bool,
    ) -> SchedulerResult<Vec<String>> {
        let op = Op::EnsureReminders {
            run_id: run_id.to_owned(),
            rebuild,
            policy,
        };
        Ok(ids(self
            .apply(Scope::Run(run_id.to_owned()), op)
            .await?
            .value))
    }

    /// Insert one reminder row; `None` when the run already has that kind.
    pub async fn add_reminder(
        self,
        run_id: &str,
        kind: &str,
        fire_at: DateTime<Utc>,
        sent_at: Option<DateTime<Utc>>,
    ) -> SchedulerResult<Option<String>> {
        let op = Op::AddReminder {
            run_id: run_id.to_owned(),
            kind: kind.to_owned(),
            fire_at,
            sent_at,
        };
        match self.apply(Scope::Run(run_id.to_owned()), op).await?.value {
            OpResult::Reminder(id) => Ok(id),
            other => unreachable!("add_reminder returned {other:?}"),
        }
    }

    /// Record that a reminder was posted now.
    pub async fn mark_reminder_sent(
        self,
        reminder_id: &str,
        message_id: Option<&str>,
    ) -> SchedulerResult<()> {
        let op = Op::MarkReminderSent {
            reminder_id: reminder_id.to_owned(),
            message_id: message_id.map(str::to_owned),
        };
        self.apply(Scope::Reminder(reminder_id.to_owned()), op)
            .await?;
        Ok(())
    }

    pub async fn reschedule_unposted_reminder(
        self,
        reminder_id: &str,
        fire_at: DateTime<Utc>,
    ) -> SchedulerResult<bool> {
        let op = Op::RescheduleUnpostedReminder {
            reminder_id: reminder_id.to_owned(),
            fire_at,
        };
        match self
            .apply(Scope::Reminder(reminder_id.to_owned()), op)
            .await?
            .value
        {
            OpResult::Changed(changed) => Ok(changed),
            other => unreachable!("reschedule_unposted_reminder returned {other:?}"),
        }
    }

    /// Move unposted day-of pings after the ping time changed.
    pub async fn reconcile_day_of(self, policy: &ReminderPolicy) -> SchedulerResult<usize> {
        let op = Op::ReconcileDayOf { policy };
        Ok(count(self.apply(Scope::All, op).await?.value))
    }

    /// Retire live runs whose slot has passed.
    pub async fn mark_done(self) -> SchedulerResult<Vec<String>> {
        Ok(ids(self.apply(Scope::All, Op::MarkDone).await?.value))
    }

    /// v5 (run completion): mark one live run done at its cutoff, without a
    /// notice; `false` when it was no longer live.
    pub async fn finish_run(self, run_id: &str) -> SchedulerResult<bool> {
        let op = Op::FinishRun {
            run_id: run_id.to_owned(),
        };
        match self.apply(Scope::Run(run_id.to_owned()), op).await?.value {
            OpResult::Changed(changed) => Ok(changed),
            other => unreachable!("finish_run returned {other:?}"),
        }
    }

    /// v5 (run completion): a prompt press, applied only if the run is still
    /// live, unmoved and the presser's to settle on the committed state.
    pub async fn settle_run(
        self,
        settle: crate::domain::schedule::SettleRun,
        policy: &ReminderPolicy,
    ) -> SchedulerResult<bool> {
        let scope = Scope::Run(settle.run_id.clone());
        match self
            .apply(scope, Op::SettleRun { settle, policy })
            .await?
            .value
        {
            OpResult::Changed(changed) => Ok(changed),
            other => unreachable!("settle_run returned {other:?}"),
        }
    }

    /// Materialise the current and next two boss weeks, then reconcile
    /// day-of pings (v4 `BossBot.materialise_weeks`). Returns created run ids.
    pub async fn materialise_weeks(self, policy: &SchedulePolicy) -> SchedulerResult<Vec<String>> {
        let op = Op::MaterialiseWeeks { policy };
        Ok(ids(self.apply(Scope::All, op).await?.value))
    }

    /// Set a run's status by hand, rebuilding reminders (v4 `service.set_status`).
    pub async fn set_status(
        self,
        run_id: &str,
        change: StatusChange,
        policy: &ReminderPolicy,
    ) -> SchedulerResult<Outcome<RunState>> {
        let op = Op::SetStatus {
            run_id: run_id.to_owned(),
            change,
            policy,
        };
        Ok(run_state(
            self.apply(Scope::Run(run_id.to_owned()), op).await?,
        ))
    }

    /// Move a run to `to`, possibly into another boss week (v4 `amend_run`).
    pub async fn amend_run(
        self,
        run_id: &str,
        to: DateTime<Utc>,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<RunState>> {
        let op = Op::AmendRun {
            run_id: run_id.to_owned(),
            to,
            policy,
        };
        // The target week's runs decide conflicts, so the whole schedule is read.
        Ok(run_state(self.apply(Scope::All, op).await?))
    }

    /// A member moves their own run (public portal): [`Self::amend_run`],
    /// refused inside the commit unless they are on the live run, it has not
    /// ended, it is in the current boss week and has not started, and `to` is
    /// still ahead. The request digest is `amend_run`'s.
    pub async fn member_amend_run(
        self,
        member: &str,
        run_id: &str,
        to: DateTime<Utc>,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<RunState>> {
        let op = Op::AmendRun {
            run_id: run_id.to_owned(),
            to,
            policy,
        };
        let guard = |draft: &Draft, now| {
            let run = draft.require_run(run_id)?;
            let weeks = member_weeks(policy, now, 1)?;
            let ended = draft.ended(&run, now);
            MemberRunRefusal::check(&run, member, &weeks, Some(to), ended, now)
                .map_err(ScheduleError::MemberRun)
        };
        Ok(run_state(self.apply_guarded(Scope::All, op, guard).await?))
    }

    /// Exchange two runs' slots as one history-recorded transaction.
    pub async fn swap_run_slots(
        self,
        run_id: &str,
        with_id: &str,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<Vec<RunState>>> {
        self.swap_run_slots_at_version(run_id, with_id, None, policy)
            .await
    }

    /// Portal swaps include the raw version in their request digest, so a
    /// reused idempotency key cannot replay a request from another screen read.
    pub async fn swap_run_slots_at_version(
        self,
        run_id: &str,
        with_id: &str,
        request_version: Option<u64>,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<Vec<RunState>>> {
        let op = Op::SwapRunSlots {
            run_id: run_id.to_owned(),
            with_id: with_id.to_owned(),
            request_version,
            policy,
        };
        match self.apply(Scope::All, op).await? {
            Outcome {
                value: OpResult::Runs(runs),
                notices,
            } => Ok(Outcome {
                value: runs,
                notices,
            }),
            Outcome { value, .. } => unreachable!("swap_run_slots returned {value:?}"),
        }
    }

    /// Change this week's line-up of one run (v4 `swap_participants`).
    pub async fn swap_participants(
        self,
        run_id: &str,
        remove: &[String],
        add: &[String],
        via_portal: bool,
        directory: &(impl Directory + Sync),
    ) -> SchedulerResult<Outcome<RunState>> {
        let op = Op::SwapParticipants {
            run_id: run_id.to_owned(),
            remove: remove.to_vec(),
            add: add.to_vec(),
            via_portal,
            directory,
        };
        Ok(run_state(
            self.apply(Scope::Run(run_id.to_owned()), op).await?,
        ))
    }

    /// Apply a weekly-timing edit with explicit choices for amended runs.
    pub async fn apply_fixed_edit(
        self,
        request: &FixedEditRequest,
        directory: &(impl Directory + Sync),
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<FixedRun>> {
        let op = Op::ApplyFixedEdit {
            request: request.clone(),
            directory,
            policy,
        };
        let outcome = self.apply(Scope::All, op).await?;
        match outcome.value {
            OpResult::Fixed(fixed) => Ok(Outcome {
                value: fixed,
                notices: outcome.notices,
            }),
            other => unreachable!("apply_fixed_edit returned {other:?}"),
        }
    }

    /// v4 `update_fixed`: apply an edit, every amended run following it.
    pub async fn update_fixed(
        self,
        fixed_id: &str,
        edit: FixedEdit,
        directory: &(impl Directory + Sync),
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<FixedRun>> {
        let request = FixedEditRequest {
            fixed_id: fixed_id.to_owned(),
            edit,
            choices: schedule::FixedEditChoices::UpdateAll,
        };
        self.apply_fixed_edit(&request, directory, policy).await
    }

    /// Put an amended run back on its weekly timing for its week.
    pub async fn reset_to_fixed(
        self,
        run_id: &str,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<RunState>> {
        let op = Op::ResetToFixed {
            run_id: run_id.to_owned(),
            policy,
        };
        Ok(run_state(
            self.apply(Scope::Run(run_id.to_owned()), op).await?,
        ))
    }

    /// v5 attendance: set (`on`) or clear `member`'s standing answer ("always
    /// in") on a weekly timing. A member may set only their own, and only
    /// on a timing whose party they are in; an administrator may set anyone
    /// in the party. Returns the timing.
    ///
    /// # Errors
    /// [`SchedulerError::Forbidden`] for a member acting for someone else,
    /// [`ScheduleError::NotInParty`], or as any edit (preconditions on
    /// `standing:<member>`).
    pub async fn set_standing_answer(
        self,
        member: &str,
        fixed_id: &str,
        on: bool,
    ) -> SchedulerResult<FixedRun> {
        let actor = &self.origin.actor;
        if matches!(actor, Actor::Member { id } if id != member) {
            return Err(SchedulerError::Forbidden(
                "members set only their own standing answer".into(),
            ));
        }
        let op = Op::SetStandingAnswer {
            fixed_id: fixed_id.to_owned(),
            user_id: member.to_owned(),
            on,
            set_by: format!("{}:{}", actor.kind(), actor.id()),
        };
        // Every run is loaded: the timing's runs are re-derived (v5).
        fixed(self.apply(Scope::All, op).await?.value)
    }

    /// v5 attendance: a weekly timing's default for unanswered members,
    /// administrators only. Returns the timing.
    ///
    /// # Errors
    /// [`SchedulerError::Forbidden`] for a non-administrator, or as any
    /// edit (preconditions on `attendance_default`).
    pub async fn set_attendance_default(
        self,
        fixed_id: &str,
        default: crate::domain::attendance::AttendanceDefault,
    ) -> SchedulerResult<FixedRun> {
        if !matches!(self.origin.actor, Actor::Admin { .. }) {
            return Err(SchedulerError::Forbidden(
                "only administrators set a timing's attendance default".into(),
            ));
        }
        let op = Op::SetAttendanceDefault {
            fixed_id: fixed_id.to_owned(),
            default,
        };
        fixed(self.apply(Scope::All, op).await?.value)
    }

    /// v5 attendance: record who attended a done run. `attended` is the
    /// whole attended set as the caller sees it (start from
    /// [`attendance::prefill`]); administrators record anyone, a member only
    /// their own entry. Returns the run with its recorded attendance.
    ///
    /// # Errors
    /// [`SchedulerError::Forbidden`] for a system actor or a member changing
    /// someone else's entry, [`ScheduleError::Attendance`] (`NotDone`,
    /// `NotOnRun`), or as any edit (preconditions on `attended:<user>`).
    pub async fn record_attendance(
        self,
        run_id: &str,
        attended: &BTreeSet<String>,
    ) -> SchedulerResult<RunState> {
        let actor = &self.origin.actor;
        let who = match actor {
            Actor::Admin { .. } => AttendanceActor::Admin,
            Actor::Member { id } => AttendanceActor::Member(id.clone()),
            Actor::System { .. } => {
                return Err(SchedulerError::Forbidden(
                    "attendance is recorded by administrators and members".into(),
                ));
            }
        };
        let op = Op::RecordAttendance {
            run_id: run_id.to_owned(),
            actor: who,
            attended: attended.clone(),
            recorded_by: format!("{}:{}", actor.kind(), actor.id()),
        };
        match self.apply(Scope::Run(run_id.to_owned()), op).await {
            Ok(outcome) => Ok(run_state(outcome).value),
            Err(SchedulerError::Schedule(ScheduleError::Attendance(
                refusal @ AttendanceRefusal::OthersNotAllowed,
            ))) => Err(SchedulerError::Forbidden(refusal.to_string())),
            Err(error) => Err(error),
        }
    }

    /// v5 attendance: re-derive live runs' statuses now (the tick's step, so
    /// a run turns at risk when its unknown window opens). Returns the runs
    /// whose status changed; nothing in v4-compat mode.
    pub async fn recount_attendance(self) -> SchedulerResult<Vec<String>> {
        if self.service.attendance.mode == AttendanceMode::V4Compat {
            return Ok(Vec::new());
        }
        Ok(ids(self
            .apply(Scope::All, Op::RecountAttendance)
            .await?
            .value))
    }

    /// v5 only: change a weekly timing's party by delta (remove, then add),
    /// applying the same delta to its live runs so one-off substitutions
    /// survive; runs it would empty are left alone and reported.
    pub async fn change_fixed_party(
        self,
        fixed_id: &str,
        add: &[String],
        remove: &[String],
        directory: &(impl Directory + Sync),
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Outcome<schedule::PartyDelta>> {
        let op = Op::FixedParticipants {
            fixed_id: fixed_id.to_owned(),
            add: add.to_vec(),
            remove: remove.to_vec(),
            directory,
            policy,
        };
        let outcome = self.apply(Scope::All, op).await?;
        match outcome.value {
            OpResult::PartyDelta(delta) => Ok(Outcome {
                value: delta,
                notices: outcome.notices,
            }),
            other => unreachable!("change_fixed_party returned {other:?}"),
        }
    }
}

impl<S: ScheduleStore + DeclineNoticeStore, I: IdSource, C: Clock> Attributed<'_, S, I, C> {
    async fn commit_rsvp_with_decline<T>(
        self,
        run_id: &str,
        user_id: &str,
        context: DeclineNoticeContext,
        request: String,
        mut plan: impl FnMut(
            &mut Draft,
            DateTime<Utc>,
        ) -> Result<(T, bool, bool, Option<String>), ScheduleError>,
    ) -> SchedulerResult<DeclineRsvpResult<T>> {
        self.expect
            .validate(&self.origin.actor)
            .map_err(SchedulerError::Precondition)?;
        let now = self.service.clock.now();
        let request = if self.expect.is_empty() {
            request
        } else {
            digest("expect", &(request, &self.expect.canonical()))
        };
        let meta = ChangeMeta {
            origin: self.origin.clone(),
            at: now,
            notices: Vec::new(),
            refs: self.expect.overrides.clone(),
            request_digest: self.origin.request_id.as_ref().map(|_| request),
            expect: self.expect,
            outbox: Vec::new(),
        };
        check_request(&self.service.store, &meta).await?;
        let ends = self.service.run_ends();
        let mut attempt = 1;
        loop {
            let snapshot = self
                .service
                .store
                .load(&Scope::Run(run_id.to_owned()))
                .await?;
            let revision = snapshot.revision;
            let mut draft = self.service.draft(snapshot, ends.clone());
            let (value, declined, retract, home_channel) = match plan(&mut draft, now) {
                Ok(value) => value,
                Err(error) => {
                    return Err(self.service.explain_refusal(revision, &meta, error).await);
                }
            };
            let changes = draft.into_changes();
            if changes.is_empty() {
                self.service.check_no_op(revision, &meta).await?;
                return Ok(DeclineRsvpResult {
                    value,
                    declined: false,
                    retract: false,
                });
            }
            let candidates = if declined
                && self.origin.surface != Surface::Import
                && !self
                    .service
                    .store
                    .decline_notice_on_cooldown(run_id, user_id, now)
                    .await?
            {
                vec![DeclineNotice::candidate(
                    run_id,
                    user_id,
                    context.channel_id.clone().or(home_channel),
                    context.reference_id.clone(),
                    context.display_name.clone(),
                    now,
                )]
            } else {
                Vec::new()
            };
            match self
                .service
                .store
                .commit_with_decline_notices(
                    revision,
                    changes,
                    meta.clone(),
                    candidates,
                    retract
                        .then(|| (run_id.to_owned(), user_id.to_owned()))
                        .into_iter()
                        .collect(),
                )
                .await
            {
                Ok(Some(committed)) if committed.replayed => {
                    return Err(already_applied(committed));
                }
                Ok(_) => {
                    return Ok(DeclineRsvpResult {
                        value,
                        declined,
                        retract,
                    });
                }
                Err(StoreError::Conflict { .. }) if attempt < COMMIT_ATTEMPTS => attempt += 1,
                Err(error) => return Err(store_failure(error)),
            }
        }
    }

    /// Apply a card RSVP while atomically recording a new decline candidate.
    pub async fn apply_reaction_with_decline(
        self,
        run_id: &str,
        user_id: &str,
        emoji: &str,
        added: bool,
        context: DeclineNoticeContext,
    ) -> SchedulerResult<DeclineRsvpResult<ReactionResult>> {
        let request = digest("apply_reaction", &(run_id, user_id, emoji, added));
        self.commit_rsvp_with_decline(run_id, user_id, context, request, |draft, now| {
            let was_no = draft.rsvps(run_id).get(user_id) == Some(&RsvpState::No);
            let result = schedule::apply_reaction(draft, run_id, user_id, emoji, added, now)?;
            let home = draft.require_run(run_id)?.channel_id.clone();
            Ok((
                result.clone(),
                result.applied && result.state == Some(RsvpState::No),
                result.applied && was_no && result.state != Some(RsvpState::No),
                home,
            ))
        })
        .await
    }

    /// Set or clear a portal-style RSVP with v4's decline/retraction effects.
    pub async fn portal_answer_with_decline(
        self,
        run_id: &str,
        user_id: &str,
        answer: Option<RsvpState>,
        context: DeclineNoticeContext,
    ) -> SchedulerResult<DeclineRsvpResult<bool>> {
        self.answer_with_decline(run_id, user_id, answer, context, None)
            .await
    }

    /// A member's own answer (public portal): [`Self::portal_answer_with_decline`],
    /// refused inside the commit unless the run is live, has not ended and is
    /// in this or next boss week.
    pub async fn member_answer_with_decline(
        self,
        run_id: &str,
        user_id: &str,
        answer: RsvpState,
        context: DeclineNoticeContext,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<DeclineRsvpResult<bool>> {
        self.answer_with_decline(run_id, user_id, Some(answer), context, Some(policy))
            .await
    }

    async fn answer_with_decline(
        self,
        run_id: &str,
        user_id: &str,
        answer: Option<RsvpState>,
        context: DeclineNoticeContext,
        member: Option<&SchedulePolicy>,
    ) -> SchedulerResult<DeclineRsvpResult<bool>> {
        let request = digest("portal_answer", &(run_id, user_id, answer));
        self.commit_rsvp_with_decline(run_id, user_id, context, request, |draft, now| {
            if let Some(policy) = member {
                let run = draft.require_run(run_id)?;
                let weeks = member_weeks(policy, now, 2)?;
                let ended = draft.ended(&run, now);
                MemberRunRefusal::check(&run, user_id, &weeks, None, ended, now)
                    .map_err(ScheduleError::MemberRun)?;
            }
            let before = draft.require_run(run_id)?;
            if !before.participants.iter().any(|user| user == user_id) {
                return Err(ScheduleError::NotOnRun(vec![user_id.to_owned()]));
            }
            draft.refuse_ended(run_id, now)?;
            let was_no = draft.rsvps(run_id).get(user_id) == Some(&RsvpState::No);
            match answer {
                Some(state) => draft.set_rsvp(run_id, user_id, state, RsvpSource::Chat, now),
                None => draft.clear_rsvp(run_id, user_id),
            }
            let after = draft.require_run(run_id)?;
            let status = schedule::derive_run_status(draft, &after, after.status, now).status;
            let changed = status != after.status;
            if changed {
                draft.set_run_status(run_id, status);
            }
            Ok((
                changed,
                answer == Some(RsvpState::No),
                was_no && answer != Some(RsvpState::No),
                after.channel_id,
            ))
        })
        .await
    }
}

fn fixed(result: OpResult) -> SchedulerResult<FixedRun> {
    match result {
        OpResult::Fixed(row) => Ok(row),
        other => unreachable!("expected a weekly timing, got {other:?}"),
    }
}

fn created(result: OpResult) -> String {
    match result {
        OpResult::Created(id) => id,
        other => unreachable!("expected a created id, got {other:?}"),
    }
}

fn ids(result: OpResult) -> Vec<String> {
    match result {
        OpResult::Ids(ids) => ids,
        other => unreachable!("expected ids, got {other:?}"),
    }
}

fn count(result: OpResult) -> usize {
    match result {
        OpResult::Count(count) => count,
        other => unreachable!("expected a count, got {other:?}"),
    }
}

/// The first `count` boss weeks from the one holding `now` (start instants).
fn member_weeks(
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
    count: usize,
) -> Result<Vec<DateTime<Utc>>, ScheduleError> {
    policy
        .materialised_weeks(now)?
        .iter()
        .take(count)
        .map(|week| utc_instant(week).map_err(ScheduleError::from))
        .collect()
}

fn run_state(outcome: Outcome<OpResult>) -> Outcome<RunState> {
    match outcome.value {
        OpResult::Run(state) => Outcome {
            value: state,
            notices: outcome.notices,
        },
        other => unreachable!("expected a run state, got {other:?}"),
    }
}

fn week_starts_of(
    week_starts: &[impl AwareDateTime],
    policy: &ReminderPolicy,
) -> SchedulerResult<Vec<WeekStart>> {
    week_starts
        .iter()
        .map(|week| WeekStart::new(week, policy.zone))
        .collect::<Result<_, _>>()
        .map_err(|error| SchedulerError::Schedule(ScheduleError::from(error)))
}

fn instants_scope(week_starts: &[WeekStart]) -> Scope {
    Scope::Weeks(week_starts.iter().map(WeekStart::instant).collect())
}

/// The request digest of an operation: the operation's name and the
/// `Debug` form of its arguments, exactly as the pre-`apply_op` methods
/// hashed them (pinned by `digest_pins`).
fn op_digest(op: &Op<'_>) -> SchedulerResult<String> {
    Ok(match op {
        Op::AddFixedRun(new) | Op::AddFixedRunMaterialised { new, .. } => {
            digest("add_fixed_run", new)
        }
        Op::CreateRun(new) => digest("create_run", new),
        Op::MaterialiseWeek { week_start, .. } => digest(
            "materialise_week",
            &instants_scope(std::slice::from_ref(week_start)),
        ),
        Op::SetRunStatus { run_id, status } => digest("set_run_status", &(run_id, status)),
        Op::SetRsvp {
            run_id,
            user_id,
            state,
            source,
        } => digest("set_rsvp", &(run_id, user_id, state, source)),
        Op::ApplyReaction {
            run_id,
            user_id,
            emoji,
            added,
        } => digest("apply_reaction", &(run_id, user_id, emoji, added)),
        Op::EditFixedRun {
            fixed_id,
            patch,
            changed,
            week_starts,
            ..
        } => digest(
            "edit_fixed_run",
            &(fixed_id, patch, changed, &instants_scope(week_starts)),
        ),
        Op::RetireFixedRun {
            fixed_id,
            week_starts,
            ..
        } => digest(
            "retire_fixed_run",
            &(fixed_id, &instants_scope(week_starts)),
        ),
        Op::EnsureReminders {
            run_id, rebuild, ..
        } => digest("ensure_reminders", &(run_id, rebuild)),
        Op::AddReminder {
            run_id,
            kind,
            fire_at,
            sent_at,
        } => digest("add_reminder", &(run_id, kind, fire_at, sent_at)),
        Op::MarkReminderSent {
            reminder_id,
            message_id,
        } => digest("mark_reminder_sent", &(reminder_id, message_id)),
        Op::RescheduleUnpostedReminder {
            reminder_id,
            fire_at,
        } => digest("reschedule_unposted_reminder", &(reminder_id, fire_at)),
        Op::ReconcileDayOf { .. } => digest("reconcile_day_of", &()),
        Op::MarkDone => digest("mark_done", &()),
        Op::MaterialiseWeeks { .. } => digest("materialise_weeks", &()),
        Op::SetStatus { run_id, change, .. } => digest("set_status", &(run_id, change)),
        Op::AmendRun { run_id, to, .. } => digest("amend_run", &(run_id, to)),
        Op::SwapRunSlots {
            run_id,
            with_id,
            request_version,
            ..
        } => digest("swap_run_slots", &(run_id, with_id, request_version)),
        Op::SwapParticipants {
            run_id,
            remove,
            add,
            via_portal,
            ..
        } => digest("swap_participants", &(run_id, remove, add, via_portal)),
        Op::ApplyFixedEdit { request, .. } => digest("apply_fixed_edit", request),
        Op::ResetToFixed { run_id, .. } => digest("reset_to_fixed", run_id),
        Op::FixedParticipants {
            fixed_id,
            add,
            remove,
            ..
        } => digest("fixed_participants", &(fixed_id, add, remove)),
        Op::SetStandingAnswer {
            fixed_id,
            user_id,
            on,
            ..
        } => digest("set_standing_answer", &(fixed_id, user_id, on)),
        Op::SetAttendanceDefault { fixed_id, default } => {
            digest("set_attendance_default", &(fixed_id, default))
        }
        Op::RecordAttendance {
            run_id, attended, ..
        } => digest("record_attendance", &(run_id, attended)),
        Op::RecountAttendance => digest("recount_attendance", &()),
        Op::SetRunBosses { run_id, bosses, .. } => digest("set_run_bosses", &(run_id, bosses)),
        Op::RecountRun { run_id } => digest("recount_run", run_id),
        Op::ReviveRun { run_id } => digest("revive_run", run_id),
        Op::FinishRun { run_id } => digest("finish_run", run_id),
        Op::SettleRun { settle, .. } => digest("settle_run", settle),
    })
}

/// An edit's request digest: exactly the operation's without preconditions
/// (as before they existed, pinned by `digest_pins`), otherwise folding the
/// declared expectations in, so a retry with other expectations mismatches.
fn edit_digest(op: &Op<'_>, expect: &Expect) -> SchedulerResult<String> {
    let request = op_digest(op)?;
    Ok(if expect.is_empty() {
        request
    } else {
        digest("expect", &(request, &expect.canonical()))
    })
}

/// The request digest a reused request id must match: SHA-256 over the
/// operation name and the `Debug` form of its normalised arguments (policies
/// and directories, which are configuration, are left out).
pub(super) fn digest(kind: &str, arguments: &impl fmt::Debug) -> String {
    sha256_hex(format!("{kind}\n{arguments:?}").as_bytes())
}

/// A request id already recorded for the actor ends the operation before
/// anything is planned: the same request is [`SchedulerError::AlreadyApplied`],
/// another one [`SchedulerError::IdempotencyMismatch`]. The commit checks
/// again for races.
async fn check_request(store: &impl ScheduleStore, meta: &ChangeMeta) -> SchedulerResult<()> {
    let Some(request_id) = &meta.origin.request_id else {
        return Ok(());
    };
    match store
        .recorded_request(&meta.origin.actor, request_id)
        .await?
    {
        None => Ok(()),
        Some(recorded) if recorded.digest == meta.request_digest => {
            Err(already_applied(recorded.committed))
        }
        Some(recorded) => Err(SchedulerError::IdempotencyMismatch {
            seq: recorded.committed.seq,
        }),
    }
}

fn already_applied(committed: Committed) -> SchedulerError {
    SchedulerError::AlreadyApplied {
        seq: committed.seq,
        revision: committed.revision,
    }
}

fn store_failure(error: StoreError) -> SchedulerError {
    match error {
        StoreError::IdempotencyMismatch { seq } => SchedulerError::IdempotencyMismatch { seq },
        StoreError::StaleEdit(conflicts) => SchedulerError::StaleEdit { conflicts },
        StoreError::Precondition(error) => SchedulerError::Precondition(error),
        other => SchedulerError::Store(other),
    }
}

fn weeks_scope(week_starts: &[impl AwareDateTime]) -> Result<Scope, ScheduleError> {
    let weeks = week_starts
        .iter()
        .map(utc_instant)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Scope::Weeks(weeks))
}

/// Which recorded changes a rollback undoes.
#[derive(Debug)]
enum Selection<'a> {
    Seqs(&'a [u64]),
    Week {
        week: DateTime<Utc>,
        revision: u64,
    },
    Actor {
        actor: &'a Actor,
        since: DateTime<Utc>,
    },
}

/// What one rollback call asks for; the request digest covers all of it.
#[derive(Debug)]
struct Rollback<'a> {
    selection: Selection<'a>,
    scope: RevertScope,
    mode: RevertMode,
    /// A checkpoint restored to (name, head), named in the notices and
    /// referenced last by the rollback record.
    checkpoint: Option<(String, ChangeRef)>,
    /// Plan only: return the outcome without committing.
    preview: bool,
}

impl<S: ScheduleStore + ChangeHistory, I: IdSource, C: Clock> SchedulerService<S, I, C> {
    /// Revert the named records as one new change by `admin` through
    /// [`Surface::Rollback`] (history is never rewritten). `held` supplies
    /// the reminders unresolved delivery attempts hold, re-read on every
    /// attempt. [`RevertOutcome::Conflicts`] and [`RevertOutcome::Unchanged`]
    /// record and announce nothing.
    ///
    /// # Errors
    /// [`SchedulerError::History`] for an unknown or tampered record;
    /// [`SchedulerError::AlreadyApplied`] / [`SchedulerError::IdempotencyMismatch`]
    /// for a reused request id.
    #[allow(clippy::too_many_arguments)]
    pub async fn revert_changes(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        seqs: &[u64],
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let rollback = Rollback {
            selection: Selection::Seqs(seqs),
            scope: RevertScope::Whole,
            mode,
            checkpoint: None,
            preview: false,
        };
        self.rollback(admin, request_id, rollback, policy, held)
            .await
    }

    /// Restore `week` to how it was at store `revision`: revert every later
    /// change touching it, only within the week (weekly timings and other
    /// weeks are reported as skipped).
    ///
    /// # Errors
    /// As [`Self::revert_changes`].
    #[allow(clippy::too_many_arguments)]
    pub async fn restore_week_to(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        week: DateTime<Utc>,
        revision: u64,
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let rollback = Rollback {
            selection: Selection::Week { week, revision },
            scope: RevertScope::Week(week),
            mode,
            checkpoint: None,
            preview: false,
        };
        self.rollback(admin, request_id, rollback, policy, held)
            .await
    }

    /// Revert everything `actor` changed since `since` (spam cleanup).
    ///
    /// # Errors
    /// As [`Self::revert_changes`].
    #[allow(clippy::too_many_arguments)]
    pub async fn revert_by_actor(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        actor: &Actor,
        since: DateTime<Utc>,
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let rollback = Rollback {
            selection: Selection::Actor { actor, since },
            scope: RevertScope::Whole,
            mode,
            checkpoint: None,
            preview: false,
        };
        self.rollback(admin, request_id, rollback, policy, held)
            .await
    }

    /// What [`Self::revert_changes`] would do now; nothing is written.
    /// Reminder ids in `rows` are planned afresh by the apply.
    ///
    /// # Errors
    /// As [`Self::revert_changes`], without the request-id refusals.
    pub async fn preview_revert_changes(
        &mut self,
        seqs: &[u64],
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let rollback = Rollback {
            selection: Selection::Seqs(seqs),
            scope: RevertScope::Whole,
            mode,
            checkpoint: None,
            preview: true,
        };
        self.rollback("preview", None, rollback, policy, held).await
    }

    /// What [`Self::restore_week_to`] would do now; nothing is written.
    ///
    /// # Errors
    /// As [`Self::preview_revert_changes`].
    pub async fn preview_restore_week(
        &mut self,
        week: DateTime<Utc>,
        revision: u64,
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let rollback = Rollback {
            selection: Selection::Week { week, revision },
            scope: RevertScope::Week(week),
            mode,
            checkpoint: None,
            preview: true,
        };
        self.rollback("preview", None, rollback, policy, held).await
    }

    /// What [`Self::revert_by_actor`] would do now; nothing is written.
    ///
    /// # Errors
    /// As [`Self::preview_revert_changes`].
    pub async fn preview_revert_by_actor(
        &mut self,
        actor: &Actor,
        since: DateTime<Utc>,
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let rollback = Rollback {
            selection: Selection::Actor { actor, since },
            scope: RevertScope::Whole,
            mode,
            checkpoint: None,
            preview: true,
        };
        self.rollback("preview", None, rollback, policy, held).await
    }

    async fn history(&self, filter: ChangeFilter) -> SchedulerResult<Vec<ChangeRecord>> {
        let mut query = ChangeQuery::new(filter);
        let mut records = Vec::new();
        loop {
            let page = self.store.list_changes(&query).await?;
            records.extend(page.records);
            match page.next_cursor {
                Some(cursor) => query.cursor = Some(cursor),
                None => return Ok(records),
            }
        }
    }

    /// The selected records, newest first, each loaded with its stored bytes
    /// checked.
    async fn select(&self, selection: &Selection<'_>) -> SchedulerResult<Vec<ChangeRecord>> {
        let seqs: BTreeSet<u64> = match selection {
            Selection::Seqs(seqs) => seqs.iter().copied().collect(),
            Selection::Week { week, revision } => changes_for_week(
                &self.history(ChangeFilter::Week(*week)).await?,
                *week,
                *revision,
            )
            .iter()
            .map(|record| record.seq)
            .collect(),
            Selection::Actor { actor, since } => changes_by_actor(
                &self.history(ChangeFilter::Actor((*actor).clone())).await?,
                actor,
                *since,
            )
            .iter()
            .map(|record| record.seq)
            .collect(),
        };
        if seqs.is_empty() {
            return Err(SchedulerError::History(HistoryRefusal::NothingToRevert));
        }
        let mut records = Vec::new();
        for seq in seqs.into_iter().rev() {
            match self.store.load_checked(seq).await? {
                CheckedChange::Intact(record) if record.seq > 0 => records.push(*record),
                CheckedChange::Intact(_) | CheckedChange::Missing => {
                    return Err(SchedulerError::History(HistoryRefusal::UnknownChange(seq)));
                }
                CheckedChange::Tampered(_) => {
                    return Err(SchedulerError::History(HistoryRefusal::Tampered(seq)));
                }
            }
        }
        Ok(records)
    }

    async fn rollback(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        rollback: Rollback<'_>,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let request_digest = request_id.as_ref().map(|_| digest("rollback", &rollback));
        let origin = Origin {
            actor: Actor::admin(admin),
            surface: Surface::Rollback,
            request_id,
        };
        let now = self.clock.now();
        let mut meta = ChangeMeta {
            origin,
            at: now,
            notices: Vec::new(),
            refs: Vec::new(),
            request_digest,
            expect: Expect::default(),
            outbox: Vec::new(),
        };
        if !rollback.preview {
            check_request(&self.store, &meta).await?;
        }
        let mut attempt = 1;
        loop {
            // Records, hashes and held reminders are re-read on every attempt.
            let records = self.select(&rollback.selection).await?;
            let held = held.held_reminders().await?;
            let snapshot = self.store.load(&Scope::All).await?;
            let revision = snapshot.revision;
            let mut draft = Draft::new(snapshot);
            let base = draft.clone();
            let outcome = apply_revert(
                &mut draft,
                &mut self.ids,
                &records,
                rollback.scope,
                rollback.mode,
                policy,
                &held,
                now,
            )?;
            let mut outcome = outcome;
            if let (RevertOutcome::Reverted { notices, .. }, Some((name, _))) =
                (&mut outcome, &rollback.checkpoint)
            {
                for notice in notices {
                    if let NoticeChange::Rollback { checkpoint, .. } = &mut notice.change {
                        *checkpoint = Some(name.clone());
                    }
                }
            }
            let RevertOutcome::Reverted {
                seqs,
                skipped,
                notices,
                rows,
                seq,
                ..
            } = &mut outcome
            else {
                return Ok(outcome);
            };
            let after = draft.clone();
            let changes = draft.into_changes();
            if changes.is_empty() {
                return Ok(RevertOutcome::Unchanged {
                    seqs: seqs.clone(),
                    skipped: skipped.clone(),
                });
            }
            *rows = changed_rows(&base, &after, &changes);
            if rollback.preview {
                return Ok(outcome);
            }
            meta.notices = notices.iter().map(Notice::effect_kind).collect();
            meta.outbox = notices.clone();
            meta.refs = records.iter().map(ChangeRecord::reference).collect();
            meta.refs
                .extend(rollback.checkpoint.as_ref().map(|(_, head)| head.clone()));
            match self.store.commit(revision, changes, meta.clone()).await {
                Ok(Some(committed)) if committed.replayed => {
                    return Err(already_applied(committed));
                }
                Ok(committed) => {
                    *seq = committed.map(|committed| committed.seq);
                    return Ok(outcome);
                }
                Err(StoreError::Conflict { .. }) if attempt < COMMIT_ATTEMPTS => attempt += 1,
                Err(error) => return Err(store_failure(error)),
            }
        }
    }
}

impl<S: ScheduleStore + ChangeHistory + Checkpoints, I: IdSource, C: Clock>
    SchedulerService<S, I, C>
{
    /// Name a checkpoint at the current history head for the boss week
    /// containing `at` (normalised to that week's start under `policy`).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] (as [`SchedulerError::Store`]) for an
    /// invalid, reserved or taken name.
    pub async fn create_checkpoint(
        &self,
        admin: &str,
        name: &str,
        at: DateTime<Utc>,
        policy: &SchedulePolicy,
    ) -> SchedulerResult<Checkpoint> {
        let start = policy.week_of(&at).map_err(ScheduleError::from)?;
        let week = utc_instant(&start).map_err(ScheduleError::from)?;
        let new = NewCheckpoint {
            name: name.to_owned(),
            kind: CheckpointKind::Admin,
            week,
            created_at: self.clock.now(),
            created_by: Actor::admin(admin),
        };
        Ok(self
            .store
            .create_checkpoint(new)
            .await?
            .checkpoint()
            .clone())
    }

    /// The checkpoint, with its head (hash and revision) re-checked against
    /// the history.
    async fn checkpoint(&self, name: &str) -> SchedulerResult<Checkpoint> {
        let checkpoint = self.store.load_checkpoint(name).await?.ok_or_else(|| {
            SchedulerError::History(HistoryRefusal::UnknownCheckpoint(name.to_owned()))
        })?;
        match self.store.load_checked(checkpoint.head.seq).await? {
            CheckedChange::Intact(record)
                if record.hash == checkpoint.head.hash
                    && record.revision == checkpoint.revision =>
            {
                Ok(checkpoint)
            }
            _ => Err(SchedulerError::History(HistoryRefusal::Tampered(
                checkpoint.head.seq,
            ))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn restore_checkpoint(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        name: &str,
        mode: RevertMode,
        preview: bool,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        let checkpoint = self.checkpoint(name).await?;
        let rollback = Rollback {
            selection: Selection::Week {
                week: checkpoint.week,
                revision: checkpoint.revision,
            },
            scope: RevertScope::Week(checkpoint.week),
            mode,
            checkpoint: Some((checkpoint.name.clone(), checkpoint.head)),
            preview,
        };
        match self
            .rollback(admin, request_id, rollback, policy, held)
            .await
        {
            // Nothing after the checkpoint touched its week: nothing to do.
            Err(SchedulerError::History(HistoryRefusal::NothingToRevert)) => {
                Ok(RevertOutcome::Unchanged {
                    seqs: Vec::new(),
                    skipped: Vec::new(),
                })
            }
            other => other,
        }
    }

    /// What restoring the checkpoint's boss week to it would do (the
    /// outcome, conflicts or skipped rows) without changing anything.
    ///
    /// # Errors
    /// As [`Self::restore_to_checkpoint`].
    pub async fn preview_checkpoint_restore(
        &mut self,
        name: &str,
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        self.restore_checkpoint("preview", None, name, mode, true, policy, held)
            .await
    }

    /// Restore the checkpoint's boss week to it ([`Self::restore_week_to`]
    /// at the checkpoint); the rollback record also references the
    /// checkpoint's head.
    ///
    /// # Errors
    /// [`HistoryRefusal::UnknownCheckpoint`], a checkpoint whose head no
    /// longer matches the history ([`HistoryRefusal::Tampered`]), or as
    /// [`Self::revert_changes`].
    #[allow(clippy::too_many_arguments)]
    pub async fn restore_to_checkpoint(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        name: &str,
        mode: RevertMode,
        policy: &ReminderPolicy,
        held: &impl HeldReminders,
    ) -> SchedulerResult<RevertOutcome> {
        self.restore_checkpoint(admin, request_id, name, mode, false, policy, held)
            .await
    }
}

#[cfg(test)]
mod digest_pins {
    use std::collections::BTreeMap;

    use chrono::{NaiveTime, TimeZone, Weekday};

    use super::*;
    use crate::domain::schedule::{AmendedRunChoice, FixedEditChoices, RunSource};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 29, 20, 0, 0).unwrap()
    }

    fn new_fixed() -> NewFixedRun {
        NewFixedRun {
            owner_pinned: false,
            owner_id: "1".into(),
            channel_id: Some("900".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
            participants: vec!["1".into(), "2".into()],
            note: None,
        }
    }

    fn new_run() -> NewRun {
        NewRun {
            fixed_run_id: None,
            channel_id: Some("900".into()),
            week_start: at(),
            datetime: at(),
            bosses: vec!["HFA".into()],
            participants: vec!["1".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        }
    }

    /// The request digests every attributed operation produced before the
    /// `apply_op` refactor, from the same argument tuples.
    pub(super) fn legacy() -> Vec<(&'static str, String)> {
        let run_id = "r-1";
        let scope = Scope::Weeks(vec![at()]);
        let patch = FixedRunPatch {
            note: Some("n".into()),
            ..FixedRunPatch::default()
        };
        let changed = [FixedField::Note];
        let remove = ["2".to_owned()];
        let add = ["3".to_owned()];
        let change = StatusChange {
            status: RunStatus::Cancelled,
            announce: true,
            via_portal: false,
        };
        let request = FixedEditRequest {
            fixed_id: "f-1".into(),
            edit: FixedEdit {
                note: Some("x".into()),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::PerRun(BTreeMap::from([(
                "r-1".to_owned(),
                AmendedRunChoice::KeepForThisWeek,
            )])),
        };
        vec![
            ("add_fixed_run", digest("add_fixed_run", &new_fixed())),
            ("create_run", digest("create_run", &new_run())),
            ("materialise_week", digest("materialise_week", &scope)),
            (
                "set_run_status",
                digest("set_run_status", &(run_id, RunStatus::Done)),
            ),
            (
                "set_rsvp",
                digest(
                    "set_rsvp",
                    &(run_id, "u-1", RsvpState::Yes, RsvpSource::Chat),
                ),
            ),
            (
                "apply_reaction",
                digest("apply_reaction", &(run_id, "u-1", "\u{2705}", true)),
            ),
            (
                "edit_fixed_run",
                digest("edit_fixed_run", &("f-1", &patch, &changed[..], &scope)),
            ),
            (
                "retire_fixed_run",
                digest("retire_fixed_run", &("f-1", &scope)),
            ),
            (
                "ensure_reminders",
                digest("ensure_reminders", &(run_id, true)),
            ),
            (
                "add_reminder",
                digest("add_reminder", &(run_id, "day_of", at(), Some(at()))),
            ),
            (
                "mark_reminder_sent",
                digest("mark_reminder_sent", &("m-1", Some("5001"))),
            ),
            (
                "reschedule_unposted_reminder",
                digest("reschedule_unposted_reminder", &("m-1", at())),
            ),
            ("reconcile_day_of", digest("reconcile_day_of", &())),
            ("mark_done", digest("mark_done", &())),
            ("materialise_weeks", digest("materialise_weeks", &())),
            ("set_status", digest("set_status", &(run_id, change))),
            ("amend_run", digest("amend_run", &(run_id, at()))),
            (
                "swap_participants",
                digest("swap_participants", &(run_id, &remove[..], &add[..], true)),
            ),
            ("apply_fixed_edit", digest("apply_fixed_edit", &request)),
            ("reset_to_fixed", digest("reset_to_fixed", &run_id)),
        ]
    }

    /// Captured from the service before `apply_op` existed.
    const PINS: &[(&str, &str)] = &[
        (
            "add_fixed_run",
            "8ca8619ea5d46f33c3d1b8694428652634960d51dd22d374e29676b076438d2d",
        ),
        (
            "create_run",
            "6a8f67f84725fc7db1e2fb8065f9919cc116aba1985e54d5f33eaa8bd7cba9c3",
        ),
        (
            "materialise_week",
            "b546ddc3a72ec89abc39fad50b04b06cd871d9a463e9c2a3f401d6670cb2e376",
        ),
        (
            "set_run_status",
            "77c7ae892778832a65d2af58e3bd54ed070bfc47a2b4ed6b1430e486f1977c22",
        ),
        (
            "set_rsvp",
            "78443bd8736c04f9c640202938af1289a9fba71e01c69e46a5ce7fa3b5d2fb72",
        ),
        (
            "apply_reaction",
            "4a6d3cd828de4d8ec5c23107454701dd92fea028286704dbb4db88039efce8e9",
        ),
        (
            "edit_fixed_run",
            "98f39b3bdb07e84bf1aa2c7bfa5ffe1a6080b6f7a6832e1fe3712432fd326248",
        ),
        (
            "retire_fixed_run",
            "d0239afb992b5e6f8f3090dc185d7d9ae8d7b7b5f4c2719d5c3f2f1aba48edaa",
        ),
        (
            "ensure_reminders",
            "4d9f59ebb52f541905fd02d97ad91125eaf64be0cdd4fc735cf6f09c2c70abd3",
        ),
        (
            "add_reminder",
            "f2d7d118e839817c39a2fcead99d0f7c4e92671f22eb7e7ea419df9e1c10a959",
        ),
        (
            "mark_reminder_sent",
            "2b218147aee0341f2011748e48af3d2fd1c65ec2da09d63471e539e8aca0071a",
        ),
        (
            "reschedule_unposted_reminder",
            "4a877997f0947b74d778cfe097543dd216bee38bb9568da9940caea95fe93a03",
        ),
        (
            "reconcile_day_of",
            "6938945a3bedf7c921225746714f3bdc5ea7c3268cb5dd5852758a3880830f7e",
        ),
        (
            "mark_done",
            "79811cefc02062409da71783aa12d6630273c2e1146dbfea25fe47ceba08f244",
        ),
        (
            "materialise_weeks",
            "419ea541f49179228989b7a26e94d6cebcfbf9ef44e9682270668edb9b31350d",
        ),
        (
            "set_status",
            "b48637a7438f7067c265a5469a17f48cd9967548be036a3aea0cbdf8c8267109",
        ),
        (
            "amend_run",
            "afa98afe79838ecc990f83844d85f87c2d60e5af222b00631f3b9565d70112f3",
        ),
        (
            "swap_participants",
            "8a6633b7a1456b9bbcb674de704bdaa22e69d4315696a2a4a043a8a9b1bbcb78",
        ),
        (
            "apply_fixed_edit",
            "c28d5ae697da62807ca196a01817ade0f3943d3c2ea6df13302b85a58aab8663",
        ),
        (
            "reset_to_fixed",
            "b731b42f4860fa1793ecf50104f1d0ff69d3f168f83dcc0c2c7a016bd18496ec",
        ),
    ];

    #[test]
    fn legacy_argument_tuples_match_the_pins() {
        let legacy = legacy();
        assert_eq!(legacy.len(), PINS.len());
        for ((kind, digest), (pinned_kind, pinned)) in legacy.iter().zip(PINS) {
            assert_eq!((*kind, digest.as_str()), (*pinned_kind, *pinned));
        }
    }

    /// An edit with expectations, pinned; declaration order is irrelevant.
    #[test]
    fn expectation_digests_are_pinned_and_order_free() {
        use crate::domain::history::{BlameTarget, Precondition};

        let op = Op::SetRunStatus {
            run_id: "r-1".into(),
            status: RunStatus::Done,
        };
        let field =
            |name: &str, seen| Precondition::new(BlameTarget::Run("r-1".into()), name, seen);
        let reference = ChangeRef {
            seq: 7,
            hash: "a".repeat(64),
        };
        let one = Expect::fields([field("status", Some(7)), field("slot", None)])
            .overriding([reference.clone()]);
        let other =
            Expect::fields([field("slot", None), field("status", Some(7))]).overriding([reference]);
        let pinned = edit_digest(&op, &one).unwrap();
        assert_eq!(pinned, edit_digest(&op, &other).unwrap());
        assert_eq!(
            pinned,
            "16093dfe29ef66fe15a920441dafe8b9510e76c715908a2d46582d5f05780e4c"
        );
    }

    #[test]
    fn op_digests_are_byte_identical_to_the_pre_refactor_ones() {
        use crate::domain::members::Roster;

        let reminders = ReminderPolicy {
            zone: chrono_tz::UTC,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60],
        };
        let schedule = SchedulePolicy::new(reminders.clone(), Weekday::Wed, NaiveTime::MIN);
        let roster = Roster::new();
        let week = WeekStart::new(&at(), chrono_tz::UTC).unwrap();
        let run_id = || "r-1".to_owned();
        let ops: Vec<(&str, Op<'_>)> = vec![
            ("add_fixed_run", Op::AddFixedRun(new_fixed())),
            ("create_run", Op::CreateRun(new_run())),
            (
                "materialise_week",
                Op::MaterialiseWeek {
                    week_start: week,
                    policy: &reminders,
                },
            ),
            (
                "set_run_status",
                Op::SetRunStatus {
                    run_id: run_id(),
                    status: RunStatus::Done,
                },
            ),
            (
                "set_rsvp",
                Op::SetRsvp {
                    run_id: run_id(),
                    user_id: "u-1".into(),
                    state: RsvpState::Yes,
                    source: RsvpSource::Chat,
                },
            ),
            (
                "apply_reaction",
                Op::ApplyReaction {
                    run_id: run_id(),
                    user_id: "u-1".into(),
                    emoji: "\u{2705}".into(),
                    added: true,
                },
            ),
            (
                "edit_fixed_run",
                Op::EditFixedRun {
                    fixed_id: "f-1".into(),
                    patch: FixedRunPatch {
                        note: Some("n".into()),
                        ..FixedRunPatch::default()
                    },
                    changed: vec![FixedField::Note],
                    week_starts: vec![week],
                    policy: &reminders,
                },
            ),
            (
                "retire_fixed_run",
                Op::RetireFixedRun {
                    fixed_id: "f-1".into(),
                    week_starts: vec![week],
                    policy: &reminders,
                },
            ),
            (
                "ensure_reminders",
                Op::EnsureReminders {
                    run_id: run_id(),
                    rebuild: true,
                    policy: &reminders,
                },
            ),
            (
                "add_reminder",
                Op::AddReminder {
                    run_id: run_id(),
                    kind: "day_of".into(),
                    fire_at: at(),
                    sent_at: Some(at()),
                },
            ),
            (
                "mark_reminder_sent",
                Op::MarkReminderSent {
                    reminder_id: "m-1".into(),
                    message_id: Some("5001".into()),
                },
            ),
            (
                "reschedule_unposted_reminder",
                Op::RescheduleUnpostedReminder {
                    reminder_id: "m-1".into(),
                    fire_at: at(),
                },
            ),
            (
                "reconcile_day_of",
                Op::ReconcileDayOf { policy: &reminders },
            ),
            ("mark_done", Op::MarkDone),
            (
                "materialise_weeks",
                Op::MaterialiseWeeks { policy: &schedule },
            ),
            (
                "set_status",
                Op::SetStatus {
                    run_id: run_id(),
                    change: StatusChange {
                        status: RunStatus::Cancelled,
                        announce: true,
                        via_portal: false,
                    },
                    policy: &reminders,
                },
            ),
            (
                "amend_run",
                Op::AmendRun {
                    run_id: run_id(),
                    to: at(),
                    policy: &schedule,
                },
            ),
            (
                "swap_participants",
                Op::SwapParticipants {
                    run_id: run_id(),
                    remove: vec!["2".into()],
                    add: vec!["3".into()],
                    via_portal: true,
                    directory: &roster,
                },
            ),
            (
                "apply_fixed_edit",
                Op::ApplyFixedEdit {
                    request: FixedEditRequest {
                        fixed_id: "f-1".into(),
                        edit: FixedEdit {
                            note: Some("x".into()),
                            ..FixedEdit::default()
                        },
                        choices: FixedEditChoices::PerRun(BTreeMap::from([(
                            "r-1".to_owned(),
                            AmendedRunChoice::KeepForThisWeek,
                        )])),
                    },
                    directory: &roster,
                    policy: &schedule,
                },
            ),
            (
                "reset_to_fixed",
                Op::ResetToFixed {
                    run_id: run_id(),
                    policy: &schedule,
                },
            ),
        ];
        assert_eq!(ops.len(), PINS.len());
        let expect = Expect::fields([crate::domain::history::Precondition::new(
            crate::domain::history::BlameTarget::Run("r-1".into()),
            "slot",
            Some(1),
        )]);
        for ((kind, op), (pinned_kind, pinned)) in ops.iter().zip(PINS) {
            assert_eq!(kind, pinned_kind);
            assert_eq!(op_digest(op).unwrap(), *pinned, "{kind}");
            // No expectations: byte-identical; any: a different request.
            assert_eq!(
                edit_digest(op, &Expect::default()).unwrap(),
                *pinned,
                "{kind}"
            );
            assert_ne!(edit_digest(op, &expect).unwrap(), *pinned, "{kind}");
        }
    }

    /// The one-commit create keeps the plain create's digest, so keys
    /// recorded before it existed still replay.
    #[test]
    fn the_materialising_create_digests_as_the_plain_create() {
        let reminders = ReminderPolicy {
            zone: chrono_tz::UTC,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60],
        };
        let schedule = SchedulePolicy::new(reminders, Weekday::Wed, NaiveTime::MIN);
        let combined = Op::AddFixedRunMaterialised {
            new: new_fixed(),
            policy: &schedule,
        };
        let (kind, pinned) = PINS[0];
        assert_eq!(kind, "add_fixed_run");
        assert_eq!(op_digest(&combined).unwrap(), pinned);
        assert_eq!(
            op_digest(&combined).unwrap(),
            op_digest(&Op::AddFixedRun(new_fixed())).unwrap()
        );
    }
}
