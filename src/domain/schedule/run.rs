//! Scheduler rows: weekly timings, concrete runs, RSVPs and reminder rows.

use crate::domain::attendance::{AttendanceDefault, AttendanceRecord, StandingAnswer, StatusPin};
use chrono::{DateTime, NaiveTime, Utc, Weekday};

use super::error::ScheduleError;

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident, $error:ident, { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The stored v4 spelling.
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            /// # Errors
            #[doc = concat!("[`ScheduleError::", stringify!($error), "`] for any other text.")]
            pub fn parse(value: &str) -> Result<Self, ScheduleError> {
                match value {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(ScheduleError::$error(value.to_owned())),
                }
            }
        }
    };
}

text_enum!(
    /// A run's lifecycle state.
    RunStatus, UnknownRunStatus, {
        Planned => "planned",
        Confirmed => "confirmed",
        AtRisk => "at_risk",
        Otot => "otot",
        Done => "done",
        Cancelled => "cancelled",
    }
);

text_enum!(
    /// Whether a run was produced by a weekly timing or an amendment.
    RunSource, UnknownRunSource, {
        Fixed => "fixed",
        Amend => "amend",
    }
);

text_enum!(
    /// One member's answer for a run.
    RsvpState, UnknownRsvpState, {
        Yes => "yes",
        No => "no",
        Maybe => "maybe",
    }
);

text_enum!(
    /// Where an RSVP came from; v4's set, so export maps 1:1 (portal/API
    /// answers are recorded as `chat`, as v4 does).
    RsvpSource, UnknownRsvpSource, {
        Reaction => "reaction",
        Chat => "chat",
        Slash => "slash",
    }
);

impl RunStatus {
    /// Still to come: `mark_done` retires these once their slot has passed.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::Planned | Self::Confirmed | Self::AtRisk | Self::Otot
        )
    }

    /// Over or called off: never rewritten by weekly edits or retirement.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// A weekly timing (`fixed_runs` row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedRun {
    pub id: String,
    /// The stored owner: who staff pinned, else who created the timing (v4's
    /// column). Read the effective owner with [`FixedRun::owner`].
    pub owner_id: String,
    pub channel_id: Option<String>,
    pub bosses: Vec<String>,
    pub weekday: Weekday,
    /// Guild-local wall clock, minute precision as v4 stores `HH:MM`.
    pub time: NaiveTime,
    pub participants: Vec<String>,
    pub note: Option<String>,
    /// v5 attendance: what unanswered members count as (opt-in for v4
    /// timings).
    pub attendance_default: AttendanceDefault,
    /// v5 attendance: members' "always in", by user id; only party members.
    pub standing: Vec<StandingAnswer>,
    /// v5 (user decision 2026-10-09): staff chose `owner_id`. Unpinned, the
    /// owner is the first participant.
    pub owner_pinned: bool,
}

impl FixedRun {
    /// Who owns the timing: the pinned owner, else the first participant
    /// (the stored owner only for an empty party).
    pub fn owner(&self) -> &str {
        match self.participants.first() {
            Some(first) if !self.owner_pinned => first,
            _ => &self.owner_id,
        }
    }
}

/// A new weekly timing before it has an id.
#[derive(Clone, PartialEq, Eq)]
pub struct NewFixedRun {
    pub owner_id: String,
    pub channel_id: Option<String>,
    pub bosses: Vec<String>,
    pub weekday: Weekday,
    pub time: NaiveTime,
    pub participants: Vec<String>,
    pub note: Option<String>,
    /// Staff named the owner explicitly (see [`FixedRun::owner_pinned`]).
    pub owner_pinned: bool,
}

/// The derived shape, with `owner_pinned` only when set: idempotency digests
/// hash this text, so unpinned creates keep their recorded digests.
impl std::fmt::Debug for NewFixedRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("NewFixedRun");
        out.field("owner_id", &self.owner_id)
            .field("channel_id", &self.channel_id)
            .field("bosses", &self.bosses)
            .field("weekday", &self.weekday)
            .field("time", &self.time)
            .field("participants", &self.participants)
            .field("note", &self.note);
        if self.owner_pinned {
            out.field("owner_pinned", &true);
        }
        out.finish()
    }
}

/// Fields a weekly-timing edit sets; `None` leaves a field unchanged, as v4.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FixedRunPatch {
    pub owner_id: Option<String>,
    /// `Some(true)` with a chosen owner, `Some(false)` back to the default.
    pub owner_pinned: Option<bool>,
    pub channel_id: Option<String>,
    pub bosses: Option<Vec<String>>,
    pub weekday: Option<Weekday>,
    pub time: Option<NaiveTime>,
    pub participants: Option<Vec<String>>,
    pub note: Option<String>,
}

/// The derived shape, with `owner_pinned` only when set (digests hash it).
impl std::fmt::Debug for FixedRunPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("FixedRunPatch");
        out.field("owner_id", &self.owner_id);
        if let Some(pinned) = self.owner_pinned {
            out.field("owner_pinned", &pinned);
        }
        out.field("channel_id", &self.channel_id)
            .field("bosses", &self.bosses)
            .field("weekday", &self.weekday)
            .field("time", &self.time)
            .field("participants", &self.participants)
            .field("note", &self.note)
            .finish()
    }
}

/// Weekly-timing fields an edit touched, which decide what reaches its runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FixedField {
    OwnerId,
    ChannelId,
    Bosses,
    Weekday,
    Time,
    Participants,
    Note,
}

impl FixedField {
    /// v4's column name, as notices list touched fields.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnerId => "owner_id",
            Self::ChannelId => "channel_id",
            Self::Bosses => "bosses",
            Self::Weekday => "weekday",
            Self::Time => "time",
            Self::Participants => "participants",
            Self::Note => "note",
        }
    }
}

/// A concrete run (`runs` row); instants are stored in UTC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub fixed_run_id: Option<String>,
    pub channel_id: Option<String>,
    pub week_start: DateTime<Utc>,
    pub datetime: DateTime<Utc>,
    pub bosses: Vec<String>,
    pub participants: Vec<String>,
    pub status: RunStatus,
    pub source: RunSource,
    /// v5 attendance recorded on a done run, by user id.
    pub attendance: Vec<AttendanceRecord>,
    /// v5: a hand-set status derivation keeps (never written in v4-compat
    /// mode).
    pub status_pin: Option<StatusPin>,
}

/// A new run before it has an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRun {
    pub fixed_run_id: Option<String>,
    pub channel_id: Option<String>,
    pub week_start: DateTime<Utc>,
    pub datetime: DateTime<Utc>,
    pub bosses: Vec<String>,
    pub participants: Vec<String>,
    pub status: RunStatus,
    pub source: RunSource,
}

/// One member's RSVP for one run, keyed by `(run_id, user_id)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rsvp {
    pub run_id: String,
    pub user_id: String,
    pub state: RsvpState,
    pub source: RsvpSource,
    pub at: DateTime<Utc>,
}

/// A reminder row; `(run_id, kind)` is unique.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub id: String,
    pub run_id: String,
    pub kind: String,
    pub fire_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub message_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_and_unknown_text_keeps_v4_message() {
        for status in RunStatus::ALL {
            assert_eq!(RunStatus::parse(status.as_str()), Ok(*status));
        }
        let error = RunStatus::parse("archived").unwrap_err();
        assert_eq!(error.to_string(), "unknown run status 'archived'");
        let error = RsvpState::parse("unknown").unwrap_err();
        assert_eq!(error.to_string(), "unknown rsvp state 'unknown'");
    }

    /// User decision 2026-10-09: the first participant owns a timing unless
    /// staff pinned an owner; the stored owner stands in only for no party.
    #[test]
    fn a_timing_is_owned_by_its_first_participant_unless_pinned() {
        let mut timing = FixedRun {
            id: "f".into(),
            owner_id: "creator".into(),
            channel_id: None,
            bosses: Vec::new(),
            weekday: Weekday::Tue,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: vec!["first".into(), "second".into()],
            note: None,
            attendance_default: AttendanceDefault::OptIn,
            standing: Vec::new(),
            owner_pinned: false,
        };
        assert_eq!(timing.owner(), "first");
        timing.participants.reverse();
        assert_eq!(timing.owner(), "second", "it follows the party's order");
        timing.participants.clear();
        assert_eq!(
            timing.owner(),
            "creator",
            "an empty party keeps the stored owner"
        );
        timing.participants = vec!["first".into()];
        timing.owner_pinned = true;
        assert_eq!(
            timing.owner(),
            "creator",
            "a pinned owner holds, on the party or not"
        );
    }
}
