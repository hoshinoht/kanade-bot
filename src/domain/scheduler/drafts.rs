//! Administrator drafts: stage schedule operations, preview their merge,
//! and merge them as one attributed change.
//!
//! The service plans against the store through [`DraftStore`]; every draft
//! write bumps its version, and the merge commits the schedule rows, the
//! history record (`Surface::DraftMerge`), the draft's `merged` status and
//! its event in one transaction. Previews and staging checks write nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, Utc};

use super::ports::{Clock, IdSource, ScheduleStore, StoreError};
use super::service::{COMMIT_ATTEMPTS, SchedulerService, digest};
use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEventKind, DraftKind, DraftOp, DraftRequest, DraftStale,
    DraftStatus, DraftStore, DraftUpdate, DraftWrite, LoadedDraft, MergeAnalysis, MergeCommit,
    MergeConflict, NewDraft, PreviewIds, ReplayError, StagedOp, StoredDraft, Target, analyze_merge,
    analyze_merge_applying, expires_week, renumber_created, replay, replay_equivalent, replay_real,
};
use crate::domain::history::{Actor, ChangeMeta, ChangeRef, HistoryGap, Origin, Surface, rewind};
use crate::domain::members::Directory;
use crate::domain::schedule::{
    Draft, FixedRun, Notice, NoticeChange, Run, ScheduleError, SchedulePolicy, ScheduleSnapshot,
    materialise_weeks,
};

/// A draft refused an edit: the operation is unknown, or removing it would
/// leave a later operation pointing at an operation that creates nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditRefusal {
    UnknownOp { ord: usize },
    DanglingTarget { removed: usize, used_by: usize },
}

impl fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownOp { ord } => write!(f, "the draft has no operation {ord}"),
            Self::DanglingTarget { removed, used_by } => write!(
                f,
                "operation {used_by} needs the row operation {removed} creates"
            ),
        }
    }
}

/// Why a draft operation did not apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftError {
    Schedule(ScheduleError),
    Store(StoreError),
    /// No draft has this id.
    UnknownDraft(String),
    /// The draft is closed or at another version; nothing was written.
    Stale {
        status: DraftStatus,
        version: u64,
        merged_seq: Option<u64>,
    },
    /// Another merge already closed the draft, as change `seq`.
    AlreadyMerged {
        seq: u64,
    },
    /// The merge request id was already recorded, as change `seq`.
    AlreadyApplied {
        seq: u64,
        revision: u64,
    },
    /// The merge request id was already used for a different merge.
    IdempotencyMismatch {
        seq: u64,
    },
    /// The records after the draft's base do not chain from it.
    HistoryGap(HistoryGap),
    /// The merge (or rebase) cannot apply as it stands; nothing was written.
    Conflicts(Vec<MergeConflict>),
    /// A staged operation no longer replays.
    ReplayFailed {
        ord: usize,
        error: ReplayError,
    },
    /// An edit would break created-target references.
    EditRefused(EditRefusal),
    /// A merge with no staged operations.
    Empty,
    /// The merge replay changed no rows.
    NoEffect,
    /// A title outside 1..=200 characters.
    InvalidTitle,
    /// The create request id was already used for another draft.
    RequestMismatch {
        draft_id: String,
    },
    /// Member requests merge through the request path (S3), not here.
    RequestDraft,
    /// The draft's boss week has passed; it is closed as expired.
    Expired,
    /// A request's requester may no longer have it merged.
    RequesterUnauthorised,
    /// The operation (its codec kind) exists only for proposals.
    ProposalOnlyOp(&'static str),
}

impl From<ScheduleError> for DraftError {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

impl From<StoreError> for DraftError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::HistoryGap(gap) => Self::HistoryGap(gap),
            error => Self::Store(error),
        }
    }
}

impl From<HistoryGap> for DraftError {
    fn from(gap: HistoryGap) -> Self {
        Self::HistoryGap(gap)
    }
}

impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schedule(error) => error.fmt(f),
            Self::Store(error) => error.fmt(f),
            Self::UnknownDraft(id) => write!(f, "no draft `{id}`"),
            Self::Stale {
                status, version, ..
            } => {
                write!(f, "the draft is {status} at version {version}")
            }
            Self::AlreadyMerged { seq } => write!(f, "the draft already merged as change {seq}"),
            Self::AlreadyApplied { seq, revision } => write!(
                f,
                "merge already applied as change {seq} (revision {revision})"
            ),
            Self::IdempotencyMismatch { seq } => write!(
                f,
                "merge request id already used by change {seq} for a different merge"
            ),
            Self::HistoryGap(gap) => gap.fmt(f),
            Self::Conflicts(conflicts) => {
                write!(f, "the draft has {} conflicts", conflicts.len())
            }
            Self::ReplayFailed { ord, error } => write!(f, "operation {ord} cannot apply: {error}"),
            Self::EditRefused(refusal) => refusal.fmt(f),
            Self::Empty => f.write_str("the draft stages no operations"),
            Self::NoEffect => f.write_str("the merge changed no rows"),
            Self::InvalidTitle => f.write_str("a draft title is 1 to 200 characters"),
            Self::RequestMismatch { draft_id } => {
                write!(f, "create request id already used for draft {draft_id}")
            }
            Self::RequestDraft => f.write_str("member requests merge through approval, not here"),
            Self::Expired => f.write_str("the draft's boss week has passed"),
            Self::RequesterUnauthorised => f.write_str("the requester may not make this request"),
            Self::ProposalOnlyOp(kind) => {
                write!(f, "`{kind}` is staged only by extractor and chat proposals")
            }
        }
    }
}

impl std::error::Error for DraftError {}

pub type DraftResult<T> = Result<T, DraftError>;

/// What one expiry pass closed: every expired draft and request id, and
/// the requester notices of the expired requests (returned, not posted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftExpiry {
    pub ids: Vec<String>,
    pub notices: Vec<Notice>,
}

/// A merged draft: its record, and the summary notices to post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeOutcome {
    pub seq: u64,
    pub revision: u64,
    /// One summary notice per affected channel, in channel order.
    pub notices: Vec<Notice>,
    /// Boss weeks whose runs the merge changed.
    pub weeks: Vec<DateTime<Utc>>,
    /// Things administrators should know that did not stop the merge.
    pub warnings: Vec<MergeWarning>,
    /// Per operation: the row it created, as committed.
    pub created: Vec<Option<String>>,
    /// Per operation: what it returned, as committed.
    pub results: Vec<crate::domain::schedule::OpResult>,
}

/// The `merged` event's note: the caller's note, then the runs a party
/// delta skipped (`skipped=<run>,…`), so a retry after a lost response can
/// still show them.
fn merged_note(note: Option<&str>, warnings: &[MergeWarning]) -> Option<String> {
    let skipped: Vec<&str> = warnings
        .iter()
        .map(|warning| match warning {
            MergeWarning::PartyDeltaSkipped { run_id, .. } => run_id.as_str(),
        })
        .collect();
    let skipped = (!skipped.is_empty()).then(|| format!("skipped={}", skipped.join(",")));
    match (note, skipped) {
        (Some(note), Some(skipped)) => Some(format!("{note} {skipped}")),
        (Some(note), None) => Some(note.to_owned()),
        (None, skipped) => skipped,
    }
}

/// The runs a replay's party deltas left alone rather than empty.
pub(super) fn skipped_runs(results: &[crate::domain::schedule::OpResult]) -> Vec<MergeWarning> {
    results
        .iter()
        .flat_map(|result| match result {
            crate::domain::schedule::OpResult::PartyDelta(delta) => delta.emptied.clone(),
            _ => Vec::new(),
        })
        .map(|run_id| MergeWarning::PartyDeltaSkipped {
            run_id,
            reason: SkipReason::Emptied,
        })
        .collect()
}

/// A merge applied, but left something as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeWarning {
    /// A weekly party delta left this run unchanged.
    PartyDeltaSkipped { run_id: String, reason: SkipReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The delta would have left the run with nobody.
    Emptied,
}

/// What [`SchedulerService::merge_loaded`] merges, and as whom.
pub(super) struct MergeInput<'a> {
    pub actor: Actor,
    pub surface: Surface,
    pub request_id: String,
    pub request_digest: String,
    pub draft: &'a StoredDraft,
    pub ops: Vec<DraftOp>,
    pub expected_version: u64,
    /// The public summary notices' description.
    pub summary: String,
    /// Detail after the record seq in the `merged` event.
    pub note: Option<String>,
    /// Re-checked on the current schedule at every attempt (a request's
    /// requester); `false` refuses with `RequesterUnauthorised`.
    pub authorise: Option<&'a (dyn Fn(&ScheduleSnapshot) -> bool + Sync)>,
    /// Runs whose status the operations set as of the merge, never
    /// conflicting with upstream (proposals only; empty for drafts and
    /// requests).
    pub status_at_apply: BTreeSet<String>,
    /// Written to the outbox with the merge, after its summary notices (a
    /// request's requester notice); never part of the record's kinds.
    pub also_notify: Vec<Notice>,
    /// Written to the outbox with the draft's close when the merge finds it
    /// expired (a request's requester notice).
    pub expired_notice: Option<Notice>,
    /// Replaces the per-channel summary notices (proposals: v4 announced
    /// only an approved move); `None` keeps the summaries.
    pub notices: Option<Vec<Notice>>,
}

pub(super) fn stale_of(id: &str, stale: DraftStale) -> DraftError {
    match stale {
        DraftStale::Missing => DraftError::UnknownDraft(id.to_owned()),
        DraftStale::Moved {
            status,
            version,
            merged_seq,
        } => {
            if status == DraftStatus::Merged
                && let Some(seq) = merged_seq
            {
                DraftError::AlreadyMerged { seq }
            } else if status == DraftStatus::Expired {
                DraftError::Expired
            } else {
                DraftError::Stale {
                    status,
                    version,
                    merged_seq,
                }
            }
        }
    }
}

async fn load_live<S: ScheduleStore + DraftStore>(
    store: &S,
    id: &str,
    expected_version: u64,
) -> DraftResult<LoadedDraft> {
    let loaded = load_admin(store, id).await?;
    if !loaded.draft.status.is_live() || loaded.draft.version != expected_version {
        return Err(stale_of(
            id,
            DraftStale::Moved {
                status: loaded.draft.status,
                version: loaded.draft.version,
                merged_seq: loaded.draft.merged_seq,
            },
        ));
    }
    Ok(loaded)
}

/// Load a draft for an administrator method: member requests merge through
/// the request path (S3), never here.
async fn load_admin<S: ScheduleStore + DraftStore>(
    store: &S,
    id: &str,
) -> DraftResult<LoadedDraft> {
    let loaded = store
        .load_draft(id)
        .await?
        .ok_or_else(|| DraftError::UnknownDraft(id.to_owned()))?;
    if loaded.draft.kind != DraftKind::Admin {
        return Err(DraftError::RequestDraft);
    }
    Ok(loaded)
}

/// Whether an expiry week starts before the current boss week.
pub(super) fn is_expired(
    expires_week: Option<DateTime<Utc>>,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<bool, ScheduleError> {
    match expires_week {
        None => Ok(false),
        Some(expires) => {
            let start = policy.week_of(&now)?;
            let week = crate::domain::schedule::utc_instant(&start)?;
            Ok(expires < week)
        }
    }
}

/// The current schedule with the history head read after it (one store
/// transaction), every record after `base`, and the rewound base snapshot.
pub(super) struct Upstream {
    pub current: ScheduleSnapshot,
    pub head: ChangeRef,
    pub base_snapshot: ScheduleSnapshot,
}

pub(super) async fn upstream<S: ScheduleStore + DraftStore>(
    store: &S,
    base: &ChangeRef,
) -> DraftResult<Upstream> {
    let (current, head) = store.snapshot_with_head().await?;
    let records = store.records_after(base).await?;
    let base_snapshot = rewind(&current, base, &head, &records)?;
    Ok(Upstream {
        current,
        head,
        base_snapshot,
    })
}

/// The system actor that closes expired drafts (tick and merge alike).
pub(super) const EXPIRY_ACTOR: &str = "delivery";

/// Whether the draft's expiry week, re-derived on `current`, has passed. A
/// replay failure is left to the merge analysis to report.
fn expired_on_current(
    current: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> Result<bool, ScheduleError> {
    match replay(current, ops, policy, directory, now) {
        Ok(replayed) => is_expired(
            expires_week(ops, &replayed.created, current, &replayed.draft, policy),
            policy,
            now,
        ),
        Err(_) => Ok(false),
    }
}

pub(super) fn merge_request_id(draft_id: &str, version: u64) -> String {
    format!("merge:{draft_id}@v{version}")
}

pub(super) fn check_title(title: &str) -> DraftResult<()> {
    if (1..=200).contains(&title.chars().count()) {
        Ok(())
    } else {
        Err(DraftError::InvalidTitle)
    }
}

/// Administrator drafts and requests may not stage proposal-only operations.
pub(super) fn check_stageable(ops: &[DraftOp]) -> DraftResult<()> {
    match ops.iter().find(|op| op.is_proposal_only()) {
        Some(op) => Err(DraftError::ProposalOnlyOp(op.kind())),
        None => Ok(()),
    }
}

/// The later operation needing the row `Target::Created(ord)` names, if any.
fn needed_by(ops: &[DraftOp], ord: usize) -> Option<usize> {
    ops.iter()
        .enumerate()
        .skip(ord + 1)
        .find_map(|(position, op)| {
            let mut probe = op.clone();
            let mut needed = false;
            probe.each_target_mut(|target| {
                if *target == Target::Created(ord) {
                    needed = true;
                }
            });
            needed.then_some(position)
        })
}

impl<S: ScheduleStore + DraftStore, I: IdSource, C: Clock> SchedulerService<S, I, C> {
    /// Start an administrator draft on the current head. `request_id` makes
    /// the create idempotent per author (exact repeat replays the draft, a
    /// different request is `RequestMismatch`). The expiry scope is derived
    /// once operations are staged, so creation takes none.
    pub async fn create_draft(
        &mut self,
        admin: &str,
        title: &str,
        request_id: Option<String>,
    ) -> DraftResult<StoredDraft> {
        check_title(title)?;
        let author = Actor::admin(admin);
        let at = self.clock.now();
        let (snapshot, head) = self.store.snapshot_with_head().await?;
        let digest = request_id
            .as_ref()
            .map(|_| digest("create_draft", &(title, author.kind(), author.id())));
        let new = NewDraft {
            id: self.ids.new_id(),
            kind: DraftKind::Admin,
            title: title.to_owned(),
            author,
            base: head,
            base_revision: snapshot.revision,
            request_type: None,
            subject: None,
            at,
            request: request_id.map(|request_id| DraftRequest {
                request_id,
                digest: digest.unwrap_or_default(),
            }),
            submit: None,
        };
        match self.store.create_draft(new).await? {
            DraftCreated::Created(draft) | DraftCreated::Replayed(draft) => Ok(draft),
            DraftCreated::Mismatch { draft_id } => Err(DraftError::RequestMismatch { draft_id }),
            // Administrator drafts carry no submission, so no limits apply.
            DraftCreated::Limited(_) => Err(DraftError::Store(StoreError::Constraint(
                "an administrator draft has no request limits".into(),
            ))),
        }
    }

    /// Stage one operation at the end. It is validated against the draft's
    /// base with the same checks the merge runs (preview-id targets refused,
    /// run timings checked, the service's own checks); the expiry scope is
    /// re-derived from the whole list in the same write.
    pub async fn add_draft_op(
        &mut self,
        admin: &str,
        draft_id: &str,
        expected_version: u64,
        op: DraftOp,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> DraftResult<LoadedDraft> {
        self.check_policy(policy)?;
        check_stageable(std::slice::from_ref(&op))?;
        let now = self.clock.now();
        let loaded = load_live(&self.store, draft_id, expected_version).await?;
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let mut staged: Vec<StagedOp> = loaded.ops.clone();
        staged.push(StagedOp {
            ord: staged.len(),
            op,
            author: Actor::admin(admin),
            added_at: now,
        });
        let ops: Vec<DraftOp> = staged.iter().map(|staged| staged.op.clone()).collect();
        let replayed =
            replay(&flow.base_snapshot, &ops, policy, directory, now).map_err(|rejected| {
                DraftError::ReplayFailed {
                    ord: rejected.ord,
                    error: rejected.error,
                }
            })?;
        let expires = expires_week(
            &ops,
            &replayed.created,
            &flow.base_snapshot,
            &replayed.draft,
            policy,
        );
        self.replace_ops(
            &loaded.draft,
            staged,
            DraftEventKind::OpAdded,
            loaded.ops.len(),
            expires,
            admin,
            now,
        )
        .await
    }

    /// Replace the operation at `ord`. Refused when a later operation needs
    /// the row the old operation creates and the new one does not.
    #[allow(clippy::too_many_arguments)]
    pub async fn edit_draft_op(
        &mut self,
        admin: &str,
        draft_id: &str,
        expected_version: u64,
        ord: usize,
        op: DraftOp,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> DraftResult<LoadedDraft> {
        self.check_policy(policy)?;
        check_stageable(std::slice::from_ref(&op))?;
        let now = self.clock.now();
        let loaded = load_live(&self.store, draft_id, expected_version).await?;
        if ord >= loaded.ops.len() {
            return Err(DraftError::EditRefused(EditRefusal::UnknownOp { ord }));
        }
        if !op.creates_row()
            && let Some(used_by) = needed_by(&loaded.draft_ops(), ord)
        {
            return Err(DraftError::EditRefused(EditRefusal::DanglingTarget {
                removed: ord,
                used_by,
            }));
        }
        let mut ops = loaded.draft_ops();
        ops[ord] = op;
        // The whole list must still replay on the base.
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let replayed =
            replay(&flow.base_snapshot, &ops, policy, directory, now).map_err(|rejected| {
                DraftError::ReplayFailed {
                    ord: rejected.ord,
                    error: rejected.error,
                }
            })?;
        let expires = expires_week(
            &ops,
            &replayed.created,
            &flow.base_snapshot,
            &replayed.draft,
            policy,
        );
        let staged = restage(&loaded.ops, &ops, admin, now);
        self.replace_ops(
            &loaded.draft,
            staged,
            DraftEventKind::OpEdited,
            ord,
            expires,
            admin,
            now,
        )
        .await
    }

    /// Remove the operation at `ord`, renumbering later created-target
    /// ordinals down by one. Refused when a later operation needs the row
    /// the removed operation creates.
    pub async fn remove_draft_op(
        &mut self,
        admin: &str,
        draft_id: &str,
        expected_version: u64,
        ord: usize,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> DraftResult<LoadedDraft> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let loaded = load_live(&self.store, draft_id, expected_version).await?;
        if ord >= loaded.ops.len() {
            return Err(DraftError::EditRefused(EditRefusal::UnknownOp { ord }));
        }
        let mut ops = loaded.draft_ops();
        ops.remove(ord);
        for op in &mut ops {
            if !renumber_created(op, |old| {
                if old == ord {
                    None
                } else if old > ord {
                    Some(old - 1)
                } else {
                    Some(old)
                }
            }) && let Some(used_by) = needed_by(&loaded.draft_ops(), ord)
            {
                return Err(DraftError::EditRefused(EditRefusal::DanglingTarget {
                    removed: ord,
                    used_by,
                }));
            }
        }
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let replayed =
            replay(&flow.base_snapshot, &ops, policy, directory, now).map_err(|rejected| {
                DraftError::ReplayFailed {
                    ord: rejected.ord,
                    error: rejected.error,
                }
            })?;
        let expires = expires_week(
            &ops,
            &replayed.created,
            &flow.base_snapshot,
            &replayed.draft,
            policy,
        );
        let staged = restage(&loaded.ops, &ops, admin, now);
        self.replace_ops(
            &loaded.draft,
            staged,
            DraftEventKind::OpRemoved,
            ord,
            expires,
            admin,
            now,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn replace_ops(
        &mut self,
        draft: &StoredDraft,
        mut staged: Vec<StagedOp>,
        event: DraftEventKind,
        ord: usize,
        expires_week: Option<DateTime<Utc>>,
        admin: &str,
        now: DateTime<Utc>,
    ) -> DraftResult<LoadedDraft> {
        for (position, op) in staged.iter_mut().enumerate() {
            op.ord = position;
        }
        let update = DraftUpdate {
            draft_id: draft.id.clone(),
            expected_version: draft.version,
            actor: Actor::admin(admin),
            at: now,
            change: DraftChange::ReplaceOps {
                ops: staged,
                event,
                ord,
                expires_week,
            },
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(draft) => Ok(self
                .store
                .load_draft(&draft.id)
                .await?
                .ok_or_else(|| DraftError::UnknownDraft(draft.id.clone()))?),
            DraftWrite::Stale(stale) => Err(stale_of(&draft.id, stale)),
        }
    }

    /// What merging now would do, without writing anything: the three-way
    /// analysis of the staged operations between the draft's base and the
    /// current schedule. Refused for an expired draft.
    pub async fn preview_draft(
        &self,
        draft_id: &str,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> DraftResult<MergeAnalysis> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let loaded = load_admin(&self.store, draft_id).await?;
        if is_expired(loaded.draft.scope.expires_week(), policy, now)? {
            return Err(DraftError::Expired);
        }
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        if expired_on_current(&flow.current, &loaded.draft_ops(), policy, directory, now)? {
            return Err(DraftError::Expired);
        }
        Ok(analyze_merge(
            &flow.base_snapshot,
            &flow.current,
            &loaded.draft_ops(),
            policy,
            directory,
            now,
        ))
    }

    /// Re-point the draft at the current head. Refused while the merge
    /// analysis reports conflicts; the operations must be fixed first.
    pub async fn rebase_draft(
        &mut self,
        admin: &str,
        draft_id: &str,
        expected_version: u64,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> DraftResult<StoredDraft> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let loaded = load_live(&self.store, draft_id, expected_version).await?;
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let analysis = analyze_merge(
            &flow.base_snapshot,
            &flow.current,
            &loaded.draft_ops(),
            policy,
            directory,
            now,
        );
        if !analysis.conflicts.is_empty() {
            return Err(DraftError::Conflicts(analysis.conflicts));
        }
        // The new base is the current schedule, so replay there for the
        // re-derived scope.
        let replayed = replay(&flow.current, &loaded.draft_ops(), policy, directory, now).map_err(
            |rejected| DraftError::ReplayFailed {
                ord: rejected.ord,
                error: rejected.error,
            },
        )?;
        let expires = expires_week(
            &loaded.draft_ops(),
            &replayed.created,
            &flow.current,
            &replayed.draft,
            policy,
        );
        let update = DraftUpdate {
            draft_id: loaded.draft.id.clone(),
            expected_version: loaded.draft.version,
            actor: Actor::admin(admin),
            at: now,
            change: DraftChange::Rebase {
                base: flow.head,
                base_revision: flow.current.revision,
                expires_week: expires,
            },
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(draft) => Ok(draft),
            DraftWrite::Stale(stale) => Err(stale_of(&loaded.draft.id, stale)),
        }
    }

    /// Discard a live draft for good; its operations stay in history.
    pub async fn discard_draft(
        &mut self,
        admin: &str,
        draft_id: &str,
        expected_version: u64,
        reason: Option<String>,
    ) -> DraftResult<StoredDraft> {
        let now = self.clock.now();
        let loaded = load_live(&self.store, draft_id, expected_version).await?;
        let update = DraftUpdate {
            draft_id: loaded.draft.id.clone(),
            expected_version: loaded.draft.version,
            actor: Actor::admin(admin),
            at: now,
            change: DraftChange::Close {
                status: DraftStatus::Discarded,
                reason,
                notices: Vec::new(),
            },
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(draft) => Ok(draft),
            DraftWrite::Stale(stale) => Err(stale_of(&loaded.draft.id, stale)),
        }
    }

    /// Merge the staged operations as one `DraftMerge` change: one record
    /// (`request_id` `merge:<id>@v<version>`, `refs` the base head), the
    /// draft closed, one summary notice per affected channel. Nothing is
    /// written when the analysis reports conflicts.
    ///
    /// # Errors
    /// [`DraftError::Conflicts`] (the report), [`DraftError::AlreadyMerged`]
    /// for a draft another merge closed, [`DraftError::AlreadyApplied`] for
    /// an exact retry, or a store failure.
    pub async fn merge_draft(
        &mut self,
        admin: &str,
        draft_id: &str,
        expected_version: u64,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
        request_id: Option<String>,
    ) -> DraftResult<MergeOutcome> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let actor = Actor::admin(admin);
        let request_id = request_id.unwrap_or_else(|| merge_request_id(draft_id, expected_version));
        let request_digest = digest("draft_merge", &(draft_id, expected_version));
        // The request id first (as scheduler commits do): an exact retry
        // replays even though the draft is already merged, while a new
        // request id for a merged draft reports `AlreadyMerged` below.
        if let Some(recorded) = self.store.recorded_request(&actor, &request_id).await? {
            if recorded.digest.as_deref() == Some(request_digest.as_str()) {
                return Err(DraftError::AlreadyApplied {
                    seq: recorded.committed.seq,
                    revision: recorded.committed.revision,
                });
            }
            return Err(DraftError::IdempotencyMismatch {
                seq: recorded.committed.seq,
            });
        }
        let loaded = load_admin(&self.store, draft_id).await?;
        let ops = loaded.draft_ops();
        self.merge_loaded(
            MergeInput {
                actor,
                surface: Surface::DraftMerge,
                request_id,
                request_digest,
                draft: &loaded.draft,
                ops,
                expected_version,
                summary: loaded.draft.title.clone(),
                note: None,
                authorise: None,
                status_at_apply: BTreeSet::new(),
                also_notify: Vec::new(),
                expired_notice: None,
                notices: None,
            },
            policy,
            directory,
            now,
        )
        .await
    }

    /// The merge shared by drafts and member requests: close an expired
    /// draft, check it is live at `expected_version`, then analyse, replay
    /// and commit `ops` in one record through `surface`, re-planning on a
    /// revision race.
    pub(super) async fn merge_loaded(
        &mut self,
        input: MergeInput<'_>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
        now: DateTime<Utc>,
    ) -> DraftResult<MergeOutcome> {
        let MergeInput {
            actor,
            surface,
            request_id,
            request_digest,
            draft,
            ops,
            expected_version,
            summary,
            note,
            authorise,
            status_at_apply,
            also_notify,
            expired_notice,
            notices: replaced,
        } = input;
        let draft_id = draft.id.as_str();
        // An expired draft is closed first (idempotently), so the window
        // between the reset and a failing tick expiry stays closed.
        if is_expired(draft.scope.expires_week(), policy, now)? {
            return Err(self.close_expired(draft, now, expired_notice).await);
        }
        if !draft.status.is_live() || draft.version != expected_version {
            return Err(stale_of(
                draft_id,
                DraftStale::Moved {
                    status: draft.status,
                    version: draft.version,
                    merged_seq: draft.merged_seq,
                },
            ));
        }
        if ops.is_empty() {
            return Err(DraftError::Empty);
        }
        let mut attempt = 1;
        loop {
            let flow = upstream(&self.store, &draft.base).await?;
            // Upstream may have moved a drafted run into a past week since the
            // scope was stored: re-derive it on the schedule being merged into.
            if expired_on_current(&flow.current, &ops, policy, directory, now)? {
                return Err(self.close_expired(draft, now, expired_notice).await);
            }
            if authorise.is_some_and(|allowed| !allowed(&flow.current)) {
                return Err(DraftError::RequesterUnauthorised);
            }
            let analysis = analyze_merge_applying(
                &flow.base_snapshot,
                &flow.current,
                &ops,
                policy,
                directory,
                now,
                &status_at_apply,
            );
            if !analysis.conflicts.is_empty() {
                return Err(DraftError::Conflicts(analysis.conflicts));
            }
            let preview =
                replay(&flow.current, &ops, policy, directory, now).map_err(|rejected| {
                    DraftError::ReplayFailed {
                        ord: rejected.ord,
                        error: rejected.error,
                    }
                })?;
            let real = replay_real(&flow.current, &ops, policy, directory, now, &mut self.ids)
                .map_err(|rejected| DraftError::ReplayFailed {
                    ord: rejected.ord,
                    error: rejected.error,
                })?;
            let warnings = skipped_runs(&real.results);
            let mismatch = replay_equivalent(&preview, &real);
            if !mismatch.is_empty() {
                return Err(DraftError::Conflicts(mismatch));
            }
            let (created, results) = (real.created, real.results);
            let merged = real.draft;
            // Routine materialisation the merge commits alongside the draft
            // stays quiet: compare against the current schedule materialised
            // the same way.
            let mut routine = Draft::new(flow.current.clone()).with_attendance(policy.attendance);
            materialise_weeks(&mut routine, &mut PreviewIds::default(), policy, now)?;
            let notices = match &replaced {
                Some(notices) => notices.clone(),
                None => merge_notices(
                    &flow.current,
                    &routine,
                    &merged,
                    draft_id,
                    expected_version,
                    &summary,
                ),
            };
            let weeks = analysis.weeks.clone();
            let changes = merged.into_changes();
            if changes.is_empty() {
                return Err(DraftError::NoEffect);
            }
            let meta = ChangeMeta {
                origin: Origin {
                    actor: actor.clone(),
                    surface,
                    request_id: Some(request_id.clone()),
                },
                at: now,
                notices: notices.iter().map(Notice::effect_kind).collect(),
                refs: vec![draft.base.clone()],
                request_digest: Some(request_digest.clone()),
                expect: Default::default(),
                outbox: notices.iter().chain(&also_notify).cloned().collect(),
            };
            match self
                .store
                .commit_merge(
                    flow.current.revision,
                    changes,
                    meta,
                    draft_id,
                    expected_version,
                    merged_note(note.as_deref(), &warnings),
                )
                .await
            {
                Ok(MergeCommit::Committed(committed)) if committed.replayed => {
                    return Err(DraftError::AlreadyApplied {
                        seq: committed.seq,
                        revision: committed.revision,
                    });
                }
                Ok(MergeCommit::Committed(committed)) => {
                    return Ok(MergeOutcome {
                        seq: committed.seq,
                        revision: committed.revision,
                        notices,
                        weeks,
                        warnings,
                        created,
                        results,
                    });
                }
                Ok(MergeCommit::Stale(stale)) => return Err(stale_of(draft_id, stale)),
                Err(StoreError::Conflict { .. }) if attempt < COMMIT_ATTEMPTS => attempt += 1,
                Err(error) => return Err(DraftError::Store(error)),
            }
        }
    }

    /// Close `draft` as expired (idempotently) as the system `delivery`
    /// actor, as the tick does, and report [`DraftError::Expired`].
    async fn close_expired(
        &mut self,
        draft: &StoredDraft,
        now: DateTime<Utc>,
        notice: Option<Notice>,
    ) -> DraftError {
        let update = DraftUpdate {
            draft_id: draft.id.clone(),
            expected_version: draft.version,
            actor: Actor::system(EXPIRY_ACTOR),
            at: now,
            change: DraftChange::Close {
                status: DraftStatus::Expired,
                reason: None,
                notices: notice.into_iter().collect(),
            },
        };
        match self.store.update_draft(update).await {
            Ok(DraftWrite::Written(_)) => DraftError::Expired,
            Ok(DraftWrite::Stale(stale)) => stale_of(&draft.id, stale),
            Err(error) => error.into(),
        }
    }

    /// Expire every live draft and member request whose boss week starts
    /// before the current one (weekly-timings-only ones never expire by
    /// week). Expired drafts stay in history. Each expired request's
    /// requester notice is planned first and written to the outbox by the
    /// expiry itself; the notices are also returned.
    pub async fn expire_due_drafts(&mut self, policy: &SchedulePolicy) -> DraftResult<DraftExpiry> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let week = policy
            .week_of(&now)
            .and_then(|start| crate::domain::schedule::utc_instant(&start))
            .map_err(crate::domain::schedule::ScheduleError::from)?;
        let planned = self.expiry_notices(week).await?;
        let ids = self
            .store
            .expire_drafts(week, now, &Actor::system(EXPIRY_ACTOR), planned.clone())
            .await?;
        let notices = planned
            .into_iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(_, notice)| notice)
            .collect();
        Ok(DraftExpiry { ids, notices })
    }

    /// The requester notice of every live request due to expire before
    /// `week`, by request id. Submitting or editing an expired request is
    /// refused, so the set the expiry closes is the one read here.
    async fn expiry_notices(&self, week: DateTime<Utc>) -> DraftResult<Vec<(String, Notice)>> {
        let mut due = Vec::new();
        for status in [DraftStatus::Open, DraftStatus::Submitted] {
            due.extend(
                self.store
                    .list_drafts(Some(status))
                    .await?
                    .into_iter()
                    .filter(|draft| {
                        draft.kind == crate::domain::drafts::DraftKind::Request
                            && draft.scope.expires_week().is_some_and(|at| at < week)
                    }),
            );
        }
        if due.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.store.load(&super::ports::Scope::All).await?;
        let mut notices = Vec::new();
        for draft in due {
            if let Some(loaded) = self.store.load_draft(&draft.id).await?
                && let Some(notice) = super::requests::decision_notice(
                    &loaded,
                    crate::domain::schedule::RequestDecision::Expired,
                    None,
                    &snapshot,
                )
            {
                notices.push((draft.id, notice));
            }
        }
        // The store expires in id order.
        notices.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(notices)
    }
}

/// One summary notice per home channel of the changed runs and timings (and
/// one channel-less notice when nothing has a channel). A run of a timing
/// the merge leaves unchanged is left out when it matches `routine` (the
/// current schedule after a quiet `materialise_weeks`): committing routine
/// materialisation is not the draft's doing.
fn merge_notices(
    before: &ScheduleSnapshot,
    routine: &Draft,
    after: &Draft,
    draft_id: &str,
    version: u64,
    title: &str,
) -> Vec<Notice> {
    let was = Draft::new(before.clone());
    let before_runs: BTreeMap<String, Run> = was
        .runs(None)
        .into_iter()
        .map(|run| (run.id.clone(), run))
        .collect();
    let after_runs: BTreeMap<String, Run> = after
        .runs(None)
        .into_iter()
        .map(|run| (run.id.clone(), run))
        .collect();
    let before_fixed: BTreeMap<String, FixedRun> = was
        .fixed_runs()
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();
    let after_fixed: BTreeMap<String, FixedRun> = after
        .fixed_runs()
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();
    let touched: BTreeSet<&String> = after_fixed
        .iter()
        .filter(|(id, row)| before_fixed.get(*id) != Some(*row))
        .map(|(id, _)| id)
        .chain(
            before_fixed
                .keys()
                .filter(|id| !after_fixed.contains_key(*id)),
        )
        .collect();
    // New runs have other ids in `routine`; a timing has one run per week.
    let is_routine = |id: &str| {
        let Some(run) = after.run(id) else {
            return false;
        };
        let Some(fixed) = run.fixed_run_id.as_ref() else {
            return false;
        };
        if touched.contains(fixed) {
            return false;
        }
        let twin = if before_runs.contains_key(id) {
            routine.run(id)
        } else {
            routine.run_for_fixed(fixed, run.week_start)
        };
        twin.is_some_and(|twin| {
            Run {
                id: run.id.clone(),
                ..twin.clone()
            } == *run
                && routine.rsvps(&twin.id) == after.rsvps(id)
        })
    };
    let mut by_channel: BTreeMap<Option<String>, (BTreeSet<String>, BTreeSet<String>)> =
        BTreeMap::new();
    let mut attribute = |id: &str, channel: Option<String>| {
        by_channel
            .entry(channel)
            .or_default()
            .0
            .insert(id.to_owned());
    };
    for (id, run) in &after_runs {
        if before_runs.get(id) != Some(run) && !is_routine(id) {
            // A run that changed channel is named in both summaries; a new
            // run has no old channel.
            attribute(id, run.channel_id.clone());
            if let Some(old) = before_runs.get(id)
                && old.channel_id != run.channel_id
            {
                attribute(id, old.channel_id.clone());
            }
        }
    }
    for id in before_runs.keys() {
        if !after_runs.contains_key(id)
            && let Some(run) = before_runs.get(id)
        {
            attribute(id, run.channel_id.clone());
        }
    }
    // Answers changed without touching the run row still name their run.
    for id in after_runs.keys().chain(before_runs.keys()) {
        let id = id.as_str();
        if was.rsvps(id) != after.rsvps(id) && !is_routine(id) {
            let channel = after_runs
                .get(id)
                .and_then(|run| run.channel_id.clone())
                .or_else(|| before_runs.get(id).and_then(|run| run.channel_id.clone()));
            attribute(id, channel);
        }
    }
    for (id, row) in &after_fixed {
        if before_fixed.get(id) != Some(row) {
            let channel = row
                .channel_id
                .clone()
                .or_else(|| before_fixed.get(id).and_then(|row| row.channel_id.clone()));
            by_channel.entry(channel).or_default().1.insert(id.clone());
        }
    }
    for id in before_fixed.keys() {
        if !after_fixed.contains_key(id)
            && let Some(row) = before_fixed.get(id)
        {
            by_channel
                .entry(row.channel_id.clone())
                .or_default()
                .1
                .insert(id.clone());
        }
    }
    if by_channel.is_empty() {
        by_channel.insert(None, (BTreeSet::new(), BTreeSet::new()));
    }
    by_channel
        .into_iter()
        .map(|(channel_id, (run_ids, fixed_ids))| Notice {
            change: NoticeChange::Merged {
                draft: draft_id.to_owned(),
                version,
                title: title.to_owned(),
                run_ids: run_ids.into_iter().collect(),
                fixed_ids: fixed_ids.into_iter().collect(),
            },
            channel_id,
            listed: Vec::new(),
            via_portal: true,
        })
        .collect()
}

/// Keep each staged operation's author and timestamp across an edit that
/// replaces the operation list.
fn restage(
    previous: &[StagedOp],
    ops: &[DraftOp],
    admin: &str,
    now: DateTime<Utc>,
) -> Vec<StagedOp> {
    ops.iter()
        .enumerate()
        .map(|(ord, op)| {
            let staged = previous.get(ord);
            StagedOp {
                ord,
                op: op.clone(),
                author: staged
                    .map(|staged| staged.author.clone())
                    .unwrap_or_else(|| Actor::admin(admin)),
                added_at: staged.map(|staged| staged.added_at).unwrap_or(now),
            }
        })
        .collect()
}
