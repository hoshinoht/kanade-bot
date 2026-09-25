//! Member requests on the scheduler service: submit, withdraw, admin edit,
//! approve (a merge through `Surface::RequestMerge`) and reject. Requests
//! are drafts of kind `request`; the administrator draft methods refuse
//! them and these methods refuse administrator drafts.

use std::fmt;

use chrono::{DateTime, Utc};

use super::drafts::{
    DraftError, MergeInput, MergeOutcome, MergeWarning, check_title, is_expired, merge_request_id,
    skipped_runs, stale_of, upstream,
};
use super::ports::{Clock, IdSource, ScheduleStore, Scope};
use super::service::{SchedulerService, digest};
use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEventKind, DraftKind, DraftOp, DraftRequest, DraftStale,
    DraftStatus, DraftStore, DraftUpdate, DraftWrite, LoadedDraft, MergeAnalysis, NewDraft,
    RequestLimit, StagedOp, StoredDraft, Submission, analyze_merge, expires_week, replay,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::members::Directory;
use crate::domain::requests::{
    DEFAULT_LIMITS, MemberGate, RequestRefusal, RequestSpec, RequestType, Subject, authorise,
    check_reason, choices_note, home_channel, operations, public_summary, requester_notice,
};
use crate::domain::schedule::{
    FixedEditChoices, Notice, RequestDecision, SchedulePolicy, ScheduleSnapshot,
};

/// Why a request action did not apply; nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    Refused(RequestRefusal),
    /// A draft-level refusal: stale version, conflicts, replay failure,
    /// store failure, idempotency...
    Draft(DraftError),
    /// Over the pending cap or the rolling submission limit.
    Limited(RequestLimit),
    /// An administrator draft; use the draft methods.
    AdminDraft,
    /// The request's boss week passed; it is now closed as expired and the
    /// requester notice is returned.
    Expired(Box<Notice>),
    /// Approving would change nothing: the request is already in effect.
    /// Nothing was written and its status is unchanged (reject it).
    NoEffect,
}

impl From<DraftError> for RequestError {
    fn from(error: DraftError) -> Self {
        Self::Draft(error)
    }
}

impl From<RequestRefusal> for RequestError {
    fn from(refusal: RequestRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<super::ports::StoreError> for RequestError {
    fn from(error: super::ports::StoreError) -> Self {
        Self::Draft(error.into())
    }
}

impl From<crate::domain::schedule::ScheduleError> for RequestError {
    fn from(error: crate::domain::schedule::ScheduleError) -> Self {
        Self::Draft(error.into())
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::Draft(error) => error.fmt(f),
            Self::Limited(RequestLimit::Pending { max, .. }) => {
                write!(f, "at most {max} requests may wait for a decision")
            }
            Self::Limited(RequestLimit::Rate { max, .. }) => {
                write!(f, "at most {max} requests per day")
            }
            Self::AdminDraft => f.write_str("that is an administrator draft, not a request"),
            Self::Expired(_) => f.write_str("the request's boss week has passed"),
            Self::NoEffect => f.write_str("the request is already in effect"),
        }
    }
}

impl std::error::Error for RequestError {}

pub type RequestResult<T> = Result<T, RequestError>;

/// An approved request: the merge and the requester's notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approved {
    pub merge: MergeOutcome,
    /// Mentions only the requester; returned, not posted.
    pub requester_notice: Notice,
    /// The requester is frozen (approval is still an admin decision).
    pub requester_frozen: bool,
}

/// What approving a request now would do; nothing was written.
#[derive(Clone, Debug)]
pub struct RequestPreview {
    pub version: u64,
    pub status: DraftStatus,
    pub analysis: MergeAnalysis,
    /// The requester may still have it approved (on the current schedule).
    pub requester_authorised: bool,
    pub requester_frozen: bool,
    /// Approving would change nothing (already in effect): the inbox flags
    /// it, and the admin rejects it with that reason. The status never
    /// changes automatically.
    pub no_effect: bool,
    /// Admin-facing: runs a party delta would leave alone rather than
    /// empty (approval still succeeds and records them).
    pub warnings: Vec<MergeWarning>,
}

/// A rejected request and the requester's notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub request: StoredDraft,
    pub requester_notice: Notice,
}

fn require_admin(actor: &Actor) -> RequestResult<()> {
    if matches!(actor, Actor::Admin { .. }) {
        Ok(())
    } else {
        Err(RequestRefusal::NotAdmin.into())
    }
}

fn requester_of(draft: &StoredDraft) -> Option<&str> {
    match &draft.author {
        Actor::Member { id } => Some(id),
        _ => None,
    }
}

fn subject_of(draft: &StoredDraft) -> Option<Subject> {
    draft.subject.as_deref().and_then(Subject::parse)
}

/// The requester notice for a closed request (`None` for an administrator
/// draft).
pub(super) fn decision_notice(
    loaded: &LoadedDraft,
    decision: RequestDecision,
    reason: Option<String>,
    snapshot: &ScheduleSnapshot,
) -> Option<Notice> {
    if loaded.draft.kind != DraftKind::Request {
        return None;
    }
    let requester = requester_of(&loaded.draft)?;
    let channel = home_channel(
        subject_of(&loaded.draft).as_ref(),
        &loaded.draft_ops(),
        snapshot,
    );
    Some(requester_notice(
        &loaded.draft.id,
        requester,
        decision,
        reason,
        channel,
    ))
}

async fn load_request<S: DraftStore>(store: &S, id: &str) -> RequestResult<LoadedDraft> {
    let loaded = store
        .load_draft(id)
        .await?
        .ok_or_else(|| DraftError::UnknownDraft(id.to_owned()))?;
    if loaded.draft.kind != DraftKind::Request {
        return Err(RequestError::AdminDraft);
    }
    Ok(loaded)
}

/// The operations an approval merges: a change request's `UpdateAll`
/// placeholder replaced by the administrator's choices (and the note that
/// records them); other types take no choices.
fn approval_ops(
    loaded: &LoadedDraft,
    choices: Option<FixedEditChoices>,
) -> RequestResult<(RequestType, Vec<DraftOp>, Option<String>)> {
    let kind = loaded
        .draft
        .request_type
        .as_deref()
        .and_then(RequestType::parse)
        .ok_or(RequestRefusal::UnknownSubject)?;
    let mut ops = loaded.draft_ops();
    let note = match (kind, choices) {
        (RequestType::ChangeFixed, None) => return Err(RequestRefusal::ChoicesRequired.into()),
        (RequestType::ChangeFixed, Some(chosen)) => {
            for op in &mut ops {
                if let DraftOp::ApplyFixedEdit { choices, .. } = op {
                    *choices = chosen.clone();
                }
            }
            Some(choices_note(&chosen))
        }
        (_, Some(_)) => return Err(RequestRefusal::ChoicesNotApplicable.into()),
        (_, None) => None,
    };
    Ok((kind, ops, note))
}

/// The request must still await a decision at `version`.
fn require_submitted(loaded: &LoadedDraft, version: u64) -> RequestResult<()> {
    if loaded.draft.status == DraftStatus::Submitted && loaded.draft.version == version {
        Ok(())
    } else {
        Err(stale_of(
            &loaded.draft.id,
            DraftStale::Moved {
                status: loaded.draft.status,
                version: loaded.draft.version,
                merged_seq: loaded.draft.merged_seq,
            },
        )
        .into())
    }
}

fn staged(ops: Vec<DraftOp>, author: &Actor, at: DateTime<Utc>) -> Vec<StagedOp> {
    ops.into_iter()
        .enumerate()
        .map(|(ord, op)| StagedOp {
            ord,
            op,
            author: author.clone(),
            added_at: at,
        })
        .collect()
}

fn replay_failed(rejected: crate::domain::drafts::Rejected) -> RequestError {
    DraftError::ReplayFailed {
        ord: rejected.ord,
        error: rejected.error,
    }
    .into()
}

impl<S: ScheduleStore + DraftStore, I: IdSource, C: Clock> SchedulerService<S, I, C> {
    /// Submit a member request. The requester is authorised on the current
    /// schedule, the operations are dry-run there (a request that could not
    /// apply is refused), the expiry week is derived as for drafts, and the
    /// store counts the member's pending requests and rolling submissions
    /// inside the insert transaction. Idempotent per (member, request id).
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_request(
        &mut self,
        member: &str,
        title: &str,
        spec: RequestSpec,
        request_id: Option<String>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
        gate: &(dyn MemberGate + Sync),
    ) -> RequestResult<StoredDraft> {
        self.check_policy(policy)?;
        let author = Actor::member(member);
        let request_digest = digest("submit_request", &(title, &spec, member));
        // An exact retry replays before anything else is checked (a member
        // frozen since, or a subject changed since, still gets their answer).
        if let Some(request_id) = &request_id
            && let Some((recorded, draft)) = self
                .store
                .recorded_draft_request(&author, request_id)
                .await?
        {
            return if recorded == request_digest {
                Ok(draft)
            } else {
                Err(DraftError::RequestMismatch { draft_id: draft.id }.into())
            };
        }
        if gate.is_frozen(member) {
            return Err(RequestRefusal::Frozen.into());
        }
        check_title(title)?;
        let now = self.clock.now();
        let (snapshot, head) = self.store.snapshot_with_head().await?;
        let kind = spec.request_type();
        let subject = spec.subject();
        authorise(kind, subject.as_ref(), member, &snapshot, directory)?;
        let ops = operations(&spec, member, &snapshot)?;
        let replayed = replay(&snapshot, &ops, policy, directory, now).map_err(replay_failed)?;
        let expires = expires_week(&ops, &replayed.created, &snapshot, &replayed.draft, policy);
        if is_expired(expires, policy, now)? {
            return Err(DraftError::Expired.into());
        }
        let new = NewDraft {
            id: self.ids.new_id(),
            kind: DraftKind::Request,
            title: title.to_owned(),
            author: author.clone(),
            base: head,
            base_revision: snapshot.revision,
            request_type: Some(kind.as_str().to_owned()),
            subject: subject.map(|subject| subject.key()),
            at: now,
            request: request_id.map(|request_id| DraftRequest {
                request_id,
                digest: request_digest,
            }),
            submit: Some(Submission {
                ops: staged(ops, &author, now),
                expires_week: expires,
                limits: DEFAULT_LIMITS,
            }),
        };
        match self.store.create_draft(new).await? {
            DraftCreated::Created(draft) | DraftCreated::Replayed(draft) => Ok(draft),
            DraftCreated::Mismatch { draft_id } => {
                Err(DraftError::RequestMismatch { draft_id }.into())
            }
            DraftCreated::Limited(limit) => Err(RequestError::Limited(limit)),
        }
    }

    /// The requester withdraws their request while it awaits a decision;
    /// requesters never edit (withdraw and resubmit instead).
    pub async fn withdraw_request(
        &mut self,
        member: &str,
        id: &str,
        expected_version: u64,
    ) -> RequestResult<StoredDraft> {
        let loaded = load_request(&self.store, id).await?;
        if requester_of(&loaded.draft) != Some(member) {
            return Err(RequestRefusal::RequesterUnauthorised.into());
        }
        require_submitted(&loaded, expected_version)?;
        self.close(&loaded, Actor::member(member), DraftStatus::Withdrawn, None)
            .await
    }

    /// An administrator replaces the request's operations before approving
    /// (bumping its version); they must still replay on its base, and the
    /// expiry week is re-derived.
    pub async fn edit_request(
        &mut self,
        actor: &Actor,
        id: &str,
        expected_version: u64,
        ops: Vec<DraftOp>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
    ) -> RequestResult<LoadedDraft> {
        self.check_policy(policy)?;
        require_admin(actor)?;
        let now = self.clock.now();
        let loaded = load_request(&self.store, id).await?;
        require_submitted(&loaded, expected_version)?;
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let replayed =
            replay(&flow.base_snapshot, &ops, policy, directory, now).map_err(replay_failed)?;
        let expires = expires_week(
            &ops,
            &replayed.created,
            &flow.base_snapshot,
            &replayed.draft,
            policy,
        );
        let update = DraftUpdate {
            draft_id: id.to_owned(),
            expected_version,
            actor: actor.clone(),
            at: now,
            change: DraftChange::ReplaceOps {
                ops: staged(ops, actor, now),
                event: DraftEventKind::OpEdited,
                ord: 0,
                expires_week: expires,
            },
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(_) => Ok(load_request(&self.store, id).await?),
            DraftWrite::Stale(stale) => Err(stale_of(id, stale).into()),
        }
    }

    /// What approving the request now would do, for an administrator:
    /// the three-way analysis of its operations (with `choices` for a
    /// change request), whether the requester is still authorised, and
    /// whether they are frozen. Nothing is written.
    #[allow(clippy::too_many_arguments)]
    pub async fn preview_request(
        &self,
        actor: &Actor,
        id: &str,
        choices: Option<FixedEditChoices>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
        gate: &(dyn MemberGate + Sync),
    ) -> RequestResult<RequestPreview> {
        self.check_policy(policy)?;
        require_admin(actor)?;
        let now = self.clock.now();
        let loaded = load_request(&self.store, id).await?;
        let (kind, ops, _) = approval_ops(&loaded, choices)?;
        let requester = requester_of(&loaded.draft)
            .ok_or(RequestRefusal::RequesterUnauthorised)?
            .to_owned();
        let flow = upstream(&self.store, &loaded.draft.base).await?;
        let requester_authorised = authorise(
            kind,
            subject_of(&loaded.draft).as_ref(),
            &requester,
            &flow.current,
            directory,
        )
        .is_ok();
        let analysis = analyze_merge(
            &flow.base_snapshot,
            &flow.current,
            &ops,
            policy,
            directory,
            now,
        );
        let warnings = replay(&flow.current, &ops, policy, directory, now)
            .map(|replayed| skipped_runs(&replayed.results))
            .unwrap_or_default();
        Ok(RequestPreview {
            warnings,
            version: loaded.draft.version,
            status: loaded.draft.status,
            no_effect: analysis
                .result_changes
                .as_ref()
                .is_some_and(|changes| changes.is_empty()),
            analysis,
            requester_authorised,
            requester_frozen: gate.is_frozen(&requester),
        })
    }

    /// Approve (merge) the request at the version the administrator
    /// reviewed. The requester is re-authorised on the current schedule at
    /// every merge attempt, conflicts block (no force), and one
    /// `RequestMerge` record is written (`request_id`
    /// `merge:<id>@v<version>`, `refs` the base head). A `change_fixed`
    /// request needs `choices` for the runs it moves; they are recorded in
    /// the `merged` event. A frozen requester does not block approval (an
    /// administrator decides); it is reported.
    #[allow(clippy::too_many_arguments)]
    pub async fn approve_request(
        &mut self,
        actor: &Actor,
        id: &str,
        reviewed_version: u64,
        choices: Option<FixedEditChoices>,
        policy: &SchedulePolicy,
        directory: &(dyn Directory + Sync),
        gate: &(dyn MemberGate + Sync),
    ) -> RequestResult<Approved> {
        self.check_policy(policy)?;
        require_admin(actor)?;
        let now = self.clock.now();
        let request_id = merge_request_id(id, reviewed_version);
        let request_digest = digest("request_merge", &(id, reviewed_version, &choices));
        if let Some(recorded) = self.store.recorded_request(actor, &request_id).await? {
            return Err(
                if recorded.digest.as_deref() == Some(request_digest.as_str()) {
                    DraftError::AlreadyApplied {
                        seq: recorded.committed.seq,
                        revision: recorded.committed.revision,
                    }
                } else {
                    DraftError::IdempotencyMismatch {
                        seq: recorded.committed.seq,
                    }
                }
                .into(),
            );
        }
        let loaded = load_request(&self.store, id).await?;
        let (kind, ops, note) = approval_ops(&loaded, choices)?;
        // Expiry, then the reviewed version, then the requester as things
        // stand now (and again at every merge attempt).
        let snapshot = self.store.load(&Scope::All).await?;
        if is_expired(loaded.draft.scope.expires_week(), policy, now)? {
            return Err(self.expired(&loaded, now, &snapshot).await);
        }
        require_submitted(&loaded, reviewed_version)?;
        let requester = requester_of(&loaded.draft)
            .ok_or(RequestRefusal::RequesterUnauthorised)?
            .to_owned();
        let subject = subject_of(&loaded.draft);
        authorise(kind, subject.as_ref(), &requester, &snapshot, directory)?;
        let still_allowed = |current: &ScheduleSnapshot| {
            authorise(kind, subject.as_ref(), &requester, current, directory).is_ok()
        };
        // Built before committing: nothing after the commit may fail.
        let channel = home_channel(subject.as_ref(), &ops, &snapshot);
        let requester_frozen = gate.is_frozen(&requester);
        let merged = self
            .merge_loaded(
                MergeInput {
                    actor: actor.clone(),
                    surface: Surface::RequestMerge,
                    request_id,
                    request_digest,
                    draft: &loaded.draft,
                    ops,
                    expected_version: reviewed_version,
                    summary: public_summary(kind, subject.as_ref()),
                    note,
                    authorise: Some(&still_allowed),
                    status_at_apply: Default::default(),
                },
                policy,
                directory,
                now,
            )
            .await;
        match merged {
            Ok(merge) => Ok(Approved {
                merge,
                requester_notice: requester_notice(
                    id,
                    &requester,
                    RequestDecision::Approved,
                    None,
                    channel,
                ),
                requester_frozen,
            }),
            // The merge closed it as expired (re-derived on the current
            // schedule): tell the requester.
            Err(DraftError::Expired) => Err(RequestError::Expired(Box::new(requester_notice(
                id,
                &requester,
                RequestDecision::Expired,
                None,
                channel,
            )))),
            Err(DraftError::RequesterUnauthorised) => {
                Err(RequestRefusal::RequesterUnauthorised.into())
            }
            Err(DraftError::NoEffect) => Err(RequestError::NoEffect),
            Err(error) => Err(error.into()),
        }
    }

    /// Reject the request at the reviewed version, with a reason (1..=500
    /// characters, trimmed, no control characters). No schedule record is
    /// written; the draft's `rejected` event keeps it.
    pub async fn reject_request(
        &mut self,
        actor: &Actor,
        id: &str,
        reviewed_version: u64,
        reason: &str,
    ) -> RequestResult<Rejected> {
        require_admin(actor)?;
        let reason = check_reason(reason)?;
        let loaded = load_request(&self.store, id).await?;
        require_submitted(&loaded, reviewed_version)?;
        let requester = requester_of(&loaded.draft)
            .ok_or(RequestRefusal::RequesterUnauthorised)?
            .to_owned();
        // Built before closing: nothing after the close may fail.
        let snapshot = self.store.load(&Scope::All).await?;
        let channel = home_channel(
            subject_of(&loaded.draft).as_ref(),
            &loaded.draft_ops(),
            &snapshot,
        );
        let request = self
            .close(
                &loaded,
                actor.clone(),
                DraftStatus::Rejected,
                Some(reason.clone()),
            )
            .await?;
        Ok(Rejected {
            request,
            requester_notice: requester_notice(
                id,
                &requester,
                RequestDecision::Rejected,
                Some(reason),
                channel,
            ),
        })
    }

    async fn close(
        &mut self,
        loaded: &LoadedDraft,
        actor: Actor,
        status: DraftStatus,
        reason: Option<String>,
    ) -> RequestResult<StoredDraft> {
        let update = DraftUpdate {
            draft_id: loaded.draft.id.clone(),
            expected_version: loaded.draft.version,
            actor,
            at: self.clock.now(),
            change: DraftChange::Close { status, reason },
        };
        match self.store.update_draft(update).await? {
            DraftWrite::Written(draft) => Ok(draft),
            DraftWrite::Stale(stale) => Err(stale_of(&loaded.draft.id, stale).into()),
        }
    }

    /// Close an expired request (as the tick's system actor) and return
    /// its requester notice.
    async fn expired(
        &mut self,
        loaded: &LoadedDraft,
        now: DateTime<Utc>,
        snapshot: &ScheduleSnapshot,
    ) -> RequestError {
        let update = DraftUpdate {
            draft_id: loaded.draft.id.clone(),
            expected_version: loaded.draft.version,
            actor: Actor::system(super::drafts::EXPIRY_ACTOR),
            at: now,
            change: DraftChange::Close {
                status: DraftStatus::Expired,
                reason: None,
            },
        };
        match self.store.update_draft(update).await {
            Ok(DraftWrite::Written(_)) => {
                match decision_notice(loaded, RequestDecision::Expired, None, snapshot) {
                    Some(notice) => RequestError::Expired(Box::new(notice)),
                    None => DraftError::Expired.into(),
                }
            }
            Ok(DraftWrite::Stale(stale)) => stale_of(&loaded.draft.id, stale).into(),
            Err(error) => error.into(),
        }
    }
}
