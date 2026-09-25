//! What drafts need from a store: draft rows, their staged operations, an
//! append-only event log, and the merge commit that closes a draft in the
//! same transaction as the schedule change it records.

use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};

use super::op::DraftOp;
use crate::domain::history::{Actor, ChangeMeta, ChangeRecord, ChangeRef};
use crate::domain::schedule::{ChangeSet, ScheduleSnapshot};
use crate::domain::scheduler::{Committed, StoreError};

/// Who may merge a draft: an administrator's own draft, or (S3) a member's
/// request an administrator approves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DraftKind {
    Admin,
    Request,
}

impl DraftKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Request => "request",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Admin, Self::Request]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

/// A draft's lifecycle. Only `Open` (and, for requests, `Submitted`) drafts
/// change; every other status is final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DraftStatus {
    Open,
    Submitted,
    Merged,
    Discarded,
    Rejected,
    Withdrawn,
    Expired,
}

impl DraftStatus {
    pub const ALL: &[Self] = &[
        Self::Open,
        Self::Submitted,
        Self::Merged,
        Self::Discarded,
        Self::Rejected,
        Self::Withdrawn,
        Self::Expired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Submitted => "submitted",
            Self::Merged => "merged",
            Self::Discarded => "discarded",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
            Self::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.as_str() == value)
    }

    /// Still accepting changes (and expiry).
    pub fn is_live(self) -> bool {
        matches!(self, Self::Open | Self::Submitted)
    }
}

/// What a draft changes, derived from its staged operations by the service
/// (never caller-supplied): one boss week (it expires once that week has
/// passed), or weekly timings only (it never expires by week).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftScope {
    Week(DateTime<Utc>),
    Weekly,
}

impl DraftScope {
    /// The stored `expires_week`.
    pub fn expires_week(self) -> Option<DateTime<Utc>> {
        match self {
            Self::Week(week) => Some(week),
            Self::Weekly => None,
        }
    }
}

/// A stored draft (without its operations).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDraft {
    pub id: String,
    pub kind: DraftKind,
    pub title: String,
    pub author: Actor,
    /// The history head the operations were drafted on.
    pub base: ChangeRef,
    /// The store revision read with `base`.
    pub base_revision: u64,
    /// Bumped by every change to the operations or the base.
    pub version: u64,
    pub status: DraftStatus,
    /// Request type and subject (S3); `None` for administrator drafts.
    pub request_type: Option<String>,
    pub subject: Option<String>,
    /// The merge record, once merged.
    pub merged_seq: Option<u64>,
    pub closed_by: Option<Actor>,
    pub close_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub scope: DraftScope,
}

/// One staged operation at position `ord`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedOp {
    pub ord: usize,
    pub op: DraftOp,
    pub author: Actor,
    pub added_at: DateTime<Utc>,
}

/// A draft with its operations in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedDraft {
    pub draft: StoredDraft,
    pub ops: Vec<StagedOp>,
}

impl LoadedDraft {
    pub fn draft_ops(&self) -> Vec<DraftOp> {
        self.ops.iter().map(|staged| staged.op.clone()).collect()
    }
}

/// What happened to a draft; the log is append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DraftEventKind {
    Created,
    OpAdded,
    OpEdited,
    OpRemoved,
    Rebased,
    Merged,
    Discarded,
    Expired,
    Submitted,
    Rejected,
    Withdrawn,
}

impl DraftEventKind {
    pub const ALL: &[Self] = &[
        Self::Created,
        Self::OpAdded,
        Self::OpEdited,
        Self::OpRemoved,
        Self::Rebased,
        Self::Merged,
        Self::Discarded,
        Self::Expired,
        Self::Submitted,
        Self::Rejected,
        Self::Withdrawn,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::OpAdded => "op_added",
            Self::OpEdited => "op_edited",
            Self::OpRemoved => "op_removed",
            Self::Rebased => "rebased",
            Self::Merged => "merged",
            Self::Discarded => "discarded",
            Self::Expired => "expired",
            Self::Submitted => "submitted",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == value)
    }
}

/// One logged event, stamped with the draft version it produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftEvent {
    pub draft_id: String,
    pub version: u64,
    pub kind: DraftEventKind,
    pub actor: Actor,
    pub at: DateTime<Utc>,
    /// Kind-specific facts (an operation position, a merge seq, a reason).
    pub detail: Option<String>,
}

/// A request id for creating a draft (idempotency).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftRequest {
    pub request_id: String,
    pub digest: String,
}

/// Per-member request limits, counted by the store inside the insert
/// transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestLimits {
    /// Submitted (undecided) requests one member may have at once.
    pub max_pending: u64,
    /// Requests one member may submit per rolling `window`.
    pub max_per_window: u64,
    pub window: chrono::TimeDelta,
}

/// A member request submitted at creation: it is stored `submitted` with
/// its operations and derived expiry, in the same transaction as the limit
/// counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub ops: Vec<StagedOp>,
    pub expires_week: Option<DateTime<Utc>>,
    pub limits: RequestLimits,
}

/// Which request limit refused a submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestLimit {
    Pending { count: u64, max: u64 },
    Rate { count: u64, max: u64 },
}

/// A draft to create at `base`. The expiry scope is derived once
/// operations are staged, so an administrator draft carries none; a member
/// request arrives with its operations (`submit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDraft {
    pub id: String,
    pub kind: DraftKind,
    pub title: String,
    pub author: Actor,
    pub base: ChangeRef,
    pub base_revision: u64,
    pub request_type: Option<String>,
    pub subject: Option<String>,
    pub at: DateTime<Utc>,
    pub request: Option<DraftRequest>,
    pub submit: Option<Submission>,
}

/// The outcome of [`DraftStore::create_draft`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftCreated {
    Created(StoredDraft),
    /// The author's request id was already used for the same request; the
    /// draft it created is returned and nothing was written.
    Replayed(StoredDraft),
    /// The request id was already used for a different request.
    Mismatch {
        draft_id: String,
    },
    /// A member request over a limit; nothing was written.
    Limited(RequestLimit),
}

/// One change to a live draft, bumping its version (except `Close`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftChange {
    /// Replace every staged operation (renumbered in order); `event` says
    /// which edit this was and `ord` where. `expires_week` is the freshly
    /// derived scope, stored in the same write.
    ReplaceOps {
        ops: Vec<StagedOp>,
        event: DraftEventKind,
        ord: usize,
        expires_week: Option<DateTime<Utc>>,
    },
    /// Re-point the draft at a newer head, with its freshly derived scope.
    Rebase {
        base: ChangeRef,
        base_revision: u64,
        expires_week: Option<DateTime<Utc>>,
    },
    /// Close it for good (discarded, rejected, withdrawn).
    Close {
        status: DraftStatus,
        reason: Option<String>,
    },
}

/// A change to the draft `draft_id`, planned from `expected_version`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftUpdate {
    pub draft_id: String,
    pub expected_version: u64,
    pub actor: Actor,
    pub at: DateTime<Utc>,
    pub change: DraftChange,
}

/// Why a draft write or merge did not apply: the draft as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftStale {
    Missing,
    /// No longer live, or at another version.
    Moved {
        status: DraftStatus,
        version: u64,
        merged_seq: Option<u64>,
    },
}

/// The outcome of [`DraftStore::update_draft`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant, reason = "a write returns the whole draft")]
pub enum DraftWrite {
    Written(StoredDraft),
    Stale(DraftStale),
}

/// The outcome of [`DraftStore::commit_merge`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeCommit {
    /// Written as this record; `replayed` when the merger's request id was
    /// already recorded for the same merge (nothing written).
    Committed(Committed),
    /// The draft was not live at the expected version; nothing written.
    Stale(DraftStale),
}

/// Draft persistence. Every write is atomic; the event log and the
/// operations of a closed draft never change.
///
/// [`DraftStore::snapshot_with_head`] reads the whole schedule and the
/// history head in one read transaction, so the head covers every record
/// the snapshot reflects.
///
/// [`DraftStore::commit_merge`] is one write transaction: the merger's
/// request id replay check (as [`ScheduleStore::commit`]), then the draft
/// (live, at `expected_version`, else [`MergeCommit::Stale`]), then the
/// store revision ([`StoreError::Conflict`]), then the rows, the record and
/// its blame index, the draft's `merged` status and `merged_seq`, and its
/// `merged` event. An empty change set is refused
/// ([`StoreError::Constraint`]).
///
/// [`ScheduleStore::commit`]: crate::domain::scheduler::ScheduleStore::commit
pub trait DraftStore {
    fn snapshot_with_head(
        &self,
    ) -> impl Future<Output = Result<(ScheduleSnapshot, ChangeRef), StoreError>> + Send;

    /// Every record after `base.seq`, in order (all pages). Callers rewind
    /// through [`check_records_after`](crate::domain::history::check_records_after)
    /// (as [`rewind`](crate::domain::history::rewind) does) and refuse a
    /// [`HistoryGap`](crate::domain::history::HistoryGap).
    fn records_after(
        &self,
        base: &ChangeRef,
    ) -> impl Future<Output = Result<Vec<ChangeRecord>, StoreError>> + Send;

    fn create_draft(
        &self,
        new: NewDraft,
    ) -> impl Future<Output = Result<DraftCreated, StoreError>> + Send;

    fn load_draft(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<LoadedDraft>, StoreError>> + Send;

    /// The draft `author`'s create `request_id` made, with the request's
    /// digest (an idempotency pre-check; `create_draft` checks again).
    fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<(String, StoredDraft)>, StoreError>> + Send;

    /// Drafts in creation order, all or with one status.
    fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> impl Future<Output = Result<Vec<StoredDraft>, StoreError>> + Send;

    /// A draft's events in order.
    fn draft_events(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Vec<DraftEvent>, StoreError>> + Send;

    fn update_draft(
        &self,
        update: DraftUpdate,
    ) -> impl Future<Output = Result<DraftWrite, StoreError>> + Send;

    /// `note` follows the record `seq` in the `merged` event's detail
    /// (e.g. the per-run choices an approval applied).
    fn commit_merge(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> impl Future<Output = Result<MergeCommit, StoreError>> + Send;

    /// Expire every live draft whose boss week starts before `week`, as
    /// `actor` at `at`; returns their ids.
    fn expire_drafts(
        &self,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
        actor: &Actor,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send;
}

impl<T: DraftStore + Sync> DraftStore for &T {
    fn snapshot_with_head(
        &self,
    ) -> impl Future<Output = Result<(ScheduleSnapshot, ChangeRef), StoreError>> + Send {
        (**self).snapshot_with_head()
    }

    fn records_after(
        &self,
        base: &ChangeRef,
    ) -> impl Future<Output = Result<Vec<ChangeRecord>, StoreError>> + Send {
        (**self).records_after(base)
    }

    fn create_draft(
        &self,
        new: NewDraft,
    ) -> impl Future<Output = Result<DraftCreated, StoreError>> + Send {
        (**self).create_draft(new)
    }

    fn load_draft(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<LoadedDraft>, StoreError>> + Send {
        (**self).load_draft(id)
    }

    fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> impl Future<Output = Result<Vec<StoredDraft>, StoreError>> + Send {
        (**self).list_drafts(status)
    }

    fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<(String, StoredDraft)>, StoreError>> + Send {
        (**self).recorded_draft_request(author, request_id)
    }

    fn draft_events(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Vec<DraftEvent>, StoreError>> + Send {
        (**self).draft_events(id)
    }

    fn update_draft(
        &self,
        update: DraftUpdate,
    ) -> impl Future<Output = Result<DraftWrite, StoreError>> + Send {
        (**self).update_draft(update)
    }

    fn commit_merge(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> impl Future<Output = Result<MergeCommit, StoreError>> + Send {
        (**self).commit_merge(
            expected_revision,
            changes,
            meta,
            draft_id,
            expected_version,
            note,
        )
    }

    fn expire_drafts(
        &self,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
        actor: &Actor,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send {
        (**self).expire_drafts(week, at, actor)
    }
}

impl fmt::Display for DraftStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
