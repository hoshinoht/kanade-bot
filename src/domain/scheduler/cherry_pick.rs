//! Cherry-pick on the scheduler service: plan with
//! [`plan_pick`](crate::domain::history::plan_pick), replay the steps
//! through `apply_op` (the normal rules), and commit one `CherryPick`
//! record referencing the picked change (and, when forced over conflicts,
//! the changes it overrides).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, Utc};

use super::ports::{Clock, IdSource, ScheduleStore, Scope, StoreError};
use super::service::{COMMIT_ATTEMPTS, SchedulerService, digest};
use crate::domain::history::{
    Actor, BlameIndex, BlameTarget, ChangeHistory, ChangeMeta, ChangeRecord, ChangeRef,
    CheckedChange, EDIT_OVERRIDE, Expect, HistoryRefusal, Origin, PickConflict, PickMode, PickPlan,
    PickRefusal, PickStep, Precondition, Surface, plan_pick,
};
use crate::domain::members::Directory;
use crate::domain::schedule::{
    Draft, Notice, Op, ScheduleError, SchedulePolicy, ScheduleSnapshot, StatusChange, apply_op,
    derive_run_status, utc_instant,
};

/// Why a pick did not apply; nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickError {
    Refused(PickRefusal),
    /// Strict mode: the target no longer holds the picked `before` values.
    Conflicts(Vec<PickConflict>),
    /// Force mode: the conflicts now differ from the ones reviewed (the
    /// preview's `force` expectations); preview again.
    PickStale {
        reviewed: Expect,
        current: Expect,
    },
    /// The picked record is unknown, the genesis record or tampered.
    History(HistoryRefusal),
    /// The target already holds every picked value.
    NoEffect,
    /// A scheduling rule refused a step.
    Schedule(ScheduleError),
    Store(StoreError),
    AlreadyApplied {
        seq: u64,
        revision: u64,
    },
    IdempotencyMismatch {
        seq: u64,
    },
}

impl From<StoreError> for PickError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::IdempotencyMismatch { seq } => Self::IdempotencyMismatch { seq },
            error => Self::Store(error),
        }
    }
}

impl From<ScheduleError> for PickError {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

impl From<PickRefusal> for PickError {
    fn from(refusal: PickRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl fmt::Display for PickError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::Conflicts(conflicts) => write!(f, "the pick has {} conflicts", conflicts.len()),
            Self::PickStale { .. } => {
                f.write_str("the conflicts changed since they were reviewed; preview again")
            }
            Self::History(refusal) => refusal.fmt(f),
            Self::NoEffect => f.write_str("the pick changes nothing"),
            Self::Schedule(error) => error.fmt(f),
            Self::Store(error) => error.fmt(f),
            Self::AlreadyApplied { seq, revision } => write!(
                f,
                "pick already applied as change {seq} (revision {revision})"
            ),
            Self::IdempotencyMismatch { seq } => write!(
                f,
                "request id already used by change {seq} for a different request"
            ),
        }
    }
}

impl std::error::Error for PickError {}

pub type PickResult<T> = Result<T, PickError>;

/// A committed pick. Notices are returned, never posted here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub seq: u64,
    pub revision: u64,
    pub plan: PickPlan,
    pub notices: Vec<Notice>,
    /// Changes a forced pick overrode (also in the record's `refs`).
    pub overridden: Vec<ChangeRef>,
}

/// What a pick would do now; nothing is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickPreview {
    pub plan: PickPlan,
    /// The notices the pick would return.
    pub notices: Vec<Notice>,
    /// The target already holds every picked value.
    pub no_effect: bool,
    /// Exactly what a forced pick must send back (`PickMode::Force`): each
    /// conflicting field with the change that last set it, and those
    /// changes as overrides. Empty when nothing conflicts.
    pub force: Expect,
    /// `plan.conflicts` apply to BOTH modes (planning is mode-independent):
    /// strict refuses when this is true; force proceeds only with `force`.
    pub strict_refuses: bool,
}

fn op<'a>(
    step: &PickStep,
    policy: &'a SchedulePolicy,
    directory: &'a (dyn Directory + Sync),
) -> Op<'a> {
    match step {
        PickStep::Amend { run_id, to } => Op::AmendRun {
            run_id: run_id.clone(),
            to: *to,
            policy,
        },
        PickStep::Swap {
            run_id,
            remove,
            add,
        } => Op::SwapParticipants {
            run_id: run_id.clone(),
            remove: remove.clone(),
            add: add.clone(),
            via_portal: true,
            directory,
        },
        PickStep::Status { run_id, status } => Op::SetStatus {
            run_id: run_id.clone(),
            change: StatusChange {
                status: *status,
                announce: true,
                via_portal: true,
            },
            policy: &policy.reminders,
        },
        PickStep::Rsvp {
            run_id,
            user_id,
            state,
            source,
        } => Op::SetRsvp {
            run_id: run_id.clone(),
            user_id: user_id.clone(),
            state: *state,
            source: *source,
        },
    }
}

/// Replay the steps on `snapshot` through the normal mutation path; after
/// an answer the run's status is recounted as a reaction does.
fn replay_steps(
    snapshot: ScheduleSnapshot,
    plan: &PickPlan,
    ids: &mut impl IdSource,
    policy: &SchedulePolicy,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> PickResult<(Draft, Vec<Notice>)> {
    let mut draft = Draft::new(snapshot).with_attendance(policy.attendance);
    let mut notices = Vec::new();
    for step in &plan.steps {
        let applied = apply_op(&mut draft, ids, &op(step, policy, directory), now)?;
        notices.extend(applied.notices);
        if let PickStep::Rsvp { run_id, .. } = step {
            let run = draft.require_run(run_id)?;
            let status = derive_run_status(&draft, &run, run.status, now).status;
            if status != run.status {
                draft.set_run_status(run_id, status);
            }
        }
    }
    Ok((draft, notices))
}

impl<S: ScheduleStore + ChangeHistory + BlameIndex, I: IdSource, C: Clock>
    SchedulerService<S, I, C>
{
    async fn picked_record(&self, seq: u64) -> PickResult<ChangeRecord> {
        match self.store.load_checked(seq).await? {
            CheckedChange::Intact(record) if record.seq > 0 => Ok(*record),
            CheckedChange::Intact(_) | CheckedChange::Missing => {
                Err(PickError::History(HistoryRefusal::UnknownChange(seq)))
            }
            CheckedChange::Tampered(_) => Err(PickError::History(HistoryRefusal::Tampered(seq))),
        }
    }

    /// What picking change `seq` into the boss week of `target_week` would
    /// do now: the planned steps, the conflicts, the notices, and the
    /// expectations a forced pick must echo. The conflict values and their
    /// versions describe one store revision (re-read on a race). Nothing is
    /// written.
    pub async fn preview_cherry_pick(
        &self,
        seq: u64,
        target_week: DateTime<Utc>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> PickResult<PickPreview> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let record = self.picked_record(seq).await?;
        let mut attempt = 1;
        loop {
            let snapshot = self.store.load(&Scope::All).await?;
            let revision = snapshot.revision;
            let plan = plan_pick(&record, &snapshot, target_week, policy, now)?;
            let force = self.review(&plan.conflicts).await?;
            // The versions were read after the values: re-check nothing was
            // committed in between, so they describe the same state.
            let found = self.store.load(&Scope::Weeks(Vec::new())).await?.revision;
            if found != revision {
                if attempt < COMMIT_ATTEMPTS {
                    attempt += 1;
                    continue;
                }
                return Err(PickError::Store(StoreError::Conflict {
                    expected: revision,
                    found,
                }));
            }
            let mut ids = crate::domain::drafts::PreviewIds::default();
            let (draft, notices) = replay_steps(snapshot, &plan, &mut ids, policy, directory, now)?;
            return Ok(PickPreview {
                no_effect: draft.into_changes().is_empty(),
                strict_refuses: !plan.conflicts.is_empty(),
                plan,
                notices,
                force,
            });
        }
    }

    /// Re-apply change `seq` to the same weekly timing's run in the boss
    /// week of `target_week`, as `admin`, in one `CherryPick` change whose
    /// `refs` name the picked record. Strict mode refuses conflicts. Force
    /// applies over exactly the conflicts reviewed in the preview: the
    /// store checks those fields' versions inside the commit, the changes
    /// they last set are also referenced, and the record is marked as an
    /// override. Idempotent per request id; re-planned on a revision race.
    #[allow(clippy::too_many_arguments)]
    pub async fn cherry_pick(
        &mut self,
        admin: &str,
        request_id: Option<String>,
        seq: u64,
        target_week: DateTime<Utc>,
        mode: PickMode,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> PickResult<Picked> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let actor = Actor::admin(admin);
        let week = policy
            .week_of(&target_week)
            .and_then(|start| utc_instant(&start))
            .map_err(ScheduleError::from)?;
        let reviewed = match &mode {
            PickMode::Strict => None,
            PickMode::Force(expect) => Some(expect.canonical()),
        };
        let request_digest = request_id
            .as_ref()
            .map(|_| digest("cherry_pick", &(seq, week, &reviewed)));
        if let Some(request_id) = &request_id
            && let Some(recorded) = self.store.recorded_request(&actor, request_id).await?
        {
            return Err(if recorded.digest == request_digest {
                PickError::AlreadyApplied {
                    seq: recorded.committed.seq,
                    revision: recorded.committed.revision,
                }
            } else {
                PickError::IdempotencyMismatch {
                    seq: recorded.committed.seq,
                }
            });
        }
        let record = self.picked_record(seq).await?;
        let mut attempt = 1;
        loop {
            let snapshot = self.store.load(&Scope::All).await?;
            let revision = snapshot.revision;
            let plan = plan_pick(&record, &snapshot, week, policy, now)?;
            let expect = match &reviewed {
                None if !plan.conflicts.is_empty() => {
                    return Err(PickError::Conflicts(plan.conflicts));
                }
                None => Expect::default(),
                Some(reviewed) => {
                    let current = self.review(&plan.conflicts).await?.canonical();
                    if current != *reviewed {
                        return Err(PickError::PickStale {
                            reviewed: reviewed.clone(),
                            current,
                        });
                    }
                    current
                }
            };
            let (draft, notices) =
                replay_steps(snapshot, &plan, &mut self.ids, policy, directory, now)?;
            let changes = draft.into_changes();
            if changes.is_empty() {
                return Err(PickError::NoEffect);
            }
            let mut kinds: Vec<String> = notices.iter().map(Notice::effect_kind).collect();
            // Any forced conflict marks an override, even over fields with
            // no recorded change (no ref to name).
            if !plan.conflicts.is_empty() {
                kinds.push(EDIT_OVERRIDE.to_owned());
            }
            let mut refs = vec![record.reference()];
            refs.extend(expect.overrides.iter().cloned());
            let overridden = expect.overrides.clone();
            let meta = ChangeMeta {
                origin: Origin {
                    actor: actor.clone(),
                    surface: Surface::CherryPick,
                    request_id: request_id.clone(),
                },
                at: now,
                notices: kinds,
                refs,
                request_digest: request_digest.clone(),
                expect,
            };
            match self.store.commit(revision, changes, meta).await {
                Ok(Some(committed)) if committed.replayed => {
                    return Err(PickError::AlreadyApplied {
                        seq: committed.seq,
                        revision: committed.revision,
                    });
                }
                Ok(Some(committed)) => {
                    return Ok(Picked {
                        seq: committed.seq,
                        revision: committed.revision,
                        plan,
                        notices,
                        overridden,
                    });
                }
                Ok(None) => return Err(PickError::NoEffect),
                Err(StoreError::Conflict { .. }) if attempt < COMMIT_ATTEMPTS => attempt += 1,
                // A reviewed field changed between planning and the commit.
                Err(StoreError::StaleEdit(_)) => {
                    let current = self
                        .store
                        .load(&Scope::All)
                        .await
                        .ok()
                        .and_then(|snapshot| plan_pick(&record, &snapshot, week, policy, now).ok());
                    let current = match current {
                        Some(plan) => self.review(&plan.conflicts).await?.canonical(),
                        None => Expect::default(),
                    };
                    return Err(PickError::PickStale {
                        reviewed: reviewed.unwrap_or_default(),
                        current,
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// The expectations a forced pick over `conflicts` carries: each
    /// conflicting field with the change that last set it (blame index;
    /// `None` without history), and those changes as overrides.
    async fn review(&self, conflicts: &[PickConflict]) -> PickResult<Expect> {
        let mut index: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        let mut fields = Vec::new();
        let mut seqs = BTreeSet::new();
        for conflict in conflicts {
            if !index.contains_key(&conflict.run_id) {
                let last = self
                    .store
                    .last_changes(&BlameTarget::Run(conflict.run_id.clone()))
                    .await?;
                index.insert(conflict.run_id.clone(), last);
            }
            let seen = index[&conflict.run_id].get(&conflict.field).copied();
            seqs.extend(seen);
            fields.push(Precondition::new(
                BlameTarget::Run(conflict.run_id.clone()),
                conflict.field.clone(),
                seen,
            ));
        }
        let mut overrides = Vec::new();
        for seq in seqs {
            match self.store.load_checked(seq).await? {
                CheckedChange::Intact(record) => overrides.push(record.reference()),
                CheckedChange::Missing => {
                    return Err(PickError::History(HistoryRefusal::UnknownChange(seq)));
                }
                CheckedChange::Tampered(_) => {
                    return Err(PickError::History(HistoryRefusal::Tampered(seq)));
                }
            }
        }
        Ok(Expect::fields(fields).overriding(overrides).canonical())
    }
}
