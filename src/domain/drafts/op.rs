//! Draft operations: the attributed mutations a draft stages, stored as
//! values and replayed later through [`apply_op`](crate::domain::schedule::apply_op).

use chrono::{DateTime, Utc};

use crate::domain::members::Directory;
use crate::domain::schedule::{
    FixedEdit, FixedEditChoices, FixedEditRequest, NewFixedRun, NewRun, Op, RsvpSource, RsvpState,
    RunSource, RunStatus, ScheduleError, SchedulePolicy, StatusChange, WeekStart,
};

/// Which row an operation acts on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// A row that exists in the schedule.
    Existing(String),
    /// The row created by the draft's operation at this position.
    Created(usize),
}

/// One staged mutation. Policies and the member directory are supplied at
/// replay time, never stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftOp {
    AddFixedRun(NewFixedRun),
    ApplyFixedEdit {
        fixed: Target,
        edit: FixedEdit,
        choices: FixedEditChoices,
    },
    /// Change a weekly timing's party by delta: `remove` then `add` apply
    /// to the timing's party as it is at replay, and the result is pushed to
    /// its live runs as a party edit (`UpdateAll`). Two drafts adding
    /// different members therefore both apply.
    FixedParticipants {
        fixed: Target,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// Delete a weekly timing and cancel its live runs in these boss weeks.
    RetireFixedRun {
        fixed: Target,
        weeks: Vec<DateTime<Utc>>,
    },
    CreateRun {
        fixed: Option<Target>,
        channel_id: Option<String>,
        week_start: DateTime<Utc>,
        datetime: DateTime<Utc>,
        bosses: Vec<String>,
        participants: Vec<String>,
        status: RunStatus,
        source: RunSource,
    },
    AmendRun {
        run: Target,
        to: DateTime<Utc>,
    },
    SetStatus {
        run: Target,
        change: StatusChange,
    },
    SwapParticipants {
        run: Target,
        remove: Vec<String>,
        add: Vec<String>,
        via_portal: bool,
    },
    SetRsvp {
        run: Target,
        user_id: String,
        state: RsvpState,
        source: RsvpSource,
    },
    ResetToFixed {
        run: Target,
    },
    /// Replace a run's bosses, rebuilding its reminders (a proposal split).
    SetRunBosses {
        run: Target,
        bosses: Vec<String>,
    },
    /// Add the reminders a run lacks (`rebuild = false`): runs a proposal
    /// creates get them at once, as v4's `add`/`split` did.
    EnsureReminders {
        run: Target,
    },
    /// Re-derive a run's status from its answers (proposed RSVPs).
    RecountRun {
        run: Target,
    },
    /// Put a cancelled or otot run back to `planned` (a proposal's move).
    ReviveRun {
        run: Target,
    },
}

/// Why an operation could not be replayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The scheduling rules refused it, or the operation names a preview id
    /// (not a stored row).
    Schedule(ScheduleError),
    /// It names a row created by an operation that created nothing.
    UnresolvedTarget(usize),
    /// A staged run's `week_start` is not the boss week of its slot.
    WeekMismatch {
        week_start: DateTime<Utc>,
        slot_week: DateTime<Utc>,
    },
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schedule(error) => error.fmt(f),
            Self::UnresolvedTarget(ord) => write!(f, "operation {ord} created nothing"),
            Self::WeekMismatch {
                week_start,
                slot_week,
            } => write!(
                f,
                "the run's week {week_start} is not its slot's boss week {slot_week}"
            ),
        }
    }
}

impl From<ScheduleError> for ReplayError {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

/// Resolve a target against the ids the draft's earlier operations created.
pub fn resolve(target: &Target, created: &[Option<String>]) -> Result<String, ReplayError> {
    match target {
        Target::Existing(id) => Ok(id.clone()),
        Target::Created(ord) => created
            .get(*ord)
            .cloned()
            .flatten()
            .ok_or(ReplayError::UnresolvedTarget(*ord)),
    }
}

impl DraftOp {
    /// The schedule operation this stands for, with created targets resolved.
    ///
    /// # Errors
    /// [`ReplayError::UnresolvedTarget`], or a week outside the representable
    /// years.
    pub fn to_op<'a>(
        &self,
        created: &[Option<String>],
        policy: &'a SchedulePolicy,
        directory: &'a (dyn Directory + Sync),
    ) -> Result<Op<'a>, ReplayError> {
        Ok(match self {
            Self::AddFixedRun(new) => Op::AddFixedRun(new.clone()),
            Self::FixedParticipants { fixed, add, remove } => Op::FixedParticipants {
                fixed_id: resolve(fixed, created)?,
                add: add.clone(),
                remove: remove.clone(),
                directory,
                policy,
            },
            Self::ApplyFixedEdit {
                fixed,
                edit,
                choices,
            } => Op::ApplyFixedEdit {
                request: FixedEditRequest {
                    fixed_id: resolve(fixed, created)?,
                    edit: edit.clone(),
                    choices: choices.clone(),
                },
                directory,
                policy,
            },
            Self::RetireFixedRun { fixed, weeks } => Op::RetireFixedRun {
                fixed_id: resolve(fixed, created)?,
                week_starts: weeks
                    .iter()
                    .map(|week| WeekStart::new(week, policy.zone()))
                    .collect::<Result<_, _>>()
                    .map_err(ScheduleError::from)?,
                policy: &policy.reminders,
            },
            Self::CreateRun {
                fixed,
                channel_id,
                week_start,
                datetime,
                bosses,
                participants,
                status,
                source,
            } => Op::CreateRun(NewRun {
                fixed_run_id: fixed
                    .as_ref()
                    .map(|fixed| resolve(fixed, created))
                    .transpose()?,
                channel_id: channel_id.clone(),
                week_start: *week_start,
                datetime: *datetime,
                bosses: bosses.clone(),
                participants: participants.clone(),
                status: *status,
                source: *source,
            }),
            Self::AmendRun { run, to } => Op::AmendRun {
                run_id: resolve(run, created)?,
                to: *to,
                policy,
            },
            Self::SetStatus { run, change } => Op::SetStatus {
                run_id: resolve(run, created)?,
                change: *change,
                policy: &policy.reminders,
            },
            Self::SwapParticipants {
                run,
                remove,
                add,
                via_portal,
            } => Op::SwapParticipants {
                run_id: resolve(run, created)?,
                remove: remove.clone(),
                add: add.clone(),
                via_portal: *via_portal,
                directory,
            },
            Self::SetRsvp {
                run,
                user_id,
                state,
                source,
            } => Op::SetRsvp {
                run_id: resolve(run, created)?,
                user_id: user_id.clone(),
                state: *state,
                source: *source,
            },
            Self::ResetToFixed { run } => Op::ResetToFixed {
                run_id: resolve(run, created)?,
                policy,
            },
            Self::SetRunBosses { run, bosses } => Op::SetRunBosses {
                run_id: resolve(run, created)?,
                bosses: bosses.clone(),
                policy: &policy.reminders,
            },
            Self::EnsureReminders { run } => Op::EnsureReminders {
                run_id: resolve(run, created)?,
                rebuild: false,
                policy: &policy.reminders,
            },
            Self::RecountRun { run } => Op::RecountRun {
                run_id: resolve(run, created)?,
            },
            Self::ReviveRun { run } => Op::ReviveRun {
                run_id: resolve(run, created)?,
            },
        })
    }
}

/// Rewrite every created-target ordinal: `renumber` maps the old position
/// to the new one (`None` refuses the rewrite).
pub fn renumber_created(
    op: &mut DraftOp,
    renumber: impl Fn(usize) -> Option<usize> + Copy,
) -> bool {
    let mut ok = true;
    op.each_target_mut(|target| {
        if let Target::Created(ord) = target {
            match renumber(*ord) {
                Some(next) => *ord = next,
                None => ok = false,
            }
        }
    });
    ok
}

impl DraftOp {
    /// Every row target the operation names.
    pub fn each_target_mut(&mut self, mut visit: impl FnMut(&mut Target)) {
        match self {
            Self::AddFixedRun(_) => {}
            Self::ApplyFixedEdit { fixed, .. }
            | Self::FixedParticipants { fixed, .. }
            | Self::RetireFixedRun { fixed, .. } => visit(fixed),
            Self::CreateRun { fixed, .. } => {
                if let Some(fixed) = fixed {
                    visit(fixed);
                }
            }
            Self::AmendRun { run, .. }
            | Self::SetStatus { run, .. }
            | Self::SwapParticipants { run, .. }
            | Self::SetRsvp { run, .. }
            | Self::ResetToFixed { run }
            | Self::SetRunBosses { run, .. }
            | Self::EnsureReminders { run }
            | Self::RecountRun { run }
            | Self::ReviveRun { run } => visit(run),
        }
    }

    /// Only a proposal stages it (v4 `commit` semantics); administrator
    /// drafts and requests refuse it.
    pub fn is_proposal_only(&self) -> bool {
        matches!(
            self,
            Self::SetRunBosses { .. }
                | Self::EnsureReminders { .. }
                | Self::RecountRun { .. }
                | Self::ReviveRun { .. }
        )
    }

    /// Whether this operation creates the row `Target::Created` names.
    pub fn creates_row(&self) -> bool {
        matches!(self, Self::AddFixedRun(_) | Self::CreateRun { .. })
    }

    /// The operation's kind, as the codec names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AddFixedRun(_) => "add_fixed_run",
            Self::ApplyFixedEdit { .. } => "apply_fixed_edit",
            Self::FixedParticipants { .. } => "fixed_participants",
            Self::RetireFixedRun { .. } => "retire_fixed_run",
            Self::CreateRun { .. } => "create_run",
            Self::AmendRun { .. } => "amend_run",
            Self::SetStatus { .. } => "set_status",
            Self::SwapParticipants { .. } => "swap_participants",
            Self::SetRsvp { .. } => "set_rsvp",
            Self::ResetToFixed { .. } => "reset_to_fixed",
            Self::SetRunBosses { .. } => "set_run_bosses",
            Self::EnsureReminders { .. } => "ensure_reminders",
            Self::RecountRun { .. } => "recount_run",
            Self::ReviveRun { .. } => "revive_run",
        }
    }
}
