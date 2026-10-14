use crate::domain::attendance::{AttendancePolicy, AttendanceRefusal};
use crate::domain::pytext::repr;
use crate::domain::time::DateOutOfRange;

/// Scheduling rule failures; `Display` keeps v4's `ValueError`/`BadRequest`
/// messages because they reach members verbatim.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("unknown run status {}", repr(.0))]
    UnknownRunStatus(String),
    #[error("unknown run source {}", repr(.0))]
    UnknownRunSource(String),
    #[error("unknown rsvp state {}", repr(.0))]
    UnknownRsvpState(String),
    #[error("unknown rsvp source {}", repr(.0))]
    UnknownRsvpSource(String),
    /// A move would give one weekly timing two runs in the same boss week.
    #[error("that weekly already has a run in that boss week")]
    RunMoveConflict,
    /// The run an operation needs does not exist (v4 would fail on `None`).
    #[error("no run {}", repr(.0))]
    UnknownRun(String),
    #[error("{0}")]
    DateOutOfRange(DateOutOfRange),
    /// A status that cannot be set by hand (`at_risk` or unknown text).
    #[error(
        "`{0}` is not a status you can set - one of planned, confirmed, otot, \
         done, cancelled. `at_risk` is derived from the answers people give."
    )]
    NotSettable(String),
    /// An amend into a week whose weekly already has a run; fields are rendered
    /// in the guild zone (`Thu 03 Sep`, `00000000`, `Mon 07 Sep`, `21:30`).
    #[error(
        "that weekly already has a run in the week of {week_day} \
         (#{existing_short_id} on {existing_day} {existing_time}). \
         Edit that existing run instead, or keep this move within its current boss week."
    )]
    MoveConflict {
        week_day: String,
        existing_short_id: String,
        existing_day: String,
        existing_time: String,
    },
    /// `Name (id)` for each id removed from a run it is not on.
    #[error("not on this run: {}", .0.join(", "))]
    NotOnRun(Vec<String>),
    #[error("`{0}` is not a Discord user id")]
    NotAUserId(String),
    #[error("a run needs at least one participant")]
    NoParticipants,
    /// A swap would leave the run with nobody on it.
    #[error("a run needs at least one participant - cancel it instead")]
    RunEmptied,
    /// `Name (id)` for each id without the bossing role.
    #[error("not in the bossing role: {}", .0.join(", "))]
    NotInRole(Vec<String>),
    #[error("bots can't be participants: {}", .0.join(", "))]
    BotParticipants(Vec<String>),
    #[error(
        "channel {0} isn't watched, so its runs would never get their pings - \
         add it to CHAT_CHANNEL_IDS, or its category to CHAT_CATEGORY_IDS"
    )]
    ChannelNotWatched(String),
    #[error("nothing to change")]
    NothingToChange,
    #[error("no fixed run `{0}`")]
    UnknownFixedRun(String),
    /// `reset_to_fixed` on a run no weekly timing produced.
    #[error("run {0} is a one-off - there is no weekly timing to reset it to")]
    NotAFixedRun(String),
    /// `reset_to_fixed` on a run whose weekly timing was retired.
    #[error("the weekly timing {0} of that run was removed - there is nothing to reset it to")]
    FixedRunRetired(String),
    /// A done or cancelled run is the record of that night.
    #[error("run {run_id} is {status} - it is left as the record")]
    RunNotLive { run_id: String, status: String },
    /// A slot exchange needs two distinct run rows.
    #[error("a run cannot be swapped with itself")]
    SameRunSwap,
    /// A slot exchange never moves either run into another boss week.
    #[error("runs can only swap within the same boss week")]
    DifferentSwapWeek,
    /// A non-midnight reset can put a swapped local date/time in the prior week.
    #[error("that slot swap would move a run outside its boss week")]
    SwapLeavesWeek,
    /// v5: `reset_to_fixed` would put a live run on a slot at or before now;
    /// the slot is rendered in the guild zone (`Mon 07 Sep`, `21:30`).
    #[error(
        "run {run_id} can't be reset - its weekly slot {slot_day} {slot_time} has \
         already passed"
    )]
    ResetSlotPassed {
        run_id: String,
        slot_day: String,
        slot_time: String,
    },
    /// A fixed-edit choice names a run the edit does not affect.
    #[error("run {0} is not an amended run this edit would move - choose only for those")]
    UnexpectedChoice(String),
    /// An affected amended run has no fixed-edit choice.
    #[error("run {0} was amended this week - choose whether it follows the new time")]
    MissingChoice(String),
    /// v5: a standing answer for someone not in the weekly timing's party.
    #[error("{user_id} is not in that weekly run's party")]
    NotInParty { user_id: String },
    /// v5: an attendance record the rules refuse.
    #[error("{0}")]
    Attendance(AttendanceRefusal),
    /// Configuration: a call's `SchedulePolicy.attendance` differs from the
    /// scheduler's (one source of truth for the attendance rules).
    #[error(
        "the schedule policy's attendance rules ({policy:?}) differ from the \
         scheduler's ({service:?})"
    )]
    AttendanceMismatch {
        service: AttendancePolicy,
        policy: AttendancePolicy,
    },
    /// v5: a member's write on their own run is not allowed (see
    /// [`MemberRunRefusal`]); checked inside the commit.
    #[error("{}", match .0 {
        MemberRunRefusal::NotInRun => "you are not on this run",
        MemberRunRefusal::Closed => "this run is finished or cancelled",
        MemberRunRefusal::Ended => RUN_ENDED,
        MemberRunRefusal::WeekOver => "this run is outside the boss week you can change",
        MemberRunRefusal::Started => "this run has already started",
        MemberRunRefusal::InThePast => "that time has already passed",
    })]
    MemberRun(MemberRunRefusal),
    /// v5 (user decision 2026-10-10): a live run whose end has passed is
    /// frozen until it is settled; move, swap, amend and late answers are
    /// refused ([`RunEnds`](crate::domain::completion::RunEnds)).
    #[error("{}", RUN_ENDED)]
    RunEnded { run_id: String },
    /// v5: an owner change the ownership rules refuse on the committed
    /// timing ([`OwnerPin`](crate::domain::ownership::OwnerPin)).
    #[error("{0}")]
    Ownership(crate::domain::ownership::OwnershipRefusal),
}

/// What both refusals of a frozen run say.
pub const RUN_ENDED: &str = "That run has already ended.";

/// Why a member may not answer or move their own run now (public portal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberRunRefusal {
    NotInRun,
    /// Done or cancelled.
    Closed,
    /// Live but past its end: frozen until it is settled.
    Ended,
    /// Answers: outside this and next boss week. Moves: outside this one.
    WeekOver,
    Started,
    /// The target slot has passed.
    InThePast,
}

impl MemberRunRefusal {
    /// May `member` write `run` now? `weeks` are the boss weeks (start
    /// instants) they may act in; `ended` is whether the run is frozen
    /// ([`crate::domain::completion::RunEnds::frozen`]); a move also needs
    /// the run not started and its target (`to`) still ahead.
    pub fn check(
        run: &super::Run,
        member: &str,
        weeks: &[chrono::DateTime<chrono::Utc>],
        to: Option<chrono::DateTime<chrono::Utc>>,
        ended: bool,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Self> {
        if !run.participants.iter().any(|id| id == member) {
            Err(Self::NotInRun)
        } else if run.status.is_terminal() {
            Err(Self::Closed)
        } else if ended {
            Err(Self::Ended)
        } else if !weeks.contains(&run.week_start) {
            Err(Self::WeekOver)
        } else if to.is_some() && run.datetime <= now {
            Err(Self::Started)
        } else if to.is_some_and(|to| to <= now) {
            Err(Self::InThePast)
        } else {
            Ok(())
        }
    }
}

impl From<DateOutOfRange> for ScheduleError {
    fn from(error: DateOutOfRange) -> Self {
        Self::DateOutOfRange(error)
    }
}
