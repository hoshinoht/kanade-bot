//! The draft merge member requests and proposals share, and draft expiry.
//!
//! The service plans against the store through [`DraftStore`]; every draft
//! write bumps its version, and the merge commits the schedule rows, the
//! history record, the draft's `merged` status and its event in one
//! transaction.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::ports::{Clock, IdSource, ScheduleStore, StoreError};
use super::service::{COMMIT_ATTEMPTS, SchedulerService};
use crate::completion::RunEnds;
use crate::drafts::{
    DraftChange, DraftOp, DraftStale, DraftStatus, DraftStore, DraftUpdate, DraftWrite,
    MergeCommit, MergeConflict, PreviewIds, Rejected, ReplayError, StoredDraft,
    analyze_merge_applying, expires_week, replay, replay_equivalent, replay_real,
};
use crate::history::{Actor, ChangeMeta, ChangeRef, HistoryGap, Origin, Surface, rewind};
use crate::members::Directory;
use crate::schedule::{
    Draft, FixedRun, Notice, NoticeChange, Run, ScheduleError, SchedulePolicy, ScheduleSnapshot,
    materialise_weeks,
};

/// Why a draft operation did not apply.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DraftError {
    #[error("{0}")]
    Schedule(ScheduleError),
    #[error("{0}")]
    Store(StoreError),
    /// No draft has this id.
    #[error("no draft `{0}`")]
    UnknownDraft(String),
    /// The draft is closed or at another version; nothing was written.
    #[error("the draft is {status} at version {version}")]
    Stale {
        status: DraftStatus,
        version: u64,
        merged_seq: Option<u64>,
    },
    /// Another merge already closed the draft, as change `seq`.
    #[error("the draft already merged as change {seq}")]
    AlreadyMerged { seq: u64 },
    /// The merge request id was already recorded, as change `seq`.
    #[error("merge already applied as change {seq} (revision {revision})")]
    AlreadyApplied { seq: u64, revision: u64 },
    /// The merge request id was already used for a different merge.
    #[error("merge request id already used by change {seq} for a different merge")]
    IdempotencyMismatch { seq: u64 },
    /// The records after the draft's base do not chain from it.
    #[error("{0}")]
    HistoryGap(HistoryGap),
    /// The merge cannot apply as it stands; nothing was written.
    #[error("the draft has {} conflicts", .0.len())]
    Conflicts(Vec<MergeConflict>),
    /// A staged operation no longer replays.
    #[error("operation {ord} cannot apply: {error}")]
    ReplayFailed { ord: usize, error: ReplayError },
    /// A merge with no staged operations.
    #[error("the draft stages no operations")]
    Empty,
    /// The merge replay changed no rows.
    #[error("the merge changed no rows")]
    NoEffect,
    /// A title outside 1..=200 characters.
    #[error("a draft title is 1 to 200 characters")]
    InvalidTitle,
    /// The create request id was already used for another draft.
    #[error("create request id already used for draft {draft_id}")]
    RequestMismatch { draft_id: String },
    /// The draft's boss week has passed; it is closed as expired.
    #[error("the draft's boss week has passed")]
    Expired,
    /// A request's requester may no longer have it merged.
    #[error("the requester may not make this request")]
    RequesterUnauthorised,
    /// The operation (its codec kind) exists only for proposals.
    #[error("`{0}` is staged only by extractor and chat proposals")]
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

impl From<Rejected> for DraftError {
    fn from(rejected: Rejected) -> Self {
        Self::ReplayFailed {
            ord: rejected.ord,
            error: rejected.error,
        }
    }
}

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
    pub results: Vec<crate::schedule::OpResult>,
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
pub(super) fn skipped_runs(results: &[crate::schedule::OpResult]) -> Vec<MergeWarning> {
    results
        .iter()
        .flat_map(|result| match result {
            crate::schedule::OpResult::PartyDelta(delta) => delta.emptied.clone(),
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
    /// Whether v4's fixed-timing notice carries the portal marker.
    pub via_portal: bool,
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
    /// conflicting with upstream (proposals only; empty for requests).
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
            let week = crate::schedule::utc_instant(&start)?;
            Ok(expires < week)
        }
    }
}

/// The current schedule (read with the history head in one store
/// transaction) and the base snapshot rewound from it.
pub(super) struct Upstream {
    pub current: ScheduleSnapshot,
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
    // Expiry reads only the week a draft touches: no run is frozen here.
    match replay(current, ops, policy, None, directory, now) {
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

/// Requests may not stage proposal-only operations.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn check_stageable(ops: &[DraftOp]) -> DraftResult<()> {
    match ops.iter().find(|op| op.is_proposal_only()) {
        Some(op) => Err(DraftError::ProposalOnlyOp(op.kind())),
        None => Ok(()),
    }
}

/// Replay `ops` on `base` with the checks a merge runs, and derive the
/// expiry scope they leave there.
pub(super) fn replay_scope(
    base: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> DraftResult<Option<DateTime<Utc>>> {
    let replayed = replay(base, ops, policy, ends, directory, now)?;
    Ok(expires_week(
        ops,
        &replayed.created,
        base,
        &replayed.draft,
        policy,
    ))
}

impl<S: ScheduleStore + DraftStore, I: IdSource, C: Clock> SchedulerService<S, I, C> {
    /// Write one change to the live `draft` as `actor` at `at`.
    pub(super) async fn update_as(
        &self,
        draft: &StoredDraft,
        actor: Actor,
        at: DateTime<Utc>,
        change: DraftChange,
    ) -> DraftResult<StoredDraft> {
        let update = DraftUpdate {
            draft_id: draft.id.clone(),
            expected_version: draft.version,
            actor,
            at,
            change,
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(draft) => Ok(draft),
            DraftWrite::Stale(stale) => Err(stale_of(&draft.id, stale)),
        }
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
            via_portal,
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
        let ends = self.run_ends();
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
                ends.as_ref(),
                directory,
                now,
                &status_at_apply,
            );
            if !analysis.conflicts.is_empty() {
                // A run past its end refuses the merge in its own words.
                if let Some((ord, error)) = analysis.conflicts.iter().find_map(|conflict| {
                    match conflict {
                        MergeConflict::OpRejected {
                            ord,
                            error: error @ ReplayError::Schedule(ScheduleError::RunEnded { .. }),
                            on_base: false,
                        } => Some((*ord, error.clone())),
                        _ => None,
                    }
                }) {
                    return Err(DraftError::ReplayFailed { ord, error });
                }
                return Err(DraftError::Conflicts(analysis.conflicts));
            }
            let preview = replay(&flow.current, &ops, policy, ends.as_ref(), directory, now)?;
            let real = replay_real(
                &flow.current,
                &ops,
                policy,
                ends.as_ref(),
                directory,
                now,
                &mut self.ids,
            )?;
            let mut timing_notices: Vec<Notice> = real
                .notices
                .iter()
                .filter(|notice| {
                    matches!(
                        &notice.change,
                        NoticeChange::FixedAdded { .. } | NoticeChange::FixedRemoved { .. }
                    )
                })
                .cloned()
                .collect();
            for notice in &mut timing_notices {
                notice.via_portal = via_portal;
            }
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
            let mut routine = Draft::new(flow.current.clone())
                .with_attendance(policy.attendance)
                .with_run_ends(ends.clone());
            materialise_weeks(&mut routine, &mut PreviewIds::default(), policy, now)?;
            let mut notices = match &replaced {
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
            notices.append(&mut timing_notices);
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
            .and_then(|start| crate::schedule::utc_instant(&start))
            .map_err(crate::schedule::ScheduleError::from)?;
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
                        draft.kind == crate::drafts::DraftKind::Request
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
                    crate::schedule::RequestDecision::Expired,
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
