//! Extractor and chatbot proposals on the scheduler service: propose,
//! approve (a merge through `Surface::ExtractionApproval` or
//! `Surface::ChatApproval`), reject, supersede and expire. Proposals are
//! drafts of kind `proposal`; every schedule effect goes through the shared
//! draft merge, so there is no second write path.

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Utc};

use super::drafts::{
    DraftError, EXPIRY_ACTOR, MergeInput, MergeOutcome, is_expired, stale_of, upstream,
};
use super::ports::{Clock, IdSource, ScheduleStore, Scope, StoreError};
use super::service::{SchedulerError, SchedulerService, digest};
use crate::domain::drafts::{
    DEFAULT_PROPOSAL_TTL, DraftChange, DraftOp, DraftStale, DraftStatus, DraftUpdate, DraftWrite,
    LoadedDraft, MergeAnalysis, NewProposal, ProposalCreated, ProposalInfo, ProposalSource,
    ProposalStore, SUPERSEDED, StagedOp, StoredDraft, Target, analyze_merge_applying, expires_week,
    replay,
};
use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::members::Directory;
use crate::domain::proposals::{
    Approver, ChangeKind, ProposalSubject, ProposedChange, Refusal, Translation, adoption_notes,
    fill_approver, live_timing_runs, may_commit, translate,
};
use crate::domain::schedule::{
    Notice, NoticeChange, OpResult, ScheduleError, SchedulePolicy, ScheduleSnapshot, utc_instant,
};
use crate::domain::time::to_iso;

/// Why a proposal action did not apply; nothing was written (except closing
/// a proposal found expired).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalError {
    /// The change cannot apply (at propose), or no longer applies (at ✅).
    Refused(Refusal),
    /// Applying it now would change nothing.
    NoEffect,
    /// The member may not approve or reject it.
    Unauthorised,
    /// The draft is not a proposal.
    NotAProposal,
    /// Past its TTL or its boss week; it is closed as expired.
    Expired,
    /// An edited approval of a change with no single time to edit.
    EditNotApplicable,
    /// An edited approval to a time in a boss week that has passed.
    EditInPast,
    Draft(DraftError),
}

impl From<DraftError> for ProposalError {
    fn from(error: DraftError) -> Self {
        match error {
            DraftError::Expired => Self::Expired,
            DraftError::NoEffect => Self::NoEffect,
            error => Self::Draft(error),
        }
    }
}

impl From<StoreError> for ProposalError {
    fn from(error: StoreError) -> Self {
        DraftError::from(error).into()
    }
}

impl From<ScheduleError> for ProposalError {
    fn from(error: ScheduleError) -> Self {
        DraftError::from(error).into()
    }
}

impl From<Refusal> for ProposalError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl fmt::Display for ProposalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::NoEffect => f.write_str("that is already the case"),
            Self::Unauthorised => f.write_str("that proposal is not yours to answer"),
            Self::NotAProposal => f.write_str("that is not a proposal"),
            Self::Expired => f.write_str("that proposal has expired"),
            Self::EditNotApplicable => f.write_str("that change has no time to edit"),
            Self::EditInPast => f.write_str("that time is in a boss week that has passed"),
            Self::Draft(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProposalError {}

pub type ProposalResult<T> = Result<T, ProposalError>;

/// Whether a new proposal retires the live ones with its target (the store
/// supersede key). Live callers use `Older`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Supersede {
    Older,
    Keep,
}

/// A change to stage, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalRequest {
    pub change: ProposedChange,
    pub source: ProposalSource,
    /// The extraction log or chat interaction id.
    pub source_id: String,
    pub supersede: Supersede,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposed {
    pub proposal: StoredDraft,
    pub subject: ProposalSubject,
    /// Live proposals it superseded.
    pub superseded: Vec<String>,
}

/// A merged proposal: the merge (its notices take the draft-merge outbox
/// path) and what the card reports (v4 `CommitResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalApproved {
    pub merge: MergeOutcome,
    pub kind: ChangeKind,
    pub run_id: Option<String>,
    pub fixed_run_id: Option<String>,
    pub created_run_ids: Vec<String>,
    pub old_datetime: Option<DateTime<Utc>>,
    /// Sibling proposals the approval retired.
    pub superseded: Vec<String>,
    pub notes: Vec<String>,
    /// Follow-up work after the committed merge that failed; reported,
    /// never an error (the merge stands).
    pub follow_up_errors: Vec<String>,
}

/// What approving a proposal now would do, for an administrator; nothing
/// is written and nobody's authority is checked.
#[derive(Clone, Debug)]
pub struct ProposalPreview {
    pub version: u64,
    pub status: DraftStatus,
    pub info: ProposalInfo,
    pub subject: ProposalSubject,
    /// Past its TTL or boss week, not yet closed by the tick.
    pub expired: bool,
    /// Between its base and the current schedule, with the approval's
    /// status-at-apply rules.
    pub analysis: MergeAnalysis,
    /// Approving would change nothing.
    pub no_effect: bool,
    /// Why approving would be refused now, in v4's words.
    pub refusal: Option<Refusal>,
}

/// v4 `supersede`: which live proposals to retire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupersedeScope<'a> {
    pub run_id: Option<&'a str>,
    pub channel_id: Option<&'a str>,
    pub bosses: &'a [String],
    pub keep: Option<&'a str>,
    /// Where the retiring row lives: it retires its own channel's cards, or
    /// every card when it is the run's home channel.
    pub from_channel: Option<&'a str>,
    pub by: ProposalSource,
}

fn surface_of(source: ProposalSource) -> Surface {
    match source {
        ProposalSource::Extraction => Surface::ExtractionApproval,
        ProposalSource::Chat => Surface::ChatApproval,
    }
}

/// v4 parity: an approved proposal announces only a move (`_announce_move`,
/// `amend_notice`); the card itself shows every other decision.
fn move_notice(
    subject: &ProposalSubject,
    ops: &[DraftOp],
    old_datetime: Option<DateTime<Utc>>,
    snapshot: &ScheduleSnapshot,
    approver: &Approver,
) -> Vec<Notice> {
    let to = ops.iter().find_map(|op| match op {
        DraftOp::AmendRun { to, .. } => Some(*to),
        _ => None,
    });
    let run = subject
        .run_id
        .as_ref()
        .and_then(|id| snapshot.runs.iter().find(|run| &run.id == id));
    match (subject.kind, run, old_datetime, to) {
        (ChangeKind::Move, Some(run), Some(from), Some(to)) => vec![Notice {
            change: NoticeChange::RunMoved {
                run_id: run.id.clone(),
                from,
                to,
            },
            channel_id: run.channel_id.clone(),
            listed: run.participants.clone(),
            via_portal: approver.via_portal,
        }],
        _ => Vec::new(),
    }
}

fn removes_timing(ops: &[DraftOp]) -> bool {
    ops.iter()
        .any(|op| matches!(op, DraftOp::RetireFixedRun { .. }))
}

/// The user decision on approval: a participant of the affected run (or
/// timing), an administrator, or its owner (the timing's).
fn allowed(subject: &ProposalSubject, approver: &Approver, snapshot: &ScheduleSnapshot) -> bool {
    let timing =
        |id: Option<&str>| id.and_then(|id| snapshot.fixed_runs.iter().find(|row| row.id == id));
    let (participants, owner) = if let Some(run_id) = &subject.run_id {
        let run = snapshot.runs.iter().find(|run| &run.id == run_id);
        (
            run.map(|run| run.participants.as_slice()),
            timing(run.and_then(|run| run.fixed_run_id.as_deref())),
        )
    } else {
        let row = timing(subject.fixed_run_id.as_deref());
        (row.map(|row| row.participants.as_slice()), row)
    };
    may_commit(
        &subject.named,
        participants,
        &approver.user_id,
        approver.has_role,
        approver.is_admin,
        owner.is_some_and(|row| row.owner_id == approver.user_id),
    )
}

/// What must still hold before merging: the target exists, and carded
/// answers are for members still on the run.
fn still_applies(
    subject: &ProposalSubject,
    ops: &[DraftOp],
    snapshot: &ScheduleSnapshot,
) -> Result<(), Refusal> {
    if let Some(run_id) = &subject.run_id {
        let Some(run) = snapshot.runs.iter().find(|run| &run.id == run_id) else {
            return Err(Refusal::RunGone);
        };
        let outsider = ops.iter().any(|op| {
            matches!(op, DraftOp::SetRsvp { user_id, .. } if !run.participants.contains(user_id))
        });
        if outsider {
            return Err(Refusal::AnswerForOutsider);
        }
    }
    if let Some(fixed) = &subject.fixed_run_id
        && !snapshot.fixed_runs.iter().any(|row| &row.id == fixed)
    {
        return Err(if removes_timing(ops) {
            Refusal::TimingAlreadyGone
        } else {
            Refusal::TimingGone
        });
    }
    Ok(())
}

/// Runs whose status a proposal sets as of approval (v4 `_cancel`/`_otot`
/// and `_rsvp`'s recount): it wins over upstream status changes.
fn status_at_apply(ops: &[DraftOp]) -> BTreeSet<String> {
    ops.iter()
        .filter_map(|op| match op {
            DraftOp::SetStatus {
                run: Target::Existing(id),
                ..
            }
            | DraftOp::RecountRun {
                run: Target::Existing(id),
            } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// The operations with their one scheduled instant moved to `to` (the
/// portal's "edit, then approve"); `None` when they already hold `to`.
fn retime(
    ops: &[DraftOp],
    to: DateTime<Utc>,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> ProposalResult<Option<Vec<DraftOp>>> {
    let timed = |op: &DraftOp| matches!(op, DraftOp::AmendRun { .. } | DraftOp::CreateRun { .. });
    if ops.iter().filter(|op| timed(op)).count() != 1 {
        return Err(ProposalError::EditNotApplicable);
    }
    let mut ops = ops.to_vec();
    let week = policy
        .week_of(&to)
        .and_then(|start| utc_instant(&start))
        .map_err(ScheduleError::from)?;
    for op in &mut ops {
        match op {
            DraftOp::AmendRun { to: at, .. } | DraftOp::CreateRun { datetime: at, .. }
                if *at == to =>
            {
                return Ok(None);
            }
            DraftOp::AmendRun { to: at, .. } => *at = to,
            DraftOp::CreateRun {
                datetime,
                week_start,
                ..
            } => {
                *datetime = to;
                *week_start = week;
            }
            _ => {}
        }
    }
    // Merging into a past week would close the proposal as expired.
    if is_expired(Some(week), policy, now)? {
        return Err(ProposalError::EditInPast);
    }
    Ok(Some(ops))
}

/// The one scheduled instant an edit would replace, if there is exactly one.
fn proposed_instant(ops: &[DraftOp]) -> Option<DateTime<Utc>> {
    let mut instants = ops.iter().filter_map(|op| match op {
        DraftOp::AmendRun { to, .. } => Some(*to),
        DraftOp::CreateRun { datetime, .. } => Some(*datetime),
        _ => None,
    });
    match (instants.next(), instants.next()) {
        (Some(at), None) => Some(at),
        _ => None,
    }
}

fn subject_of(loaded: &LoadedDraft) -> ProposalResult<ProposalSubject> {
    loaded
        .draft
        .subject
        .as_deref()
        .and_then(ProposalSubject::parse)
        .ok_or(ProposalError::NotAProposal)
}

impl<S: ScheduleStore + ProposalStore, I: IdSource, C: Clock> SchedulerService<S, I, C> {
    /// Stage a proposal. The change is translated and dry-run on the current
    /// schedule; one that cannot apply (or would change nothing) is refused
    /// with v4's reason and nothing is written (`D-PROPOSE-REFUSES`).
    pub async fn propose(
        &mut self,
        request: ProposalRequest,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> ProposalResult<Proposed> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let (snapshot, head) = self.store.snapshot_with_head().await?;
        let Translation { ops, subject } = translate(&request.change, &snapshot, policy, now)?;
        let replayed = replay(&snapshot, &ops, policy, directory, now).map_err(|rejected| {
            Refusal::from_replay(subject.kind, removes_timing(&ops), &rejected.error)
        })?;
        if replayed.draft.clone().into_changes().is_empty() {
            return Err(ProposalError::NoEffect);
        }
        let expires = expires_week(&ops, &replayed.created, &snapshot, &replayed.draft, policy);
        if is_expired(expires, policy, now)? {
            return Err(ProposalError::Expired);
        }
        let author = Actor::system(request.source.as_str());
        let new = NewProposal {
            id: self.ids.new_id(),
            title: format!("{} proposal", subject.kind.as_str()),
            author: author.clone(),
            base: head,
            base_revision: snapshot.revision,
            subject: Some(subject.encode()),
            at: now,
            ops: ops
                .into_iter()
                .enumerate()
                .map(|(ord, op)| StagedOp {
                    ord,
                    op,
                    author: author.clone(),
                    added_at: now,
                })
                .collect(),
            expires_week: expires,
            source: request.source,
            source_id: request.source_id,
            supersede_key: match request.supersede {
                Supersede::Older => subject.supersede_key(),
                Supersede::Keep => None,
            },
            ttl: DEFAULT_PROPOSAL_TTL,
        };
        Ok(match self.store.create_proposal(new).await? {
            ProposalCreated::Created { draft, superseded } => Proposed {
                proposal: draft,
                subject,
                superseded,
            },
            ProposalCreated::Replayed(draft) => Proposed {
                proposal: draft,
                subject,
                superseded: Vec::new(),
            },
        })
    }

    /// Load a live proposal `approver` may answer; one past its TTL is then
    /// closed as expired (`D-EXPIRED-REFUSED`: v4 applied it). Authority is
    /// checked first, so anyone else's reaction changes nothing.
    async fn live_proposal(
        &mut self,
        id: &str,
        approver: &Approver,
        now: DateTime<Utc>,
    ) -> ProposalResult<(LoadedDraft, ProposalInfo, ProposalSubject, ScheduleSnapshot)> {
        let (loaded, info) = self
            .store
            .load_proposal(id)
            .await?
            .ok_or_else(|| DraftError::UnknownDraft(id.to_owned()))?;
        let subject = subject_of(&loaded)?;
        let snapshot = self.store.load(&Scope::All).await?;
        if !allowed(&subject, approver, &snapshot) {
            return Err(ProposalError::Unauthorised);
        }
        let draft = &loaded.draft;
        if draft.status.is_live() && info.expires_at <= now {
            self.close_proposal(
                draft,
                Actor::system(EXPIRY_ACTOR),
                DraftStatus::Expired,
                None,
            )
            .await?;
            return Err(ProposalError::Expired);
        }
        if !draft.status.is_live() {
            return Err(stale_of(
                id,
                DraftStale::Moved {
                    status: draft.status,
                    version: draft.version,
                    merged_seq: draft.merged_seq,
                },
            )
            .into());
        }
        Ok((loaded, info, subject, snapshot))
    }

    async fn close_proposal(
        &mut self,
        draft: &StoredDraft,
        actor: Actor,
        status: DraftStatus,
        reason: Option<String>,
    ) -> ProposalResult<StoredDraft> {
        let update = DraftUpdate {
            draft_id: draft.id.clone(),
            expected_version: draft.version,
            actor,
            at: self.clock.now(),
            change: DraftChange::Close {
                status,
                reason,
                notices: Vec::new(),
            },
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(draft) => Ok(draft),
            DraftWrite::Stale(stale) => Err(stale_of(&draft.id, stale).into()),
        }
    }

    /// Approve (✅) a proposal as `approver`: authorised on the current
    /// schedule and again at every merge attempt, then merged as one record
    /// (`request_id` `approve:<id>`, so a repeated ✅ is `AlreadyApplied`
    /// and writes and posts nothing). Afterwards the sibling proposals about
    /// the same target are retired (v4 `commit`), and a new weekly timing's
    /// weeks are materialised. Nothing after the merge returns an error.
    /// A cancel/otot target and a recount apply to the run as it is at
    /// approval, whatever its status did meanwhile (v4 parity).
    ///
    /// A repeated ✅ (`AlreadyApplied`, or `AlreadyMerged`) by the member who
    /// merged it or an admin, still authorised now, re-runs those idempotent
    /// follow-ups before returning the error, so a crash right after the
    /// merge commit does not leave them undone. Anyone else's has no effect.
    pub async fn approve_proposal(
        &mut self,
        id: &str,
        approver: &Approver,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> ProposalResult<ProposalApproved> {
        self.approve_proposal_at(id, approver, None, policy, directory)
            .await
    }

    /// [`Self::approve_proposal`] with an optional edited time (v4's portal
    /// "edit, then approve"): the proposal's one scheduled instant (a move's
    /// target, a new run's slot) becomes `edit`, dry-run on the current
    /// schedule and merged as the same single record, with `edited=<instant>`
    /// in the `merged` event. The proposal itself is never changed, so its
    /// card never shows a time ✅ would not apply. The edit is part of the
    /// request digest: repeating it, or a plain ✅ by the member who merged it
    /// edited, is `AlreadyApplied`; another edit under the same
    /// `approve:<id>` is `IdempotencyMismatch`. The recorded request is
    /// looked up before the edit is checked, so a retry after a boss-week
    /// reset still answers its first result. An edit
    /// equal to the proposed time is a plain approval.
    pub async fn approve_proposal_at(
        &mut self,
        id: &str,
        approver: &Approver,
        edit: Option<DateTime<Utc>>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> ProposalResult<ProposalApproved> {
        let result = self
            .approve_once(id, approver, edit, policy, directory)
            .await;
        if let Err(ProposalError::Draft(
            DraftError::AlreadyApplied { .. } | DraftError::AlreadyMerged { .. },
        )) = &result
        {
            self.repeat_follow_ups(id, approver, policy).await;
        }
        result
    }

    /// The follow-ups of an already merged proposal; failures stay for the
    /// next ✅ (the merge is what the caller reports).
    async fn repeat_follow_ups(&mut self, id: &str, approver: &Approver, policy: &SchedulePolicy) {
        let Ok(Some((loaded, info))) = self.store.load_proposal(id).await else {
            return;
        };
        let Ok(subject) = subject_of(&loaded) else {
            return;
        };
        let draft = &loaded.draft;
        let actor = Actor::member(approver.user_id.clone());
        // Only the merger or an admin: an old card must not let anyone
        // retire newer proposals or write records in their name.
        let merger = approver.is_admin || draft.closed_by.as_ref() == Some(&actor);
        if draft.status != DraftStatus::Merged || !merger {
            return;
        }
        let Ok(snapshot) = self.store.load(&Scope::All).await else {
            return;
        };
        if !allowed(&subject, approver, &snapshot) {
            return;
        }
        let new_timing = loaded
            .draft_ops()
            .iter()
            .any(|op| matches!(op, DraftOp::AddFixedRun(_)));
        let merged_at = draft.updated_at;
        self.follow_ups(
            id,
            &subject,
            info.source,
            new_timing,
            &actor,
            merged_at,
            policy,
            &mut Vec::new(),
        )
        .await;
    }

    /// After a merge: materialise a new timing's weeks (its own record,
    /// `approve:<id>:materialise`), then retire the sibling proposals (v4
    /// `commit`'s `supersede`) that were live at the merge — created at or
    /// before `merged_at`, never newer ones. Both are idempotent. Returns
    /// the retired ids and whether materialising succeeded; failures go to
    /// `errors`.
    #[allow(clippy::too_many_arguments)]
    async fn follow_ups(
        &mut self,
        id: &str,
        subject: &ProposalSubject,
        source: ProposalSource,
        new_timing: bool,
        actor: &Actor,
        merged_at: DateTime<Utc>,
        policy: &SchedulePolicy,
        errors: &mut Vec<String>,
    ) -> (Vec<String>, bool) {
        let mut materialised = false;
        if new_timing {
            let origin = Origin::new(actor.clone(), surface_of(source))
                .with_request_id(format!("approve:{id}:materialise"));
            match self.as_origin(origin).materialise_weeks(policy).await {
                Ok(_) | Err(SchedulerError::AlreadyApplied { .. }) => materialised = true,
                Err(error) => errors.push(error.to_string()),
            }
        }
        let scope = SupersedeScope {
            run_id: subject.run_id.as_deref(),
            channel_id: subject.channel_id.as_deref(),
            bosses: &subject.bosses,
            keep: Some(id),
            from_channel: subject.channel_id.as_deref(),
            by: source,
        };
        let superseded = match self.retire(scope, Some(merged_at)).await {
            Ok(ids) => ids,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        };
        (superseded, materialised)
    }

    async fn approve_once(
        &mut self,
        id: &str,
        approver: &Approver,
        edit: Option<DateTime<Utc>>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> ProposalResult<ProposalApproved> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let actor = Actor::member(approver.user_id.clone());
        let request_id = format!("approve:{id}");
        // Stored operations never change, so the edit normalises the same
        // way on every retry; an edit equal to the proposed time is none.
        let edit = match edit {
            None => None,
            Some(to) => {
                let (loaded, _) = self
                    .store
                    .load_proposal(id)
                    .await?
                    .ok_or_else(|| DraftError::UnknownDraft(id.to_owned()))?;
                let ops = loaded.draft_ops();
                (proposed_instant(&ops) != Some(to)).then_some((to, ops))
            }
        };
        let request_digest = match &edit {
            None => digest("proposal_approve", &id),
            Some((to, _)) => digest("proposal_approve", &(id, to)),
        };
        // The exact-retry lookup runs first, so a retry answers its recorded
        // result whatever happened since (a boss-week reset included).
        if let Some(recorded) = self.store.recorded_request(&actor, &request_id).await? {
            let same = recorded.digest.as_deref() == Some(request_digest.as_str());
            // A plain ✅ after this member's own edited approval (the card
            // not yet refreshed) is a repeat, not a different request.
            let edited_by_them = edit.is_none() && {
                let merged = self.store.load_proposal(id).await?;
                merged.is_some_and(|(loaded, _)| {
                    loaded.draft.status == DraftStatus::Merged
                        && loaded.draft.merged_seq == Some(recorded.committed.seq)
                        && loaded.draft.closed_by.as_ref() == Some(&actor)
                })
            };
            return Err(if same || edited_by_them {
                DraftError::AlreadyApplied {
                    seq: recorded.committed.seq,
                    revision: recorded.committed.revision,
                }
            } else {
                DraftError::IdempotencyMismatch {
                    seq: recorded.committed.seq,
                }
            }
            .into());
        }
        let edited = match edit {
            None => None,
            Some((to, ops)) => retime(&ops, to, policy, now)?.map(|ops| (to, ops)),
        };
        let (loaded, info, subject, snapshot) = self.live_proposal(id, approver, now).await?;
        let (mut ops, note) = match edited {
            Some((to, ops)) => (
                ops,
                Some(format!(
                    "edited={}",
                    to_iso(&to).map_err(ScheduleError::from)?
                )),
            ),
            None => (loaded.draft_ops(), None),
        };
        still_applies(&subject, &ops, &snapshot)?;
        fill_approver(&mut ops, &approver.user_id);
        if note.is_some() {
            // The edited time was never dry-run: refuse in v4's words, as
            // proposing does.
            replay(&snapshot, &ops, policy, directory, now).map_err(|rejected| {
                Refusal::from_replay(subject.kind, removes_timing(&ops), &rejected.error)
            })?;
        }
        // Read before the merge: nothing after it may fail.
        let old_datetime = ops.iter().find_map(|op| match op {
            DraftOp::AmendRun { .. } => subject
                .run_id
                .as_ref()
                .and_then(|run_id| snapshot.runs.iter().find(|run| &run.id == run_id))
                .map(|run| run.datetime),
            _ => None,
        });
        let edited = ops
            .iter()
            .any(|op| matches!(op, DraftOp::ApplyFixedEdit { .. }));
        let edit_count = subject
            .fixed_run_id
            .as_deref()
            .filter(|_| edited)
            .map(|fixed| live_timing_runs(&snapshot, fixed, policy, now));
        let announced = move_notice(&subject, &ops, old_datetime, &snapshot, approver);
        let surface = surface_of(info.source);
        let removing = removes_timing(&ops);
        let still_allowed = |current: &ScheduleSnapshot| allowed(&subject, approver, current);
        let merge = self
            .merge_loaded(
                MergeInput {
                    actor: actor.clone(),
                    surface,
                    via_portal: approver.via_portal,
                    request_id: request_id.clone(),
                    request_digest,
                    draft: &loaded.draft,
                    ops: ops.clone(),
                    expected_version: loaded.draft.version,
                    summary: format!("{} proposal", subject.kind.as_str()),
                    note,
                    authorise: Some(&still_allowed),
                    status_at_apply: status_at_apply(&ops),
                    also_notify: Vec::new(),
                    expired_notice: None,
                    notices: Some(announced),
                },
                policy,
                directory,
                now,
            )
            .await
            .map_err(|error| match error {
                DraftError::RequesterUnauthorised => ProposalError::Unauthorised,
                DraftError::ReplayFailed { error, .. } => {
                    Refusal::from_replay(subject.kind, removing, &error).into()
                }
                error => error.into(),
            })?;

        let created = |wanted: fn(&DraftOp) -> bool| -> Vec<String> {
            ops.iter()
                .zip(&merge.created)
                .filter(|(op, _)| wanted(op))
                .filter_map(|(_, id)| id.clone())
                .collect()
        };
        let created_run_ids = created(|op| matches!(op, DraftOp::CreateRun { .. }));
        let new_timing = created(|op| matches!(op, DraftOp::AddFixedRun(_)))
            .into_iter()
            .next();
        let mut notes = Vec::new();
        let mut follow_up_errors = Vec::new();
        for (op, result) in ops.iter().zip(&merge.results) {
            if let (DraftOp::RetireFixedRun { .. }, OpResult::Count(count)) = (op, result) {
                notes.push(format!("cancelled {count} scheduled run(s)"));
            }
        }
        if let Some(count) = edit_count {
            notes.push(format!("updated {count} scheduled run(s)"));
        }
        let (superseded, materialised) = self
            .follow_ups(
                id,
                &subject,
                info.source,
                new_timing.is_some(),
                &actor,
                now,
                policy,
                &mut follow_up_errors,
            )
            .await;
        if let (Some(fixed), true) = (&new_timing, materialised) {
            match self.store.load(&Scope::All).await {
                Ok(state) => notes.extend(adoption_notes(&state, fixed, policy, now)),
                Err(error) => follow_up_errors.push(error.to_string()),
            }
        }
        Ok(ProposalApproved {
            kind: subject.kind,
            run_id: match subject.kind {
                ChangeKind::Add => created_run_ids.first().cloned(),
                ChangeKind::Fix => None,
                _ => subject.run_id.clone(),
            },
            fixed_run_id: new_timing.or(subject.fixed_run_id),
            created_run_ids,
            old_datetime,
            superseded,
            notes,
            follow_up_errors,
            merge,
        })
    }

    /// What approving the proposal now would do, for an administrator: the
    /// three-way analysis of its operations (the approver, when given,
    /// fills an unnamed party as approval would), whether it would change
    /// nothing, and why it would be refused. Nothing is written, not even
    /// closing an expired proposal.
    pub async fn preview_proposal(
        &self,
        id: &str,
        approver: Option<&str>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> ProposalResult<ProposalPreview> {
        self.check_policy(policy)?;
        let now = self.clock.now();
        let (loaded, info) = self
            .store
            .load_proposal(id)
            .await?
            .ok_or_else(|| DraftError::UnknownDraft(id.to_owned()))?;
        let subject = subject_of(&loaded)?;
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let mut ops = loaded.draft_ops();
        let mut refusal = still_applies(&subject, &ops, &flow.current).err();
        if let Some(user) = approver {
            fill_approver(&mut ops, user);
        }
        let analysis = analyze_merge_applying(
            &flow.base_snapshot,
            &flow.current,
            &ops,
            policy,
            directory,
            now,
            &status_at_apply(&ops),
        );
        if refusal.is_none()
            && let Err(rejected) = replay(&flow.current, &ops, policy, directory, now)
        {
            refusal = Some(Refusal::from_replay(
                subject.kind,
                removes_timing(&ops),
                &rejected.error,
            ));
        }
        let expired =
            info.expires_at <= now || is_expired(loaded.draft.scope.expires_week(), policy, now)?;
        Ok(ProposalPreview {
            version: loaded.draft.version,
            status: loaded.draft.status,
            no_effect: analysis
                .result_changes
                .as_ref()
                .is_some_and(|changes| changes.is_empty()),
            info,
            subject,
            expired,
            analysis,
            refusal,
        })
    }

    /// Reject (❌) a live proposal; the same members may as for ✅. No
    /// schedule record is written.
    pub async fn reject_proposal(
        &mut self,
        id: &str,
        approver: &Approver,
    ) -> ProposalResult<StoredDraft> {
        let now = self.clock.now();
        let (loaded, ..) = self.live_proposal(id, approver, now).await?;
        self.close_proposal(
            &loaded.draft,
            Actor::member(approver.user_id.clone()),
            DraftStatus::Rejected,
            None,
        )
        .await
    }

    /// v4 `supersede`: retire the live proposals about the same run
    /// (channel-scoped by `from_channel`), else about the same new boss set
    /// in `channel_id`, except `keep`. Closed `discarded`, reason
    /// `superseded`, by the `by` component.
    pub async fn supersede_proposals(
        &mut self,
        scope: SupersedeScope<'_>,
    ) -> ProposalResult<Vec<String>> {
        self.retire(scope, None).await
    }

    /// [`Self::supersede_proposals`], only among proposals created at or
    /// before `live_at` when given (a merge retires what it replaced, never
    /// what was proposed after it).
    async fn retire(
        &mut self,
        scope: SupersedeScope<'_>,
        live_at: Option<DateTime<Utc>>,
    ) -> ProposalResult<Vec<String>> {
        let live = self.store.list_proposals(true).await?;
        let others = live.into_iter().filter_map(|stored| {
            let subject = stored
                .draft
                .subject
                .as_deref()
                .and_then(ProposalSubject::parse)?;
            let older = live_at.is_none_or(|at| stored.draft.created_at <= at);
            (older && Some(stored.draft.id.as_str()) != scope.keep)
                .then_some((stored.draft, subject))
        });
        let candidates: Vec<StoredDraft> = if let Some(run_id) = scope.run_id {
            let mine: Vec<(StoredDraft, ProposalSubject)> = others
                .filter(|(_, subject)| subject.run_id.as_deref() == Some(run_id))
                .collect();
            let home = match scope.from_channel {
                None => None,
                Some(_) => self
                    .store
                    .load(&Scope::Run(run_id.to_owned()))
                    .await?
                    .runs
                    .into_iter()
                    .find(|run| run.id == run_id)
                    .and_then(|run| run.channel_id),
            };
            mine.into_iter()
                .filter(|(_, subject)| match scope.from_channel {
                    None => true,
                    Some(from) if home.as_deref() == Some(from) => true,
                    Some(from) => subject.channel_id.as_deref() == Some(from),
                })
                .map(|(draft, _)| draft)
                .collect()
        } else if !scope.bosses.is_empty() && scope.channel_id.is_some() {
            others
                .filter(|(_, subject)| {
                    subject.run_id.is_none()
                        && subject.channel_id.as_deref() == scope.channel_id
                        && subject.same_bosses(scope.bosses)
                })
                .map(|(draft, _)| draft)
                .collect()
        } else {
            return Ok(Vec::new());
        };
        let mut retired = Vec::new();
        for draft in candidates {
            let closed = self
                .close_proposal(
                    &draft,
                    Actor::system(scope.by.as_str()),
                    DraftStatus::Discarded,
                    Some(SUPERSEDED.to_owned()),
                )
                .await;
            match closed {
                Ok(_) => retired.push(draft.id),
                // Closed meanwhile: nothing left to retire.
                Err(ProposalError::Draft(
                    DraftError::Stale { .. } | DraftError::AlreadyMerged { .. },
                ))
                | Err(ProposalError::Expired) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(retired)
    }

    /// Expire every live proposal at or past its TTL deadline, as the
    /// system `delivery` actor; returns their ids. Nothing is posted.
    pub async fn expire_due_proposals(&mut self) -> ProposalResult<Vec<String>> {
        let now = self.clock.now();
        Ok(self
            .store
            .expire_proposals(now, &Actor::system(EXPIRY_ACTOR))
            .await?)
    }
}
