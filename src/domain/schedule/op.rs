//! Every attributed schedule mutation as one value, and its single pure
//! implementation [`apply_op`]. The scheduler service's methods and draft
//! replay both go through it.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::draft::Draft;
use super::error::ScheduleError;
use super::fixed_edit::{FixedEditRequest, apply_fixed_edit, apply_party_delta};
use super::lifecycle::{apply_fixed_to_runs, mark_done, retire_fixed_run};
use super::materialise::{materialise_week, materialise_weeks};
use super::mutate::{
    StatusChange, amend_run, reset_to_fixed, set_status, swap_participants, swap_run_slots,
};
use super::notice::{Notice, NoticeChange, Outcome};
use super::policy::SchedulePolicy;
use super::policy::utc_instant;
use super::reminders::{ReminderPolicy, ensure_reminders, reconcile_day_of, refresh_run_reminders};
use super::roster::RunState;
use super::rsvp::{ReactionResult, apply_reaction, derive_run_status};
use super::run::{
    FixedField, FixedRun, FixedRunPatch, NewFixedRun, NewRun, RsvpSource, RsvpState, RunStatus,
};
use crate::domain::ids::IdGenerator;
use crate::domain::members::Directory;
use crate::domain::time::{AwareDateTime, DateOutOfRange, ZonedDateTime};

/// A week start as the caller gave it: its UTC instant (the request scope)
/// and its guild-zone view. A failed conversion is only raised where a
/// planner reads the week, exactly as when planners took the caller's value.
#[derive(Clone, Copy, Debug)]
pub struct WeekStart {
    instant: DateTime<Utc>,
    zoned: Result<ZonedDateTime, DateOutOfRange>,
}

impl WeekStart {
    /// # Errors
    /// [`DateOutOfRange`] when the week has no UTC instant.
    pub fn new(week: &impl AwareDateTime, zone: Tz) -> Result<Self, DateOutOfRange> {
        Ok(Self {
            instant: utc_instant(week)?,
            zoned: week.astimezone(zone),
        })
    }

    pub fn instant(&self) -> DateTime<Utc> {
        self.instant
    }
}

impl AwareDateTime for WeekStart {
    fn astimezone(&self, zone: Tz) -> Result<ZonedDateTime, DateOutOfRange> {
        // Already in the planner's zone, this is the caller's own view.
        self.zoned?.astimezone(zone)
    }
}

/// One attributed mutation with its arguments. Policies and directories are
/// configuration, borrowed from the caller.
#[derive(Clone)]
pub enum Op<'a> {
    AddFixedRun(NewFixedRun),
    /// A new weekly timing and its runs in the materialised weeks as one
    /// change (the admin and Discord create). Its request digest is
    /// `add_fixed_run`'s, so a key a plain create recorded still replays.
    AddFixedRunMaterialised {
        new: NewFixedRun,
        policy: &'a SchedulePolicy,
    },
    CreateRun(NewRun),
    MaterialiseWeek {
        week_start: WeekStart,
        policy: &'a ReminderPolicy,
    },
    SetRunStatus {
        run_id: String,
        status: RunStatus,
    },
    SetRsvp {
        run_id: String,
        user_id: String,
        state: RsvpState,
        source: RsvpSource,
    },
    ApplyReaction {
        run_id: String,
        user_id: String,
        emoji: String,
        added: bool,
    },
    EditFixedRun {
        fixed_id: String,
        patch: FixedRunPatch,
        changed: Vec<FixedField>,
        week_starts: Vec<WeekStart>,
        policy: &'a ReminderPolicy,
    },
    RetireFixedRun {
        fixed_id: String,
        week_starts: Vec<WeekStart>,
        policy: &'a ReminderPolicy,
    },
    EnsureReminders {
        run_id: String,
        rebuild: bool,
        policy: &'a ReminderPolicy,
    },
    AddReminder {
        run_id: String,
        kind: String,
        fire_at: DateTime<Utc>,
        sent_at: Option<DateTime<Utc>>,
    },
    MarkReminderSent {
        reminder_id: String,
        message_id: Option<String>,
    },
    RescheduleUnpostedReminder {
        reminder_id: String,
        fire_at: DateTime<Utc>,
    },
    ReconcileDayOf {
        policy: &'a ReminderPolicy,
    },
    MarkDone,
    MaterialiseWeeks {
        policy: &'a SchedulePolicy,
    },
    SetStatus {
        run_id: String,
        change: StatusChange,
        policy: &'a ReminderPolicy,
    },
    AmendRun {
        run_id: String,
        to: DateTime<Utc>,
        policy: &'a SchedulePolicy,
    },
    SwapRunSlots {
        run_id: String,
        with_id: String,
        /// The portal's raw week version is part of idempotency identity.
        request_version: Option<u64>,
        policy: &'a SchedulePolicy,
    },
    SwapParticipants {
        run_id: String,
        remove: Vec<String>,
        add: Vec<String>,
        via_portal: bool,
        directory: &'a (dyn Directory + Sync),
    },
    ApplyFixedEdit {
        request: FixedEditRequest,
        directory: &'a (dyn Directory + Sync),
        policy: &'a SchedulePolicy,
    },
    ResetToFixed {
        run_id: String,
        policy: &'a SchedulePolicy,
    },
    /// v5 attendance: set or clear a member's standing answer.
    SetStandingAnswer {
        fixed_id: String,
        user_id: String,
        on: bool,
        /// `<kind>:<id>` of who set it (stored with the answer).
        set_by: String,
    },
    /// v5 attendance: a weekly timing's default for unanswered members.
    SetAttendanceDefault {
        fixed_id: String,
        default: crate::domain::attendance::AttendanceDefault,
    },
    /// v5 attendance: record who attended a done run.
    RecordAttendance {
        run_id: String,
        actor: crate::domain::attendance::AttendanceActor,
        attended: std::collections::BTreeSet<String>,
        /// `<kind>:<id>` of who recorded it.
        recorded_by: String,
    },
    /// v5 attendance: re-derive live runs' statuses (the tick).
    RecountAttendance,
    /// v5 only (draft replay): a weekly timing's party changed by delta.
    FixedParticipants {
        fixed_id: String,
        add: Vec<String>,
        remove: Vec<String>,
        directory: &'a (dyn Directory + Sync),
        policy: &'a SchedulePolicy,
    },
    /// v5 only (proposal split): replace a run's bosses and rebuild its
    /// reminders, as v4 `_split` does to the run it shrinks.
    SetRunBosses {
        run_id: String,
        bosses: Vec<String>,
        policy: &'a ReminderPolicy,
    },
    /// v5 only (proposed answers): re-derive one run's status from its
    /// answers, as v4 `api.service.set_rsvp`; a pin or a started run keeps
    /// its status ([`derive_run_status`](super::rsvp::derive_run_status)).
    RecountRun {
        run_id: String,
    },
    /// v5 only (proposal move): a cancelled or otot run goes back to
    /// `planned` without clearing answers, as v4 `_move`; others unchanged.
    ReviveRun {
        run_id: String,
    },
}

impl Op<'_> {
    /// The schedule policy the operation carries, if any.
    pub fn schedule_policy(&self) -> Option<&SchedulePolicy> {
        match self {
            Self::MaterialiseWeeks { policy }
            | Self::AddFixedRunMaterialised { policy, .. }
            | Self::AmendRun { policy, .. }
            | Self::SwapRunSlots { policy, .. }
            | Self::ApplyFixedEdit { policy, .. }
            | Self::ResetToFixed { policy, .. }
            | Self::FixedParticipants { policy, .. } => Some(policy),
            _ => None,
        }
    }
}

/// What an operation returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpResult {
    /// The id of the weekly timing or run created.
    Created(String),
    /// Ids created (materialising, reminders) or runs retired (`mark_done`).
    Ids(Vec<String>),
    Done,
    Reaction(ReactionResult),
    Count(usize),
    /// An inserted reminder's id; `None` when the kind existed.
    Reminder(Option<String>),
    Changed(bool),
    Run(RunState),
    Runs(Vec<RunState>),
    Fixed(FixedRun),
    /// A party delta, and the runs it left alone rather than empty.
    PartyDelta(super::fixed_edit::PartyDelta),
}

fn quiet(result: OpResult) -> Outcome<OpResult> {
    Outcome::quiet(result)
}

fn run(outcome: Outcome<RunState>) -> Outcome<OpResult> {
    Outcome {
        value: OpResult::Run(outcome.value),
        notices: outcome.notices,
    }
}

/// Apply one operation to `draft` at `now`.
///
/// # Errors
/// The operation's [`ScheduleError`]; the draft may be partly changed and
/// must then be discarded.
pub fn apply_op(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    op: &Op<'_>,
    now: DateTime<Utc>,
) -> Result<Outcome<OpResult>, ScheduleError> {
    Ok(match op {
        Op::AddFixedRun(new) => {
            let fixed_id = draft.add_fixed_run(ids, new.clone());
            let fixed = draft
                .fixed_run(&fixed_id)
                .cloned()
                .expect("added fixed run is present");
            Outcome {
                value: OpResult::Created(fixed_id.clone()),
                notices: vec![Notice {
                    change: NoticeChange::FixedAdded {
                        fixed_id,
                        bosses: fixed.bosses,
                        weekday: fixed.weekday,
                        time: fixed.time,
                        participants: fixed.participants.clone(),
                    },
                    channel_id: fixed.channel_id,
                    listed: fixed.participants,
                    via_portal: true,
                }],
            }
        }
        Op::AddFixedRunMaterialised { new, policy } => {
            let outcome = apply_op(draft, ids, &Op::AddFixedRun(new.clone()), now)?;
            materialise_weeks(draft, ids, policy, now)?;
            outcome
        }
        Op::CreateRun(new) => quiet(OpResult::Created(draft.create_run(ids, new.clone())?)),
        Op::MaterialiseWeek { week_start, policy } => quiet(OpResult::Ids(materialise_week(
            draft,
            ids,
            *week_start,
            policy,
            now,
        )?)),
        Op::SetRunStatus { run_id, status } => {
            draft.set_run_status(run_id, *status);
            if draft
                .run(run_id)
                .and_then(|run| run.status_pin)
                .is_some_and(|pin| pin.status != *status)
            {
                draft.set_run_pin(run_id, None);
            }
            quiet(OpResult::Done)
        }
        Op::SetRsvp {
            run_id,
            user_id,
            state,
            source,
        } => {
            draft.set_rsvp(run_id, user_id, *state, *source, now);
            quiet(OpResult::Done)
        }
        Op::ApplyReaction {
            run_id,
            user_id,
            emoji,
            added,
        } => quiet(OpResult::Reaction(apply_reaction(
            draft, run_id, user_id, emoji, *added, now,
        )?)),
        Op::EditFixedRun {
            fixed_id,
            patch,
            changed,
            week_starts,
            policy,
        } => {
            draft.update_fixed_run(fixed_id, patch.clone());
            quiet(OpResult::Count(apply_fixed_to_runs(
                draft,
                ids,
                fixed_id,
                changed,
                week_starts,
                policy,
                now,
            )?))
        }
        Op::RetireFixedRun {
            fixed_id,
            week_starts,
            policy,
        } => {
            let fixed = draft
                .fixed_run(fixed_id)
                .cloned()
                .ok_or_else(|| ScheduleError::UnknownFixedRun(fixed_id.clone()))?;
            let cancelled_runs = retire_fixed_run(draft, ids, fixed_id, week_starts, policy, now)?;
            Outcome {
                value: OpResult::Count(cancelled_runs),
                notices: vec![Notice {
                    change: NoticeChange::FixedRemoved {
                        fixed_id: fixed_id.clone(),
                        bosses: fixed.bosses,
                        weekday: fixed.weekday,
                        time: fixed.time,
                        participants: fixed.participants.clone(),
                        cancelled_runs,
                    },
                    channel_id: fixed.channel_id,
                    listed: fixed.participants,
                    via_portal: true,
                }],
            }
        }
        Op::EnsureReminders {
            run_id,
            rebuild,
            policy,
        } => quiet(OpResult::Ids(ensure_reminders(
            draft, ids, run_id, policy, now, *rebuild,
        )?)),
        Op::AddReminder {
            run_id,
            kind,
            fire_at,
            sent_at,
        } => quiet(OpResult::Reminder(
            draft.add_reminder(ids, run_id, kind, *fire_at, *sent_at),
        )),
        Op::MarkReminderSent {
            reminder_id,
            message_id,
        } => {
            draft.mark_reminder_sent(reminder_id, message_id.as_deref(), now);
            quiet(OpResult::Done)
        }
        Op::RescheduleUnpostedReminder {
            reminder_id,
            fire_at,
        } => quiet(OpResult::Changed(draft.reschedule_unposted_reminder(
            reminder_id,
            *fire_at,
            now,
        ))),
        Op::ReconcileDayOf { policy } => {
            quiet(OpResult::Count(reconcile_day_of(draft, policy, now)?))
        }
        Op::MarkDone => quiet(OpResult::Ids(mark_done(draft, now))),
        Op::MaterialiseWeeks { policy } => {
            quiet(OpResult::Ids(materialise_weeks(draft, ids, policy, now)?))
        }
        Op::SetStatus {
            run_id,
            change,
            policy,
        } => run(set_status(draft, ids, run_id, *change, policy, now)?),
        Op::AmendRun { run_id, to, policy } => {
            run(amend_run(draft, ids, run_id, *to, policy, now)?)
        }
        Op::SwapRunSlots {
            run_id,
            with_id,
            policy,
            ..
        } => {
            let outcome = swap_run_slots(draft, ids, run_id, with_id, policy, now)?;
            Outcome {
                value: OpResult::Runs(outcome.value),
                notices: outcome.notices,
            }
        }
        Op::SwapParticipants {
            run_id,
            remove,
            add,
            via_portal,
            directory,
        } => run(swap_participants(
            draft,
            directory,
            run_id,
            remove,
            add,
            *via_portal,
            now,
        )?),
        Op::ApplyFixedEdit {
            request,
            directory,
            policy,
        } => {
            let outcome = apply_fixed_edit(draft, ids, directory, request, policy, now)?;
            Outcome {
                value: OpResult::Fixed(outcome.value),
                notices: outcome.notices,
            }
        }
        Op::SetStandingAnswer {
            fixed_id,
            user_id,
            on,
            set_by,
        } => {
            let outcome =
                super::attendance::set_standing_answer(draft, fixed_id, user_id, *on, set_by, now)?;
            Outcome {
                value: OpResult::Fixed(outcome.value),
                notices: outcome.notices,
            }
        }
        Op::SetAttendanceDefault { fixed_id, default } => {
            let outcome =
                super::attendance::set_attendance_default(draft, fixed_id, *default, now)?;
            Outcome {
                value: OpResult::Fixed(outcome.value),
                notices: outcome.notices,
            }
        }
        Op::FixedParticipants {
            fixed_id,
            add,
            remove,
            directory,
            policy,
        } => {
            let outcome =
                apply_party_delta(draft, ids, directory, fixed_id, add, remove, policy, now)?;
            Outcome {
                value: OpResult::PartyDelta(outcome.value),
                notices: outcome.notices,
            }
        }
        Op::RecordAttendance {
            run_id,
            actor,
            attended,
            recorded_by,
        } => run(super::attendance::record_attendance(
            draft,
            run_id,
            actor,
            attended,
            recorded_by,
            now,
        )?),
        Op::RecountAttendance => quiet(OpResult::Ids(super::attendance::recount_attendance(
            draft, now,
        ))),
        Op::ResetToFixed { run_id, policy } => {
            run(reset_to_fixed(draft, ids, run_id, policy, now)?)
        }
        Op::SetRunBosses {
            run_id,
            bosses,
            policy,
        } => {
            draft.require_run(run_id)?;
            draft.set_run_bosses(run_id, bosses.clone());
            refresh_run_reminders(draft, ids, run_id, policy, now)?;
            quiet(OpResult::Done)
        }
        Op::RecountRun { run_id } => {
            let before = draft.require_run(run_id)?;
            let status = derive_run_status(draft, &before, before.status, now).status;
            if status != before.status {
                draft.set_run_status(run_id, status);
            }
            quiet(OpResult::Changed(status != before.status))
        }
        Op::ReviveRun { run_id } => {
            let revive = matches!(
                draft.require_run(run_id)?.status,
                RunStatus::Cancelled | RunStatus::Otot
            );
            if revive {
                draft.set_run_status(run_id, RunStatus::Planned);
            }
            quiet(OpResult::Changed(revive))
        }
    })
}
