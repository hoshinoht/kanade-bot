//! The attendance model (v5): standing answers, a per-timing default,
//! derived answer states, status and ping rules, tallies and recorded
//! attendance. Pure rules only; storage and the write path build on these.
//! Contract: `docs/v5/attendance.md`.
//!
//! In [`AttendanceMode::V4Compat`] there are no standing answers and every
//! timing is opt-in, so every rule reduces exactly to v4's.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};

use crate::domain::schedule::{FixedRun, RsvpState, Run, RunStatus, ScheduleSnapshot};

/// Whether the v5 attendance rules apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttendanceMode {
    /// v4 exactly: no standing answers, every timing opt-in, status from
    /// explicit answers only.
    V4Compat,
    V5,
}

/// Attendance settings of a schedule policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttendancePolicy {
    pub mode: AttendanceMode,
    /// Unknown members make a run at risk this long (elapsed time, not
    /// wall-clock) before it starts. Default 12 h.
    pub unknown_window: TimeDelta,
}

impl AttendancePolicy {
    pub const DEFAULT_UNKNOWN_WINDOW: TimeDelta = TimeDelta::hours(12);

    /// v4 behaviour, for replaying frozen vectors.
    pub const V4_COMPAT: Self = Self {
        mode: AttendanceMode::V4Compat,
        unknown_window: Self::DEFAULT_UNKNOWN_WINDOW,
    };

    pub const V5: Self = Self {
        mode: AttendanceMode::V5,
        unknown_window: Self::DEFAULT_UNKNOWN_WINDOW,
    };
}

/// A weekly timing's default for members who did not answer. One-off runs
/// are always opt-in; timings imported from v4 start opt-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AttendanceDefault {
    #[default]
    OptIn,
    AssumeComing,
}

impl AttendanceDefault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OptIn => "opt_in",
            Self::AssumeComing => "assume_coming",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::OptIn, Self::AssumeComing]
            .into_iter()
            .find(|default| default.as_str() == value)
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
/// answer; then an assume-coming timing; otherwise unknown. In v4-compat
/// mode only explicit answers count.
pub fn answer_state(facts: AnswerFacts, mode: AttendanceMode) -> AnswerState {
    match facts.explicit {
        Some(RsvpState::Yes) => return AnswerState::Confirmed,
        Some(RsvpState::No) => return AnswerState::Declined,
        _ => {}
    }
    if mode == AttendanceMode::V4Compat {
        return AnswerState::Unknown;
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
    mode: AttendanceMode,
) -> Vec<(String, AnswerState)> {
    participants
        .iter()
        .map(|user| {
            let facts = AnswerFacts {
                explicit: explicit.get(user).copied(),
                standing: standing.contains(user),
                default,
            };
            (user.clone(), answer_state(facts, mode))
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
    mode: AttendanceMode,
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
        mode,
    )
}

/// [`states_of`] over a loaded snapshot.
pub fn snapshot_states(
    snapshot: &ScheduleSnapshot,
    run: &Run,
    mode: AttendanceMode,
) -> Vec<(String, AnswerState)> {
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
    states_of(run, &explicit, timing, mode)
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
/// Sticky statuses are kept. At risk only from an explicit ❌ or, in v5
/// mode, from unknown members once `now` is within `unknown_window` of the
/// start. Confirmed when every participant is confirmed or assumed
/// (`expected` if any is assumed); otherwise planned: in v5 status is fully
/// derived, while v4 keeps a confirmation through silence. In v4-compat
/// mode this is exactly v4's
/// [`compute_status`](crate::domain::schedule::compute_status).
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
    if policy.mode == AttendanceMode::V5
        && states.contains(&AnswerState::Unknown)
        && now >= start - policy.unknown_window
    {
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
    // v4 keeps a confirmation through silence; v5 derives status fully, so
    // a confirmation resting on withdrawn assumptions does not linger.
    plain(
        if policy.mode == AttendanceMode::V4Compat && current == RunStatus::Confirmed {
            RunStatus::Confirmed
        } else {
            RunStatus::Planned
        },
    )
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

/// Who recorded attendance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttendanceActor {
    Admin,
    Member(String),
}

/// Why an attendance record was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttendanceRefusal {
    /// Attendance is recorded at or after marking the run done.
    NotDone,
    /// A member may confirm only their own attendance.
    OthersNotAllowed,
    /// A member not on the run.
    NotOnRun(String),
}

impl fmt::Display for AttendanceRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDone => f.write_str("attendance is recorded once the run is done"),
            Self::OthersNotAllowed => f.write_str("members may confirm only their own attendance"),
            Self::NotOnRun(user) => write!(f, "{user} is not on the run"),
        }
    }
}

impl std::error::Error for AttendanceRefusal {}

/// The roster pre-ticked for one-tap correction: everyone not declined.
pub fn prefill(states: &[(String, AnswerState)]) -> BTreeSet<String> {
    countdown_mentions(states).into_iter().collect()
}

/// Check an attendance change: the run is done; attendees are on the run;
/// a member changes only their own entry (`recorded` is what is stored).
///
/// # Errors
/// [`AttendanceRefusal`].
pub fn check_attendance(
    actor: &AttendanceActor,
    status: RunStatus,
    participants: &[String],
    recorded: &BTreeSet<String>,
    attended: &BTreeSet<String>,
) -> Result<(), AttendanceRefusal> {
    if status != RunStatus::Done {
        return Err(AttendanceRefusal::NotDone);
    }
    if let Some(stranger) = attended.iter().find(|user| !participants.contains(user)) {
        return Err(AttendanceRefusal::NotOnRun(stranger.clone()));
    }
    if let AttendanceActor::Member(me) = actor {
        let changed: BTreeSet<&String> = recorded.symmetric_difference(attended).collect();
        if changed.iter().any(|user| *user != me) {
            return Err(AttendanceRefusal::OthersNotAllowed);
        }
    }
    Ok(())
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

/// What stands for each participant now: their recorded entry, else the
/// prefill (everyone not declined).
pub fn recorded_or_prefill(
    states: &[(String, AnswerState)],
    records: &[AttendanceRecord],
) -> BTreeSet<String> {
    states
        .iter()
        .filter(
            |(user, state)| match records.iter().find(|record| &record.user_id == user) {
                Some(record) => record.attended,
                None => *state != AnswerState::Declined,
            },
        )
        .map(|(user, _)| user.clone())
        .collect()
}

/// v5: a status an administrator set by hand (planned or confirmed),
/// kept by derivation until an explicit answer, a party change, another
/// hand-set status or the run leaving the live statuses clears it. Who set
/// it is the blame of the run's `status_pin` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StatusPin {
    pub status: RunStatus,
    pub at: DateTime<Utc>,
}

/// How a card qualifies a run's status (v5 only).
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
/// "expected". `None` in v4-compat mode.
pub fn status_label(
    status: RunStatus,
    pin: Option<StatusPin>,
    tally: &Tally,
    mode: AttendanceMode,
) -> Option<StatusLabel> {
    if mode == AttendanceMode::V4Compat {
        return None;
    }
    match pin {
        Some(pin) if pin.status == status => Some(StatusLabel::SetByAdmin(status)),
        _ if status == RunStatus::Confirmed && tally.assumed > 0 => Some(StatusLabel::Expected),
        _ => None,
    }
}

/// Runs per timing the patterns and the suggestion read ("last 4 runs").
pub const RECENT_RUNS_PER_TIMING: usize = 4;

/// One member's patterns and the timings suggested for "always in".
/// Visible only to administrators and to the member themselves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberAttendance {
    pub member: String,
    /// Their recorded runs, newest first per timing (at most
    /// [`RECENT_RUNS_PER_TIMING`] each).
    pub history: Vec<PastRun>,
    pub patterns: Patterns,
    /// Weekly timings to suggest a standing answer on (never applied).
    pub suggest_standing: Vec<String>,
}

/// One member's past run: what they answered explicitly and whether they
/// attended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastRun {
    pub run_id: String,
    pub fixed_run_id: Option<String>,
    pub at: DateTime<Utc>,
    pub explicit: Option<RsvpState>,
    pub attended: bool,
}

/// A member's attendance patterns. Visible only to administrators and to
/// the member themselves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Patterns {
    pub attended_without_answering: usize,
    pub answered_in_but_absent: usize,
    pub answered_out_but_attended: usize,
}

pub fn attendance_patterns(history: &[PastRun]) -> Patterns {
    let mut patterns = Patterns::default();
    for run in history {
        match (run.explicit, run.attended) {
            (None, true) => patterns.attended_without_answering += 1,
            (Some(RsvpState::Yes), false) => patterns.answered_in_but_absent += 1,
            (Some(RsvpState::No), true) => patterns.answered_out_but_attended += 1,
            _ => {}
        }
    }
    patterns
}

/// Suggest "always in" for a timing when the member attended without
/// answering on at least 3 of their last 4 runs of it (never applied
/// automatically).
pub fn suggest_standing(history: &[PastRun], fixed_run_id: &str) -> bool {
    let mut runs: Vec<&PastRun> = history
        .iter()
        .filter(|run| run.fixed_run_id.as_deref() == Some(fixed_run_id))
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.at));
    runs.iter()
        .take(4)
        .filter(|run| run.explicit.is_none() && run.attended)
        .count()
        >= 3
}

/// A card control setting a standing answer ("✅ every week"). The Discord
/// adapter renders it as a button with this custom id and routes presses
/// back here; nothing is wired yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandingAction {
    pub fixed_run_id: String,
    pub on: bool,
}

impl StandingAction {
    /// `standing:<fixed_run_id>:on|off`.
    pub fn custom_id(&self) -> String {
        format!(
            "standing:{}:{}",
            self.fixed_run_id,
            if self.on { "on" } else { "off" }
        )
    }

    pub fn parse(custom_id: &str) -> Option<Self> {
        let rest = custom_id.strip_prefix("standing:")?;
        let (fixed_run_id, on) = rest.rsplit_once(':')?;
        let on = match on {
            "on" => true,
            "off" => false,
            _ => return None,
        };
        (!fixed_run_id.is_empty()).then(|| Self {
            fixed_run_id: fixed_run_id.to_owned(),
            on,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::schedule::compute_status;

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
        let v5 = AttendanceMode::V5;
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
            assert_eq!(answer_state(input, v5), expected, "{input:?}");
        }
        // v4-compat: explicit answers only.
        assert_eq!(
            answer_state(facts(None, true, assume), AttendanceMode::V4Compat),
            Unknown
        );
        assert_eq!(
            answer_state(facts(yes, true, assume), AttendanceMode::V4Compat),
            Confirmed
        );
        // Removing a reaction reverts to the assumed or unknown state.
        assert_eq!(answer_state(facts(None, true, None), v5), Assumed(Standing));
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 5, hour, 0, 0).unwrap()
    }

    #[test]
    fn v4_compat_status_is_exactly_compute_status() {
        let answers = [None, Some(RsvpState::Yes), Some(RsvpState::No)];
        let statuses = [
            RunStatus::Planned,
            RunStatus::Confirmed,
            RunStatus::AtRisk,
            RunStatus::Cancelled,
            RunStatus::Otot,
            RunStatus::Done,
        ];
        let party = vec!["1".to_owned(), "2".to_owned()];
        for current in statuses {
            for first in answers {
                for second in answers {
                    let mut explicit = BTreeMap::new();
                    for (user, answer) in party.iter().zip([first, second]) {
                        if let Some(answer) = answer {
                            explicit.insert(user.clone(), answer);
                        }
                    }
                    let states: Vec<AnswerState> = run_states(
                        &party,
                        &explicit,
                        &BTreeSet::from(["1".to_owned()]),
                        Some(AttendanceDefault::AssumeComing),
                        AttendanceMode::V4Compat,
                    )
                    .into_iter()
                    .map(|(_, state)| state)
                    .collect();
                    let derived = derive_status(
                        current,
                        &states,
                        at(20),
                        at(19),
                        AttendancePolicy::V4_COMPAT,
                    );
                    assert_eq!(
                        derived.status,
                        compute_status(current, &party, &explicit),
                        "{current:?} {first:?} {second:?}"
                    );
                    assert!(!derived.expected);
                }
            }
        }
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
        // No sticky confirmation in v5: withdrawn assumptions un-confirm.
        let unknown = [Confirmed, Unknown];
        assert_eq!(
            derive_status(RunStatus::Confirmed, &unknown, start, early, policy).status,
            RunStatus::Planned
        );
        assert_eq!(
            derive_status(
                RunStatus::Confirmed,
                &unknown,
                start,
                early,
                AttendancePolicy::V4_COMPAT
            )
            .status,
            RunStatus::Confirmed,
            "v4 keeps it"
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
        assert_eq!(
            prefill(&states),
            BTreeSet::from(["1".into(), "2".into(), "3".into()])
        );
    }

    #[test]
    fn attendance_permissions() {
        let party = vec!["1".to_owned(), "2".to_owned()];
        let recorded = BTreeSet::from(["1".to_owned(), "2".to_owned()]);
        let without_2 = BTreeSet::from(["1".to_owned()]);
        let member = |id: &str| AttendanceActor::Member(id.to_owned());
        assert_eq!(
            check_attendance(
                &AttendanceActor::Admin,
                RunStatus::Planned,
                &party,
                &recorded,
                &recorded
            ),
            Err(AttendanceRefusal::NotDone)
        );
        assert!(
            check_attendance(
                &AttendanceActor::Admin,
                RunStatus::Done,
                &party,
                &recorded,
                &without_2
            )
            .is_ok()
        );
        // A member may change only their own entry.
        assert!(
            check_attendance(&member("2"), RunStatus::Done, &party, &recorded, &without_2).is_ok()
        );
        assert_eq!(
            check_attendance(&member("1"), RunStatus::Done, &party, &recorded, &without_2),
            Err(AttendanceRefusal::OthersNotAllowed)
        );
        assert_eq!(
            check_attendance(
                &AttendanceActor::Admin,
                RunStatus::Done,
                &party,
                &recorded,
                &BTreeSet::from(["9".to_owned()])
            ),
            Err(AttendanceRefusal::NotOnRun("9".into()))
        );
    }

    #[test]
    fn patterns_and_the_suggestion() {
        let run = |day: u32, fixed: &str, explicit, attended| PastRun {
            run_id: format!("r{day}"),
            fixed_run_id: Some(fixed.to_owned()),
            at: Utc.with_ymd_and_hms(2026, 9, day, 20, 0, 0).unwrap(),
            explicit,
            attended,
        };
        let history = vec![
            run(1, "f", None, true),
            run(8, "f", Some(RsvpState::Yes), false),
            run(15, "f", None, true),
            run(22, "f", None, true),
            run(29, "f", Some(RsvpState::No), true),
            run(2, "g", None, true),
        ];
        assert_eq!(
            attendance_patterns(&history),
            Patterns {
                attended_without_answering: 4,
                answered_in_but_absent: 1,
                answered_out_but_attended: 1,
            }
        );
        // Last four runs of f: 29 (answered), 22, 15 (silent, attended),
        // 8 (answered): only 2 silent attendances.
        assert!(!suggest_standing(&history, "f"));
        let mostly_silent = vec![
            run(8, "f", None, true),
            run(15, "f", None, true),
            run(22, "f", Some(RsvpState::Yes), true),
            run(29, "f", None, true),
            run(1, "f", Some(RsvpState::Yes), true),
        ];
        assert!(suggest_standing(&mostly_silent, "f"));
        assert!(!suggest_standing(&mostly_silent, "g"));
    }

    #[test]
    fn standing_actions_round_trip() {
        for on in [true, false] {
            let action = StandingAction {
                fixed_run_id: "abc".into(),
                on,
            };
            assert_eq!(StandingAction::parse(&action.custom_id()), Some(action));
        }
        assert_eq!(StandingAction::parse("standing::on"), None);
        assert_eq!(StandingAction::parse("standing:abc:maybe"), None);
        assert_eq!(
            AttendanceDefault::parse("assume_coming"),
            Some(AttendanceDefault::AssumeComing)
        );
    }
}
