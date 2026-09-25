use std::fmt;

use crate::domain::attendance::{AttendancePolicy, AttendanceRefusal};
use crate::domain::pytext::repr;
use crate::domain::time::DateOutOfRange;

/// Scheduling rule failures; `Display` keeps v4's `ValueError`/`BadRequest`
/// messages because they reach members verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    UnknownRunStatus(String),
    UnknownRunSource(String),
    UnknownRsvpState(String),
    UnknownRsvpSource(String),
    /// A move would give one weekly timing two runs in the same boss week.
    RunMoveConflict,
    /// The run an operation needs does not exist (v4 would fail on `None`).
    UnknownRun(String),
    DateOutOfRange(DateOutOfRange),
    /// A status that cannot be set by hand (`at_risk` or unknown text).
    NotSettable(String),
    /// An amend into a week whose weekly already has a run; fields are rendered
    /// in the guild zone (`Thu 03 Sep`, `00000000`, `Mon 07 Sep`, `21:30`).
    MoveConflict {
        week_day: String,
        existing_short_id: String,
        existing_day: String,
        existing_time: String,
    },
    /// `Name (id)` for each id removed from a run it is not on.
    NotOnRun(Vec<String>),
    NotAUserId(String),
    NoParticipants,
    /// A swap would leave the run with nobody on it.
    RunEmptied,
    /// `Name (id)` for each id without the bossing role.
    NotInRole(Vec<String>),
    BotParticipants(Vec<String>),
    ChannelNotWatched(String),
    NothingToChange,
    UnknownFixedRun(String),
    /// `reset_to_fixed` on a run no weekly timing produced.
    NotAFixedRun(String),
    /// `reset_to_fixed` on a run whose weekly timing was retired.
    FixedRunRetired(String),
    /// A done or cancelled run is the record of that night.
    RunNotLive {
        run_id: String,
        status: String,
    },
    /// v5: `reset_to_fixed` would put a live run on a slot at or before now;
    /// the slot is rendered in the guild zone (`Mon 07 Sep`, `21:30`).
    ResetSlotPassed {
        run_id: String,
        slot_day: String,
        slot_time: String,
    },
    /// A fixed-edit choice names a run the edit does not affect.
    UnexpectedChoice(String),
    /// An affected amended run has no fixed-edit choice.
    MissingChoice(String),
    /// v5: a standing answer for someone not in the weekly timing's party.
    NotInParty {
        user_id: String,
    },
    /// v5: an attendance record the rules refuse.
    Attendance(AttendanceRefusal),
    /// Configuration: a call's `SchedulePolicy.attendance` differs from the
    /// scheduler's (one source of truth for the attendance rules).
    AttendanceMismatch {
        service: AttendancePolicy,
        policy: AttendancePolicy,
    },
}

impl From<DateOutOfRange> for ScheduleError {
    fn from(error: DateOutOfRange) -> Self {
        Self::DateOutOfRange(error)
    }
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRunStatus(value) => write!(f, "unknown run status {}", repr(value)),
            Self::UnknownRunSource(value) => write!(f, "unknown run source {}", repr(value)),
            Self::UnknownRsvpState(value) => write!(f, "unknown rsvp state {}", repr(value)),
            Self::UnknownRsvpSource(value) => write!(f, "unknown rsvp source {}", repr(value)),
            Self::RunMoveConflict => f.write_str("that weekly already has a run in that boss week"),
            Self::UnknownRun(id) => write!(f, "no run {}", repr(id)),
            Self::DateOutOfRange(error) => error.fmt(f),
            Self::NotSettable(status) => write!(
                f,
                "`{status}` is not a status you can set - one of planned, confirmed, otot, \
                 done, cancelled. `at_risk` is derived from the answers people give."
            ),
            Self::MoveConflict {
                week_day,
                existing_short_id,
                existing_day,
                existing_time,
            } => write!(
                f,
                "that weekly already has a run in the week of {week_day} \
                 (#{existing_short_id} on {existing_day} {existing_time}). \
                 Edit that existing run instead, or keep this move within its current boss week."
            ),
            Self::NotOnRun(names) => write!(f, "not on this run: {}", names.join(", ")),
            Self::NotAUserId(value) => write!(f, "`{value}` is not a Discord user id"),
            Self::NoParticipants => f.write_str("a run needs at least one participant"),
            Self::RunEmptied => {
                f.write_str("a run needs at least one participant - cancel it instead")
            }
            Self::NotInRole(names) => write!(f, "not in the bossing role: {}", names.join(", ")),
            Self::BotParticipants(ids) => {
                write!(f, "bots can't be participants: {}", ids.join(", "))
            }
            Self::ChannelNotWatched(id) => write!(
                f,
                "channel {id} isn't watched, so its runs would never get their pings - \
                 add it to CHAT_CHANNEL_IDS, or its category to CHAT_CATEGORY_IDS"
            ),
            Self::NothingToChange => f.write_str("nothing to change"),
            Self::UnknownFixedRun(id) => write!(f, "no fixed run `{id}`"),
            Self::NotAFixedRun(id) => write!(
                f,
                "run {id} is a one-off - there is no weekly timing to reset it to"
            ),
            Self::FixedRunRetired(id) => write!(
                f,
                "the weekly timing {id} of that run was removed - there is nothing to reset it to"
            ),
            Self::NotInParty { user_id } => {
                write!(f, "{user_id} is not in that weekly run's party")
            }
            Self::Attendance(refusal) => refusal.fmt(f),
            Self::AttendanceMismatch { service, policy } => write!(
                f,
                "the schedule policy's attendance rules ({policy:?}) differ from the \
                 scheduler's ({service:?})"
            ),
            Self::RunNotLive { run_id, status } => {
                write!(f, "run {run_id} is {status} - it is left as the record")
            }
            Self::ResetSlotPassed {
                run_id,
                slot_day,
                slot_time,
            } => write!(
                f,
                "run {run_id} can't be reset - its weekly slot {slot_day} {slot_time} has \
                 already passed"
            ),
            Self::UnexpectedChoice(id) => write!(
                f,
                "run {id} is not an amended run this edit would move - choose only for those"
            ),
            Self::MissingChoice(id) => write!(
                f,
                "run {id} was amended this week - choose whether it follows the new time"
            ),
        }
    }
}

impl std::error::Error for ScheduleError {}
