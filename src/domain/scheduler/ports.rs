//! What the scheduler needs from the outside: a store, an id source and a clock.

use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};

use crate::domain::attendance::PastRun;
use crate::domain::history::{Actor, ChangeMeta, HistoryGap, PreconditionError, StaleField};
use crate::domain::schedule::{ChangeSet, ScheduleSnapshot};

pub use crate::domain::ids::IdGenerator as IdSource;

/// The single aware clock every scheduler decision reads.
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
}

/// Which runs a snapshot holds. Every weekly timing is always included, and
/// each loaded run brings all of its reminders and RSVPs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Runs whose boss week starts at one of these instants.
    Weeks(Vec<DateTime<Utc>>),
    Run(String),
    /// The run owning this reminder (and that run id's reminders even if the
    /// run row is absent).
    Reminder(String),
}

/// Persistence failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// A commit would break a table invariant; nothing was written.
    Constraint(String),
    /// Another commit landed after the snapshot was read; nothing was written.
    Conflict { expected: u64, found: u64 },
    /// The actor's request id was already recorded (as change `seq`) for a
    /// different request; nothing was written.
    IdempotencyMismatch { seq: u64 },
    /// A stored change record does not parse or match its hash: the history
    /// after the read base cannot be trusted.
    HistoryGap(HistoryGap),
    /// A declared field changed since the caller saw it (or its row is
    /// gone); nothing was written.
    StaleEdit(Vec<StaleField>),
    /// The commit's preconditions are malformed; nothing was written.
    Precondition(PreconditionError),
    /// The backend failed; nothing is known to have been written.
    Backend(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constraint(detail) => write!(f, "schedule constraint violated: {detail}"),
            Self::IdempotencyMismatch { seq } => write!(
                f,
                "request id already used by change {seq} for a different request"
            ),
            Self::Conflict { expected, found } => write!(
                f,
                "schedule changed concurrently: planned at revision {expected}, store is at {found}"
            ),
            Self::HistoryGap(gap) => gap.fmt(f),
            Self::StaleEdit(fields) => {
                write!(f, "{} field(s) changed since they were read", fields.len())
            }
            Self::Precondition(error) => error.fmt(f),
            Self::Backend(detail) => write!(f, "schedule store failed: {detail}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The record a commit appended, or (`replayed`) the earlier record of the
/// same actor's request id, in which case nothing was applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Committed {
    pub seq: u64,
    pub revision: u64,
    pub replayed: bool,
}

/// A recorded request id: its change and the digest of the request that
/// made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRequest {
    pub committed: Committed,
    pub digest: Option<String>,
}

/// Scheduler persistence: scoped snapshot reads and atomic change-set commits.
///
/// Snapshot order: weekly timings by `(weekday, time, id)`, runs by
/// `(datetime, id)`, reminders by `(run_id, fire_at, kind)`, RSVPs by
/// `(run_id, user_id)`. A commit applies every change or none; the resulting
/// state must keep reminders unique per `(run_id, kind)`, runs unique per
/// `(fixed_run_id, week_start)` when linked to a timing, and every reminder
/// and RSVP attached to an existing run.
///
/// Every non-empty commit advances the store-wide revision by one and, in the
/// same transaction, appends a hash-chained [`ChangeRecord`] built from
/// `meta` and the touched rows' before/after values (the store reads the
/// `before` values itself). A commit is refused with [`StoreError::Conflict`]
/// unless `expected_revision` is still current, even when `changes` is empty;
/// refused and empty commits record nothing (`Ok(None)` when empty).
///
/// Preconditions: after the request-id replay check and before the revision
/// check, every `meta.expect` field is compared with the blame index in the
/// same transaction ([`StoreError::StaleEdit`] lists every stale one) and
/// every override must name a recorded change with its hash
/// ([`StoreError::Precondition`]). An empty change set is still checked.
///
/// Idempotency: when `meta` carries a request id the store already recorded
/// for the same actor, the commit applies nothing: with the same request
/// digest it returns that earlier record as [`Committed`] with `replayed`,
/// whatever the revision; with another digest it fails with
/// [`StoreError::IdempotencyMismatch`].
///
/// [`ChangeRecord`]: crate::domain::history::ChangeRecord
pub trait ScheduleStore {
    fn load(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<ScheduleSnapshot, StoreError>> + Send;

    /// The change recorded for `actor`'s `request_id`, if any.
    fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<RecordedRequest>, StoreError>> + Send;

    fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send;
}

/// A shared store: the API's single writer and its readers hold one `Arc`.
impl<T: ScheduleStore + Send + Sync> ScheduleStore for std::sync::Arc<T> {
    fn load(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<ScheduleSnapshot, StoreError>> + Send {
        (**self).load(scope)
    }

    fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<RecordedRequest>, StoreError>> + Send {
        (**self).recorded_request(actor, request_id)
    }

    fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send {
        (**self).commit(expected_revision, changes, meta)
    }
}

/// v5 attendance reads: one member's recorded attendance.
pub trait AttendanceHistory {
    /// `member`'s recorded attendance on done runs with their explicit
    /// answer, newest first (`(datetime, run id)` descending), at most
    /// `per_timing` runs per weekly timing (one-off runs share one group).
    fn member_history(
        &self,
        member: &str,
        per_timing: usize,
    ) -> impl Future<Output = Result<Vec<PastRun>, StoreError>> + Send;
}

impl<T: AttendanceHistory + Sync> AttendanceHistory for &T {
    fn member_history(
        &self,
        member: &str,
        per_timing: usize,
    ) -> impl Future<Output = Result<Vec<PastRun>, StoreError>> + Send {
        (**self).member_history(member, per_timing)
    }
}

/// A shared store (several services over one store) is a store.
impl<T: ScheduleStore + Sync> ScheduleStore for &T {
    fn load(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<ScheduleSnapshot, StoreError>> + Send {
        (**self).load(scope)
    }

    fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<RecordedRequest>, StoreError>> + Send {
        (**self).recorded_request(actor, request_id)
    }

    fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send {
        (**self).commit(expected_revision, changes, meta)
    }
}
