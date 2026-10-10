//! Replaying a draft's operations on a snapshot, writing nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::op::{DraftOp, ReplayError, Target, resolve};
use crate::domain::completion::RunEnds;
use crate::domain::ids::IdGenerator;
use crate::domain::members::Directory;
use crate::domain::schedule::{
    Draft, FixedEditChoices, Notice, OpResult, ScheduleError, SchedulePolicy, ScheduleSnapshot,
    apply_op, preview_fixed_edit, utc_instant,
};

pub const PREVIEW_ID_PREFIX: &str = "preview-";

/// Deterministic ids for replays (`preview-1`, `preview-2`, ...). The owner
/// log separately records every id drawn and which operation drew it; drawn
/// ids never collide with stored UUIDs.
#[derive(Clone, Debug, Default)]
pub struct PreviewIds {
    next: u64,
    ord: usize,
    owners: BTreeMap<String, (usize, u64)>,
}

impl PreviewIds {
    /// The operation position and draw number of a preview id.
    pub fn owner(&self, id: &str) -> Option<(usize, u64)> {
        self.owners.get(id).copied()
    }

    /// Record that the current operation drew `id` (the next draw number).
    pub(super) fn note(&mut self, id: String) {
        self.next += 1;
        self.owners.insert(id, (self.ord, self.next));
    }
}

impl IdGenerator for PreviewIds {
    fn new_id(&mut self) -> String {
        self.next += 1;
        let id = format!("{PREVIEW_ID_PREFIX}{}", self.next);
        self.owners.insert(id.clone(), (self.ord, self.next));
        id
    }
}

/// What staging a `DraftOp` must check before it is accepted (and what
/// replay re-checks): every `Target` is well-formed, and the service's own
/// checks will run against the snapshot it stages on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StagedError {
    /// A target starting with [`PREVIEW_ID_PREFIX`]: a preview id is not a
    /// stored row.
    PreviewTarget,
    /// A staged run may only join a weekly timing that exists now; the plain
    /// service mutation keeps v4's unchecked insert.
    MissingTiming(String),
    /// A staged run's `week_start` must be its slot's boss week.
    WeekMismatch {
        week_start: DateTime<Utc>,
        slot_week: DateTime<Utc>,
    },
    Schedule(ScheduleError),
}

impl std::fmt::Display for StagedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreviewTarget => write!(f, "a preview id is not a stored row"),
            Self::MissingTiming(id) => write!(f, "no fixed run `{id}`"),
            Self::WeekMismatch {
                week_start,
                slot_week,
            } => write!(
                f,
                "the run's week {week_start} is not its slot's boss week {slot_week}"
            ),
            Self::Schedule(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StagedError {}

impl From<StagedError> for ReplayError {
    fn from(error: StagedError) -> Self {
        match error {
            StagedError::Schedule(error) => Self::Schedule(error),
            StagedError::MissingTiming(id) => Self::Schedule(ScheduleError::UnknownFixedRun(id)),
            StagedError::PreviewTarget => Self::UnresolvedTarget(usize::MAX),
            StagedError::WeekMismatch {
                week_start,
                slot_week,
            } => Self::WeekMismatch {
                week_start,
                slot_week,
            },
        }
    }
}

/// Reject a target naming a preview id, then try resolving it. A reorder or
/// edit can also leave `Target::Created(ord)` pointing at an operation that
/// created nothing; that too is [`StagedError::PreviewTarget`], standing for
/// a `ReplayError::UnresolvedTarget` (see the `From` impl).
pub fn resolve_staged(target: &Target, created: &[Option<String>]) -> Result<String, StagedError> {
    if matches!(target, Target::Existing(id) if id.starts_with(PREVIEW_ID_PREFIX)) {
        return Err(StagedError::PreviewTarget);
    }
    resolve(target, created).map_err(|_| StagedError::PreviewTarget)
}

/// Staging-time checks for one operation against `draft`, resolved through
/// `created`. Draft checks validate what the service leaves unchecked
/// (preview ids, run timings); everything else (`ReplayError::Schedule`)
/// comes from the same replay the merge runs.
///
/// # Errors
/// [`StagedError`]; the draft may be partly changed and must be discarded.
pub fn check_staged(
    draft: &Draft,
    created: &[Option<String>],
    op: &DraftOp,
    policy: &SchedulePolicy,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> Result<(), StagedError> {
    let targets: Vec<&Target> = match op {
        DraftOp::AddFixedRun(_) => Vec::new(),
        DraftOp::ApplyFixedEdit { fixed, .. } | DraftOp::FixedParticipants { fixed, .. } => {
            vec![fixed]
        }
        DraftOp::RetireFixedRun { fixed, .. } => vec![fixed],
        DraftOp::CreateRun { fixed, .. } => fixed.iter().collect(),
        DraftOp::AmendRun { run, .. } => vec![run],
        DraftOp::SetStatus { run, .. } => vec![run],
        DraftOp::SwapParticipants { run, .. } => vec![run],
        DraftOp::SetRsvp { run, .. } => vec![run],
        DraftOp::ResetToFixed { run }
        | DraftOp::SetRunBosses { run, .. }
        | DraftOp::EnsureReminders { run }
        | DraftOp::RecountRun { run }
        | DraftOp::ReviveRun { run } => vec![run],
    };
    for target in targets {
        resolve_staged(target, created)?;
    }
    if let DraftOp::CreateRun {
        fixed: Some(fixed), ..
    } = op
    {
        let fixed_id = resolve_staged(fixed, created)?;
        if draft.fixed_run(&fixed_id).is_none() {
            return Err(StagedError::MissingTiming(fixed_id));
        }
    }
    if let DraftOp::CreateRun {
        week_start,
        datetime,
        ..
    } = op
    {
        let slot_week = policy
            .week_of(datetime)
            .map_err(ScheduleError::from)
            .and_then(|start| utc_instant(&start).map_err(ScheduleError::from))
            .map_err(StagedError::Schedule)?;
        if *week_start != slot_week {
            return Err(StagedError::WeekMismatch {
                week_start: *week_start,
                slot_week,
            });
        }
    }
    // The service's own checks (roster, channels, choices...) against this
    // snapshot; a `PreviewIds` trial run writes nothing but the inputs.
    let mut trial = draft.clone();
    let mut ids = PreviewIds::default();
    let op_value = op
        .to_op(created, policy, directory)
        .map_err(|error| match error {
            ReplayError::Schedule(error) => StagedError::Schedule(error),
            _ => StagedError::PreviewTarget,
        })?;
    apply_op(&mut trial, &mut ids, &op_value, now).map_err(StagedError::Schedule)?;
    Ok(())
}

/// A successful replay.
#[derive(Clone, Debug)]
pub struct Replay {
    pub draft: Draft,
    pub ids: PreviewIds,
    /// Per operation: the row it created (targets of [`Target::Created`](super::Target)).
    pub created: Vec<Option<String>>,
    pub results: Vec<OpResult>,
    /// Per operation: for a weekly-timing edit with per-run choices, the
    /// amended runs it would have listed right before it applied.
    pub amended: Vec<Option<Vec<String>>>,
    /// Every notice, in operation order; listed, never posted.
    pub notices: Vec<Notice>,
}

/// The operation at `ord` could not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub ord: usize,
    pub error: ReplayError,
}

/// Replay `ops` in order on `snapshot`; `ends` freezes runs past their end
/// as the scheduler does (`None`: v4 rules).
///
/// # Errors
/// [`Rejected`] for the first operation that fails; the partial result is
/// discarded.
pub fn replay(
    snapshot: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> Result<Replay, Rejected> {
    let mut preview = PreviewGen::default();
    match run_all(snapshot, ops, policy, ends, directory, now, &mut preview) {
        (replay, None) => Ok(replay),
        (_, Some(rejected)) => Err(rejected),
    }
}

/// [`replay`] with real row ids from `ids` (the merge commit): the owner log
/// still names which operation drew each id, so the result compares
/// field-by-field with the preview.
///
/// # Errors
/// As [`replay`].
pub fn replay_real(
    snapshot: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
    ids: &mut impl IdGenerator,
) -> Result<Replay, Rejected> {
    match run_all(snapshot, ops, policy, ends, directory, now, ids) {
        (replay, None) => Ok(replay),
        (_, Some(rejected)) => Err(rejected),
    }
}

/// [`replay`] that keeps the operations applied before a rejection; the
/// rejected one's amended-run listing is kept when it was taken.
pub(super) fn replay_partial(
    snapshot: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> (Replay, Option<Rejected>) {
    let mut preview = PreviewGen::default();
    run_all(snapshot, ops, policy, ends, directory, now, &mut preview)
}

fn run_all<G: IdGenerator>(
    snapshot: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
    ids: &mut G,
) -> (Replay, Option<Rejected>) {
    let mut body = Body {
        draft: Draft::new(snapshot.clone())
            .with_attendance(policy.attendance)
            .with_run_ends(ends.cloned()),
        created: Vec::with_capacity(ops.len()),
        results: Vec::with_capacity(ops.len()),
        amended: Vec::with_capacity(ops.len()),
        notices: Vec::new(),
    };
    let mut log = PreviewIds::default();
    for (ord, op) in ops.iter().enumerate() {
        log.ord = ord;
        let mut draws = Recorder {
            inner: &mut *ids,
            log: &mut log,
        };
        if let Err(error) = replay_one(&mut body, op, policy, directory, now, &mut draws) {
            return (body.finish(log), Some(Rejected { ord, error }));
        }
    }
    (body.finish(log), None)
}

/// The mutable replay state; the owner log travels beside it so every drawn
/// id is recorded, including rows operations create indirectly (a
/// weekly-timing edit materialises missing runs).
struct Body {
    draft: Draft,
    created: Vec<Option<String>>,
    results: Vec<OpResult>,
    amended: Vec<Option<Vec<String>>>,
    notices: Vec<Notice>,
}

impl Body {
    fn finish(self, ids: PreviewIds) -> Replay {
        Replay {
            draft: self.draft,
            ids,
            created: self.created,
            results: self.results,
            amended: self.amended,
            notices: self.notices,
        }
    }
}

/// An id source that logs every draw (operation position and draw number)
/// into the replay's owner log.
struct Recorder<'a, I: IdGenerator + ?Sized> {
    inner: &'a mut I,
    log: &'a mut PreviewIds,
}

impl<I: IdGenerator + ?Sized> IdGenerator for Recorder<'_, I> {
    fn new_id(&mut self) -> String {
        let id = self.inner.new_id();
        self.log.note(id.clone());
        id
    }
}

/// Preview ids for [`replay`]; the log separately records the owners.
#[derive(Default)]
struct PreviewGen(u64);

impl IdGenerator for PreviewGen {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("{PREVIEW_ID_PREFIX}{}", self.0)
    }
}

fn replay_one<G: IdGenerator>(
    body: &mut Body,
    op: &DraftOp,
    policy: &SchedulePolicy,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
    ids: &mut G,
) -> Result<(), ReplayError> {
    let amended = match op {
        DraftOp::ApplyFixedEdit {
            fixed,
            edit,
            choices: FixedEditChoices::PerRun(_),
        } => {
            let fixed_id = resolve_staged(fixed, &body.created)?;
            let listed = preview_fixed_edit(&body.draft, &fixed_id, edit, policy, now)?;
            Some(listed.into_iter().map(|run| run.run_id).collect())
        }
        _ => None,
    };
    body.amended.push(amended);
    check_staged(&body.draft, &body.created, op, policy, directory, now)?;
    let op_value = op.to_op(&body.created, policy, directory)?;
    let applied = apply_op(&mut body.draft, ids, &op_value, now)?;
    body.created.push(match (op, &applied.value) {
        (DraftOp::AddFixedRun(_) | DraftOp::CreateRun { .. }, OpResult::Created(id)) => {
            Some(id.clone())
        }
        _ => None,
    });
    body.results.push(applied.value);
    body.notices.extend(applied.notices);
    Ok(())
}
