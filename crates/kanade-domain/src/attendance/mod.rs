//! The attendance model (v5): standing answers, a per-timing default,
//! derived answer states, status and ping rules, tallies and recorded
//! attendance. Pure rules only; storage and the write path build on these.
//! Contract: `docs/notes/attendance.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};

use crate::schedule::{FixedRun, RsvpState, Run, RunStatus, ScheduleSnapshot};

/// Attendance settings of a schedule policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttendancePolicy {
    /// Unknown members make a run at risk this long (elapsed time, not
    /// wall-clock) before it starts. Default 12 h.
    pub unknown_window: TimeDelta,
}

impl AttendancePolicy {
    pub const DEFAULT_UNKNOWN_WINDOW: TimeDelta = TimeDelta::hours(12);

    pub const V5: Self = Self {
        unknown_window: Self::DEFAULT_UNKNOWN_WINDOW,
    };
}

spelled! {
    /// A weekly timing's default for members who did not answer. One-off runs
    /// are always opt-in; timings imported from v4 start opt-in.
    #[derive(Default)]
    AttendanceDefault {
        #[default]
        OptIn => "opt_in",
        AssumeComing => "assume_coming",
    }
}

/// A member's "always in" for a weekly timing (stored with the timing).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StandingAnswer {
    pub user_id: String,
    /// Who set it, `<kind>:<id>` (the member themselves or an admin).
    pub set_by: String,
    pub at: DateTime<Utc>,
}

/// Why an answer is assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssumedSource {
    /// The member's "always in" for the timing.
    Standing,
    /// The timing assumes everyone is coming.
    Default,
}

/// A participant's answer for one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnswerState {
    /// An explicit ✅.
    Confirmed,
    Assumed(AssumedSource),
    /// No answer on an opt-in run.
    Unknown,
    /// An explicit ❌.
    Declined,
}

/// What is known about one participant for one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnswerFacts {
    /// Their explicit reaction, if any (removing it reverts to the rest).
    pub explicit: Option<RsvpState>,
    /// They set "always in" for the run's timing.
    pub standing: bool,
    /// The run's timing default; `None` for a one-off run (always opt-in).
    pub default: Option<AttendanceDefault>,
}

/// THE derivation: an explicit reaction always wins; then a standing
/// answer; then an assume-coming timing; otherwise unknown.
pub fn answer_state(facts: AnswerFacts) -> AnswerState {
    match facts.explicit {
        Some(RsvpState::Yes) => return AnswerState::Confirmed,
        Some(RsvpState::No) => return AnswerState::Declined,
        _ => {}
    }
    if facts.standing {
        AnswerState::Assumed(AssumedSource::Standing)
    } else if facts.default == Some(AttendanceDefault::AssumeComing) {
        AnswerState::Assumed(AssumedSource::Default)
    } else {
        AnswerState::Unknown
    }
}

/// Every participant's state for one run, in party order.
pub fn run_states(
    participants: &[String],
    explicit: &BTreeMap<String, RsvpState>,
    standing: &BTreeSet<String>,
    default: Option<AttendanceDefault>,
) -> Vec<(String, AnswerState)> {
    participants
        .iter()
        .map(|user| {
            let facts = AnswerFacts {
                explicit: explicit.get(user).copied(),
                standing: standing.contains(user),
                default,
            };
            (user.clone(), answer_state(facts))
        })
        .collect()
}

/// Every participant's state for `run`, given its explicit answers and its
/// weekly timing (`None` for a one-off run or a retired timing: opt-in, no
/// standing answers).
pub fn states_of(
    run: &Run,
    explicit: &BTreeMap<String, RsvpState>,
    timing: Option<&FixedRun>,
) -> Vec<(String, AnswerState)> {
    let standing: BTreeSet<String> = timing
        .map(|timing| {
            timing
                .standing
                .iter()
                .map(|answer| answer.user_id.clone())
                .collect()
        })
        .unwrap_or_default();
    run_states(
        &run.participants,
        explicit,
        &standing,
        timing.map(|timing| timing.attendance_default),
    )
}

/// [`states_of`] over a loaded snapshot.
pub fn snapshot_states(snapshot: &ScheduleSnapshot, run: &Run) -> Vec<(String, AnswerState)> {
    let explicit: BTreeMap<String, RsvpState> = snapshot
        .rsvps
        .iter()
        .filter(|row| row.run_id == run.id)
        .map(|row| (row.user_id.clone(), row.state))
        .collect();
    let timing = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| snapshot.fixed_runs.iter().find(|row| row.id == id));
    states_of(run, &explicit, timing)
}

/// Counts for cards, digest, board and API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub confirmed: usize,
    pub assumed: usize,
    pub unknown: usize,
    pub declined: usize,
    pub total: usize,
}

impl Tally {
    pub fn of<'a>(states: impl IntoIterator<Item = &'a AnswerState>) -> Self {
        let mut tally = Self::default();
        for state in states {
            tally.total += 1;
            match state {
                AnswerState::Confirmed => tally.confirmed += 1,
                AnswerState::Assumed(_) => tally.assumed += 1,
                AnswerState::Unknown => tally.unknown += 1,
                AnswerState::Declined => tally.declined += 1,
            }
        }
        tally
    }
}

/// `4/4 (2 assumed)`: coming (confirmed or assumed) over the party, then
/// the assumed and declined counts when there are any.
impl fmt::Display for Tally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.confirmed + self.assumed, self.total)?;
        let mut notes = Vec::new();
        if self.assumed > 0 {
            notes.push(format!("{} assumed", self.assumed));
        }
        if self.declined > 0 {
            notes.push(format!("{} out", self.declined));
        }
        if notes.is_empty() {
            Ok(())
        } else {
            write!(f, " ({})", notes.join(", "))
        }
    }
}

/// A run's derived status, and whether a `Confirmed` rests on any assumed
/// answer (the UI says "expected" rather than "confirmed").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DerivedStatus {
    pub status: RunStatus,
    pub expected: bool,
}

fn is_sticky(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Cancelled | RunStatus::Otot | RunStatus::Done
    )
}

/// Derive a run's status from its participants' states.
///
/// Sticky statuses are kept. At risk from an explicit ❌ or from unknown
/// members once `now` is within `unknown_window` of the start. Confirmed
/// when every participant is confirmed or assumed (`expected` if any is
/// assumed); otherwise planned: status is fully derived, so a confirmation
/// resting on withdrawn assumptions does not linger.
pub fn derive_status(
    current: RunStatus,
    states: &[AnswerState],
    start: DateTime<Utc>,
    now: DateTime<Utc>,
    policy: AttendancePolicy,
) -> DerivedStatus {
    let plain = |status| DerivedStatus {
        status,
        expected: false,
    };
    if is_sticky(current) {
        return plain(current);
    }
    if states.contains(&AnswerState::Declined) {
        return plain(RunStatus::AtRisk);
    }
    if states.contains(&AnswerState::Unknown) && now >= start - policy.unknown_window {
        return plain(RunStatus::AtRisk);
    }
    let coming =
        |state: &AnswerState| matches!(state, AnswerState::Confirmed | AnswerState::Assumed(_));
    if !states.is_empty() && states.iter().all(coming) {
        return DerivedStatus {
            status: RunStatus::Confirmed,
            expected: states
                .iter()
                .any(|state| matches!(state, AnswerState::Assumed(_))),
        };
    }
    plain(RunStatus::Planned)
}

/// The morning ping mentions only members whose answer is unknown.
pub fn morning_mentions(states: &[(String, AnswerState)]) -> Vec<String> {
    states
        .iter()
        .filter(|(_, state)| *state == AnswerState::Unknown)
        .map(|(user, _)| user.clone())
        .collect()
}

/// Countdown pings keep v4: everyone who has not declined.
pub fn countdown_mentions(states: &[(String, AnswerState)]) -> Vec<String> {
    states
        .iter()
        .filter(|(_, state)| *state != AnswerState::Declined)
        .map(|(user, _)| user.clone())
        .collect()
}

/// One member's recorded attendance on a done run (stored with the run,
/// `Run.attendance`, sorted by user).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttendanceRecord {
    pub user_id: String,
    pub attended: bool,
    /// Who recorded it, `<kind>:<id>`.
    pub recorded_by: String,
    pub at: DateTime<Utc>,
}

/// v5: a status an administrator set by hand (planned or confirmed),
/// kept by derivation until an explicit answer or party change before the
/// start, a move, another hand-set status or the run leaving the live
/// statuses clears it. Who set
/// it is the blame of the run's `status_pin` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StatusPin {
    pub status: RunStatus,
    pub at: DateTime<Utc>,
}

/// How a card qualifies a run's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLabel {
    /// Confirmed on assumed answers.
    Expected,
    /// Set by hand and pinned, e.g. "confirmed (set by admin)".
    SetByAdmin(RunStatus),
}

impl fmt::Display for StatusLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expected => f.write_str("expected"),
            Self::SetByAdmin(status) => write!(f, "{} (set by admin)", status.as_str()),
        }
    }
}

/// The label a card shows beside a run's tally: a pin wins over
/// "expected".
pub fn status_label(
    status: RunStatus,
    pin: Option<StatusPin>,
    tally: &Tally,
) -> Option<StatusLabel> {
    match pin {
        Some(pin) if pin.status == status => Some(StatusLabel::SetByAdmin(status)),
        _ if status == RunStatus::Confirmed && tally.assumed > 0 => Some(StatusLabel::Expected),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn facts(
        explicit: Option<RsvpState>,
        standing: bool,
        default: Option<AttendanceDefault>,
    ) -> AnswerFacts {
        AnswerFacts {
            explicit,
            standing,
            default,
        }
    }

    #[test]
    fn the_state_truth_table() {
        use AnswerState::*;
        use AssumedSource::*;
        let opt_in = Some(AttendanceDefault::OptIn);
        let assume = Some(AttendanceDefault::AssumeComing);
        let yes = Some(RsvpState::Yes);
        let no = Some(RsvpState::No);
        let cases = [
            // An explicit reaction always wins.
            (facts(yes, true, assume), Confirmed),
            (facts(no, true, assume), Declined),
            (facts(yes, false, None), Confirmed),
            (facts(no, false, opt_in), Declined),
            // Then a standing answer, then the timing default.
            (facts(None, true, opt_in), Assumed(Standing)),
            (facts(None, true, assume), Assumed(Standing)),
            (facts(None, false, assume), Assumed(Default)),
            (facts(None, false, opt_in), Unknown),
            // One-off runs are opt-in, but a standing answer is per timing
            // and cannot apply there; the default never does.
            (facts(None, false, None), Unknown),
        ];
        for (input, expected) in cases {
            assert_eq!(answer_state(input), expected, "{input:?}");
        }
        // Removing a reaction reverts to the assumed or unknown state.
        assert_eq!(answer_state(facts(None, true, None)), Assumed(Standing));
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 5, hour, 0, 0).unwrap()
    }

    #[test]
    fn v5_status_rules() {
        use AnswerState::*;
        let policy = AttendancePolicy::V5;
        let start = at(20);
        let early = start - TimeDelta::hours(13);
        let late = start - TimeDelta::hours(12);
        let derive = |states: &[AnswerState], now| {
            derive_status(RunStatus::Planned, states, start, now, policy)
        };
        // Unknowns are fine until the window opens, then at risk.
        assert_eq!(
            derive(&[Confirmed, Unknown], early).status,
            RunStatus::Planned
        );
        assert_eq!(
            derive(&[Confirmed, Unknown], late).status,
            RunStatus::AtRisk
        );
        // An explicit ❌ is at risk at any time.
        assert_eq!(
            derive(&[Declined, Assumed(AssumedSource::Standing)], early).status,
            RunStatus::AtRisk
        );
        // Everyone coming: confirmed, "expected" when any is assumed.
        assert_eq!(
            derive(&[Confirmed, Confirmed], early),
            DerivedStatus {
                status: RunStatus::Confirmed,
                expected: false
            }
        );
        assert_eq!(
            derive(&[Confirmed, Assumed(AssumedSource::Default)], late),
            DerivedStatus {
                status: RunStatus::Confirmed,
                expected: true
            }
        );
        // No sticky confirmation: withdrawn assumptions un-confirm.
        let unknown = [Confirmed, Unknown];
        assert_eq!(
            derive_status(RunStatus::Confirmed, &unknown, start, early, policy).status,
            RunStatus::Planned
        );
        // Sticky statuses are kept.
        assert_eq!(
            derive_status(RunStatus::Otot, &[Declined], start, late, policy).status,
            RunStatus::Otot
        );
    }

    #[test]
    fn the_unknown_window_is_elapsed_time_across_dst() {
        // London springs forward at 01:00 UTC on 29 Mar 2026: a run at
        // 10:00 BST (09:00 UTC) opens its 12 h window at 21:00 UTC the day
        // before, 12 real hours earlier, not 12 wall-clock hours.
        let start = Utc.with_ymd_and_hms(2026, 3, 29, 9, 0, 0).unwrap();
        let states = [AnswerState::Unknown];
        let before = Utc.with_ymd_and_hms(2026, 3, 28, 20, 59, 0).unwrap();
        let opened = Utc.with_ymd_and_hms(2026, 3, 28, 21, 0, 0).unwrap();
        let derive = |now| {
            derive_status(
                RunStatus::Planned,
                &states,
                start,
                now,
                AttendancePolicy::V5,
            )
        };
        assert_eq!(derive(before).status, RunStatus::Planned);
        assert_eq!(derive(opened).status, RunStatus::AtRisk);
    }

    #[test]
    fn pings_and_tallies() {
        use AnswerState::*;
        let states = vec![
            ("1".to_owned(), Confirmed),
            ("2".to_owned(), Assumed(AssumedSource::Standing)),
            ("3".to_owned(), Unknown),
            ("4".to_owned(), Declined),
        ];
        assert_eq!(morning_mentions(&states), ["3"]);
        assert_eq!(countdown_mentions(&states), ["1", "2", "3"]);
        let tally = Tally::of(states.iter().map(|(_, state)| state));
        assert_eq!(
            tally,
            Tally {
                confirmed: 1,
                assumed: 1,
                unknown: 1,
                declined: 1,
                total: 4
            }
        );
        assert_eq!(tally.to_string(), "2/4 (1 assumed, 1 out)");
        let full = [
            Confirmed,
            Confirmed,
            Assumed(AssumedSource::Default),
            Assumed(AssumedSource::Standing),
        ];
        assert_eq!(Tally::of(&full).to_string(), "4/4 (2 assumed)");
        assert_eq!(Tally::of(&[Confirmed]).to_string(), "1/1");
    }

    #[test]
    fn attendance_defaults_parse() {
        assert_eq!(
            AttendanceDefault::parse("assume_coming"),
            Some(AttendanceDefault::AssumeComing)
        );
    }
}
