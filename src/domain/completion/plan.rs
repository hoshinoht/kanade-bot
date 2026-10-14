//! When a run's completion prompt is due and when an unanswered run is
//! marked done automatically (user decisions 2026-10-09/10).

use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;

use crate::domain::schedule::{Run, RunStatus, SchedulePolicy, utc_instant};
use crate::domain::time::{AwareDateTime, DateOutOfRange, ZonedDateTime};
use crate::domain::weeks;

/// The prompt posts this long after the run ends.
pub const PROMPT_DELAY: TimeDelta = TimeDelta::minutes(30);
/// "Not yet" asks again this much later.
pub const ASK_AGAIN_AFTER: TimeDelta = TimeDelta::minutes(30);

/// One run's completion timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionPlan {
    /// Its start plus its length.
    pub ends_at: DateTime<Utc>,
    /// When its first prompt is due; `None` for an own-time run and for a
    /// prompt that would fall at or after the boss reset.
    pub prompt_at: Option<DateTime<Utc>>,
    /// When it is marked done unanswered: the earlier of the end of the
    /// guild-local day holding the prompt time and the reset after the run;
    /// the reset itself when there is no prompt.
    pub cutoff_at: DateTime<Utc>,
}

/// A run's length in minutes at `lengths` (`RunLengths::minutes_for`, the
/// API's helper); without a catalog every boss takes the default length.
pub fn run_minutes(
    lengths: &crate::domain::settings::RunLengths,
    catalog: Option<&crate::domain::catalog::BossTable>,
    bosses: &[String],
) -> u32 {
    match catalog {
        Some(catalog) => lengths.minutes_for(catalog, bosses),
        None => lengths
            .default_minutes
            .saturating_mul(u32::try_from(bosses.len()).unwrap_or(u32::MAX)),
    }
}

/// `run`'s timing, `minutes` being its length (`RunLengths::minutes_for`).
///
/// # Errors
/// [`DateOutOfRange`] when an instant leaves years 1..=9999.
pub fn plan(
    run: &Run,
    minutes: u32,
    policy: &SchedulePolicy,
) -> Result<CompletionPlan, DateOutOfRange> {
    let zone = policy.zone();
    let reset = utc_instant(&weeks::week_end(&policy.week_of(&run.datetime)?, zone)?)?;
    let ends_at = run
        .datetime
        .checked_add_signed(TimeDelta::minutes(i64::from(minutes)))
        .ok_or(DateOutOfRange)?;
    let due = ends_at
        .checked_add_signed(PROMPT_DELAY)
        .ok_or(DateOutOfRange)?;
    if run.status == RunStatus::Otot || due >= reset {
        return Ok(CompletionPlan {
            ends_at,
            prompt_at: None,
            cutoff_at: reset,
        });
    }
    Ok(CompletionPlan {
        ends_at,
        prompt_at: Some(due),
        cutoff_at: end_of_local_day(due, zone)?.min(reset),
    })
}

/// The guild-local midnight that ends the day holding `at`.
fn end_of_local_day(at: DateTime<Utc>, zone: Tz) -> Result<DateTime<Utc>, DateOutOfRange> {
    let next = at
        .astimezone(zone)?
        .wall()
        .date()
        .succ_opt()
        .ok_or(DateOutOfRange)?;
    utc_instant(&ZonedDateTime::new(next.and_time(NaiveTime::MIN), zone)?)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Weekday};

    use super::*;
    use crate::domain::schedule::{ReminderPolicy, RunSource};

    /// Kuala Lumpur (UTC+8), boss reset Wednesday 00:00 local.
    fn policy() -> SchedulePolicy {
        SchedulePolicy::new(
            ReminderPolicy {
                zone: chrono_tz::Asia::Kuala_Lumpur,
                ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            Weekday::Wed,
            NaiveTime::MIN,
        )
    }

    fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
    }

    fn run(at: DateTime<Utc>, status: RunStatus) -> Run {
        Run {
            id: "r-1".into(),
            fixed_run_id: None,
            channel_id: None,
            week_start: utc(8, 16, 0),
            datetime: at,
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status,
            source: RunSource::Fixed,
            attendance: Vec::new(),
            status_pin: None,
        }
    }

    #[test]
    fn the_prompt_is_due_half_an_hour_after_the_end_and_cut_off_at_local_midnight() {
        // Thu 20:00 local, 30 minutes: ends 20:30, prompt 21:00, cutoff Fri 00:00.
        let plan = plan(&run(utc(10, 12, 0), RunStatus::Planned), 30, &policy()).unwrap();
        assert_eq!(plan.ends_at, utc(10, 12, 30));
        assert_eq!(plan.prompt_at, Some(utc(10, 13, 0)));
        assert_eq!(plan.cutoff_at, utc(10, 16, 0));
    }

    #[test]
    fn a_prompt_past_midnight_belongs_to_the_next_day() {
        // Thu 23:00 local, 60 minutes: prompt Fri 00:30, cutoff Sat 00:00.
        let plan = plan(&run(utc(10, 15, 0), RunStatus::Confirmed), 60, &policy()).unwrap();
        assert_eq!(plan.prompt_at, Some(utc(10, 16, 30)));
        assert_eq!(plan.cutoff_at, utc(11, 16, 0));
    }

    #[test]
    fn the_reset_cuts_off_and_a_prompt_at_or_after_it_is_skipped() {
        // Tue 22:30 local, 60 minutes: ends 23:30, prompt due Wed 00:00, the
        // reset itself, so it is skipped and the reset cuts the run off.
        let plan = plan(&run(utc(15, 14, 30), RunStatus::Planned), 60, &policy()).unwrap();
        assert_eq!(plan.prompt_at, None);
        assert_eq!(plan.cutoff_at, utc(15, 16, 0));
        // Tue 22:00 local: prompt 23:30, before the reset that cuts it off.
        let earlier = super::plan(&run(utc(15, 14, 0), RunStatus::Planned), 60, &policy()).unwrap();
        assert_eq!(earlier.prompt_at, Some(utc(15, 15, 30)));
        assert_eq!(earlier.cutoff_at, utc(15, 16, 0));
    }

    #[test]
    fn an_own_time_run_has_no_prompt_and_ends_at_the_reset() {
        let plan = plan(&run(utc(10, 12, 0), RunStatus::Otot), 30, &policy()).unwrap();
        assert_eq!(plan.prompt_at, None);
        assert_eq!(plan.cutoff_at, utc(15, 16, 0));
    }

    /// New York (UTC-4 in September), boss reset Thursday 10:00 local.
    fn west() -> SchedulePolicy {
        SchedulePolicy::new(
            ReminderPolicy {
                zone: chrono_tz::America::New_York,
                ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            Weekday::Thu,
            NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
        )
    }

    #[test]
    fn west_of_utc_the_day_ends_at_local_midnight() {
        // Wed 22:00 EDT: prompt 23:00 EDT (Thu 03:00Z), cutoff Thu 00:00 EDT.
        let plan = plan(&run(utc(10, 2, 0), RunStatus::Planned), 30, &west()).unwrap();
        assert_eq!(plan.prompt_at, Some(utc(10, 3, 0)));
        assert_eq!(plan.cutoff_at, utc(10, 4, 0));
    }

    #[test]
    fn a_mid_day_reset_cuts_off_before_midnight_and_a_tie_skips_the_prompt() {
        // Thu 08:00 EDT, 60 minutes: prompt 09:30, cut off by the 10:00 reset.
        let plan = plan(&run(utc(10, 12, 0), RunStatus::Planned), 60, &west()).unwrap();
        assert_eq!(plan.prompt_at, Some(utc(10, 13, 30)));
        assert_eq!(plan.cutoff_at, utc(10, 14, 0));
        // Thu 08:30 EDT: the prompt would be due exactly at the reset.
        let tie = super::plan(&run(utc(10, 12, 30), RunStatus::Planned), 60, &west()).unwrap();
        assert_eq!(tie.prompt_at, None);
        assert_eq!(tie.cutoff_at, utc(10, 14, 0));
    }
}
