//! What a three-way merge compares (entities and their merge fields) and
//! the conflicts it reports.

use std::fmt;

use chrono::{DateTime, Utc};

use super::op::ReplayError;

/// A merged entity. Rows a draft created are named `created:<ord>.<n>`: the
/// `n`th weekly timing or run (in id-draw order) its operation `ord` created,
/// so both replays name them alike.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Entity {
    Fixed(String),
    Run(String),
    Rsvp { run_id: String, user_id: String },
}

/// The fields a merge compares. Reminders, run `source` and `week_start`
/// (derived from the slot) and RSVP timestamps are not merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    /// Run: the instant.
    Slot,
    /// Weekly timing: weekday and wall time together.
    DayTime,
    Bosses,
    /// Merged as a set.
    Participants,
    Channel,
    Status,
    /// Run: its weekly timing.
    FixedRun,
    Note,
    Owner,
    /// RSVP: state and source together.
    Answer,
}

impl Field {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slot => "slot",
            Self::DayTime => "day_time",
            Self::Bosses => "bosses",
            Self::Participants => "participants",
            Self::Channel => "channel",
            Self::Status => "status",
            Self::FixedRun => "fixed_run",
            Self::Note => "note",
            Self::Owner => "owner",
            Self::Answer => "answer",
        }
    }
}

/// A field's value; `None` in a map lookup means the entity is absent.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldValue {
    Text(Option<String>),
    /// Sorted and deduplicated.
    Set(Vec<String>),
}

/// How upstream took away an entity the draft changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Removal {
    /// A run or RSVP row no longer exists.
    Deleted,
    /// The weekly timing was retired.
    Retired,
    Cancelled,
    Done,
    /// An answer's member is no longer on the run.
    LeftRun,
}

/// Why a draft cannot merge as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeConflict {
    /// Draft and upstream set one field to different values.
    BothChanged {
        entity: Entity,
        field: Field,
        base: Option<FieldValue>,
        draft: Option<FieldValue>,
        upstream: Option<FieldValue>,
    },
    /// Upstream removed, retired, cancelled or finished what the draft edits.
    UpstreamRemoved { entity: Entity, removal: Removal },
    /// The operation at `ord` cannot be applied to the current schedule
    /// (or, `on_base`, not even to the draft's own base).
    OpRejected {
        ord: usize,
        error: ReplayError,
        on_base: bool,
    },
    /// A weekly-timing edit's per-run choices were made for a different set
    /// of amended runs than the current schedule has.
    ChoicesStale {
        ord: usize,
        base: Vec<String>,
        current: Vec<String>,
    },
    /// A retirement's staged boss weeks are not the ones materialised at
    /// merge time (a reset happened); re-stage the operation.
    StaleWeeks {
        ord: usize,
        staged: Vec<DateTime<Utc>>,
        current: Vec<DateTime<Utc>>,
    },
    /// Replaying on the current schedule gives a field a value the
    /// three-way merge does not expect.
    Divergent { entity: Entity, field: Field },
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixed(id) => write!(f, "weekly timing {id}"),
            Self::Run(id) => write!(f, "run {id}"),
            Self::Rsvp { run_id, user_id } => write!(f, "answer of {user_id} on run {run_id}"),
        }
    }
}
