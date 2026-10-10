//! When a run is over (user decision 2026-10-10, "Frozen, shown as ended"):
//! its start plus its length, as the completion prompt plans it; an own-time
//! run at the boss reset. A live run past its end is frozen until its prompt
//! is answered or its cutoff marks it done: settling works, edits do not, and
//! every view reads it as ended. The stored status never changes for it.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};

use super::plan::{plan, run_minutes};
use crate::domain::catalog::BossTable;
use crate::domain::schedule::{Run, RunStatus, SchedulePolicy};
use crate::domain::settings::RunLengths;
use crate::domain::time::DateOutOfRange;

/// The run lengths and boss reset every "is it over?" reads, at one moment.
#[derive(Clone, Debug)]
pub struct RunEnds {
    lengths: RunLengths,
    catalog: Option<Arc<BossTable>>,
    policy: SchedulePolicy,
}

impl RunEnds {
    /// `catalog` prices bosses (`RunLengths::minutes_for`); without one every
    /// boss takes the default length, as the completion prompt does.
    pub fn new(
        lengths: RunLengths,
        catalog: Option<Arc<BossTable>>,
        policy: SchedulePolicy,
    ) -> Self {
        Self {
            lengths,
            catalog,
            policy,
        }
    }

    /// A run of `bosses` lasts this many minutes (the API's `minutes`).
    pub fn minutes(&self, bosses: &[String]) -> u32 {
        run_minutes(&self.lengths, self.catalog.as_deref(), bosses)
    }

    /// When `run` is over: [`plan`]'s `ends_at`, or for an own-time run its
    /// cutoff, the boss reset (so it stays editable all week).
    ///
    /// # Errors
    /// [`DateOutOfRange`] when an instant leaves years 1..=9999.
    pub fn ends_at(&self, run: &Run) -> Result<DateTime<Utc>, DateOutOfRange> {
        let plan = plan(run, self.minutes(&run.bosses), &self.policy)?;
        Ok(if run.status == RunStatus::Otot {
            plan.cutoff_at
        } else {
            plan.ends_at
        })
    }

    /// Has `run` ended by `now`? An end out of range never comes.
    pub fn ended(&self, run: &Run, now: DateTime<Utc>) -> bool {
        self.ends_at(run).is_ok_and(|end| end <= now)
    }

    /// A live run whose end has passed: frozen and shown as ended.
    pub fn frozen(&self, run: &Run, now: DateTime<Utc>) -> bool {
        run.status.is_live() && self.ended(run, now)
    }

    /// Whether a weekly slot at `at` for `bosses` (a planned run, not yet
    /// created) would already have ended: materialisation skips it.
    pub fn slot_ended(&self, at: DateTime<Utc>, bosses: &[String], now: DateTime<Utc>) -> bool {
        at.checked_add_signed(TimeDelta::minutes(i64::from(self.minutes(bosses))))
            .is_some_and(|end| end <= now)
    }
}

/// Reads the live `v5.run_lengths` (serve: the settings watch).
pub type RunLengthsNow = Arc<dyn Fn() -> RunLengths + Send + Sync>;

/// Where a long-lived writer or view takes its [`RunEnds`] from: the live
/// run lengths with the fixed catalog and schedule policy.
#[derive(Clone)]
pub struct RunEndsSource {
    lengths: RunLengthsNow,
    catalog: Option<Arc<BossTable>>,
    policy: SchedulePolicy,
}

impl RunEndsSource {
    pub fn new(
        lengths: RunLengthsNow,
        catalog: Option<Arc<BossTable>>,
        policy: SchedulePolicy,
    ) -> Self {
        Self {
            lengths,
            catalog,
            policy,
        }
    }

    /// Lengths that never change (tests, one tick).
    pub fn fixed(
        lengths: RunLengths,
        catalog: Option<Arc<BossTable>>,
        policy: SchedulePolicy,
    ) -> Self {
        Self::new(Arc::new(move || lengths.clone()), catalog, policy)
    }

    /// The rules as they stand now.
    pub fn now(&self) -> RunEnds {
        RunEnds::new((self.lengths)(), self.catalog.clone(), self.policy.clone())
    }
}

impl std::fmt::Debug for RunEndsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunEndsSource")
            .field("catalog", &self.catalog.is_some())
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeZone, Weekday};

    use super::*;
    use crate::domain::schedule::{ReminderPolicy, RunSource};

    /// Kuala Lumpur (UTC+8), boss reset Wednesday 00:00 local (Tue 16:00Z).
    fn ends() -> RunEnds {
        let policy = SchedulePolicy::new(
            ReminderPolicy {
                zone: chrono_tz::Asia::Kuala_Lumpur,
                ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            Weekday::Wed,
            NaiveTime::MIN,
        );
        RunEnds::new(RunLengths::default(), None, policy)
    }

    fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
    }

    fn run(status: RunStatus, bosses: &[&str]) -> Run {
        Run {
            id: "r-1".into(),
            fixed_run_id: None,
            channel_id: None,
            week_start: utc(8, 16, 0),
            datetime: utc(10, 12, 0),
            bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
            participants: vec!["1001".into()],
            status,
            source: RunSource::Fixed,
            attendance: Vec::new(),
            status_pin: None,
        }
    }

    #[test]
    fn a_run_ends_at_its_start_plus_its_length() {
        let minutes = RunLengths::default().default_minutes;
        let run = run(RunStatus::Planned, &["Kalos", "Kaling"]);
        let end = utc(10, 12, 0) + TimeDelta::minutes(i64::from(minutes * 2));
        assert_eq!(ends().ends_at(&run), Ok(end));
        assert!(!ends().frozen(&run, end - TimeDelta::seconds(1)));
        assert!(ends().frozen(&run, end));
    }

    #[test]
    fn an_own_time_run_ends_at_the_reset() {
        let run = run(RunStatus::Otot, &["Kalos"]);
        assert_eq!(ends().ends_at(&run), Ok(utc(15, 16, 0)));
        assert!(!ends().frozen(&run, utc(15, 15, 59)));
        assert!(ends().frozen(&run, utc(15, 16, 0)));
    }

    #[test]
    fn a_settled_run_is_never_frozen() {
        for status in [RunStatus::Done, RunStatus::Cancelled] {
            assert!(!ends().frozen(&run(status, &["Kalos"]), utc(20, 0, 0)));
        }
    }
}
