//! Turning the model's literal `day_ref`/`time_ref` into a date and time (v4
//! `bot/extract/resolve.py`), against the latest evidence message's timestamp.
//!
//! Chat is parsed here rather than by a general date parser because guessing
//! wrong silently moves a run: anything unrecognised resolves to nothing and
//! the card says TBD. A bare hour 1-11 means pm and a bare 12 means midnight
//! ([`PM_CUTOFF`]), unless written as a leading-zero four-digit time (`0930`);
//! this is deliberately not `weeks::parse_hhmm`, which reads typed
//! slash-command input as 24-hour. "after"/"before" make the reference a
//! [`Bound`] rather than a target.

mod bound;
mod clock;
mod day;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};
use chrono_tz::Tz;

pub use bound::{Bound, Side};
pub use clock::{PM_CUTOFF, parse_clock};
pub use day::WEEKDAY_ALIASES;

use crate::domain::time::{AwareDateTime, DateOutOfRange, ZonedDateTime};

/// What could be pinned down. `at` is set only when both a day and a clock
/// time are known, so a day-only reference reads "Wed — time TBD".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub day: Option<NaiveDate>,
    pub clock: Option<NaiveTime>,
    pub at: Option<ZonedDateTime>,
    /// A bare hour was read as pm.
    pub assumed_pm: bool,
    /// "after"/"before" made this a bound on the run, not (only) a target;
    /// the fields above keep the v4 target reading for kinds that ignore it.
    pub bound: Option<Bound>,
}

impl Resolved {
    pub fn known(&self) -> bool {
        self.day.is_some() || self.clock.is_some()
    }
}

fn later(wall: NaiveDateTime, days: i64) -> Result<NaiveDateTime, DateOutOfRange> {
    wall.checked_add_signed(TimeDelta::days(days))
        .ok_or(DateOutOfRange)
}

/// Resolve a day/time reference against `anchor`, viewed in `zone`.
///
/// A time with no day is today, rolling to tomorrow once past; a vague day
/// ("later") rolls a day and a bare weekday a week, while an explicit day
/// ("today", "tmr", a date such as "17th") never rolls. Comparisons and rolls
/// are on the wall clock, as v4 compares datetimes sharing a zone.
///
/// # Errors
/// [`DateOutOfRange`] outside years 1..=9999.
pub fn resolve(
    day_ref: Option<&str>,
    time_ref: Option<&str>,
    anchor: &impl AwareDateTime,
    zone: Tz,
) -> Result<Resolved, DateOutOfRange> {
    let local = anchor.astimezone(zone)?.wall();
    let clock = parse_clock(time_ref);
    let day = day::parse_day(day_ref, local.date())?;
    let bound = bound::parse_side(day_ref, time_ref)
        .map(|side| Bound {
            side,
            day: day.as_ref().map(|day| day.day),
            clock: clock.map(|(clock, _)| clock),
        })
        .filter(|bound| bound.day.is_some() || bound.clock.is_some());
    let Some((clock, assumed_pm)) = clock else {
        return Ok(Resolved {
            day: day.map(|day| day.day),
            bound,
            ..Resolved::default()
        });
    };

    let mut candidate = match &day {
        Some(day) => day.day.and_time(clock),
        None => local.date().and_time(clock),
    };
    if candidate < local {
        match day {
            None => candidate = later(candidate, 1)?,
            // "later at 11" past 23:00, or "wed 9:30pm" said late on a Wednesday.
            Some(day) if !day.explicit => {
                let days = if day::names_weekday(day_ref) { 7 } else { 1 };
                candidate = later(candidate, days)?;
            }
            Some(_) => {}
        }
    }
    let at = ZonedDateTime::new(candidate, zone)?;
    Ok(Resolved {
        day: Some(candidate.date()),
        clock: Some(clock),
        at: Some(at),
        assumed_pm,
        bound,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use chrono_tz::Asia::Kuala_Lumpur;

    use super::*;

    /// Thu 15 Oct 2026 18:57 in Kuala Lumpur.
    fn anchor() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, 10, 57, 0).unwrap()
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    fn read(day: Option<&str>, time: Option<&str>) -> Resolved {
        resolve(day, time, &anchor(), Kuala_Lumpur).unwrap()
    }

    #[test]
    fn after_0930_on_the_17th_is_a_bound_on_saturday_morning() {
        let resolved = read(Some("17th"), Some("after 0930"));
        assert_eq!(resolved.day, Some(date(17)));
        assert_eq!(resolved.clock, Some(time(9, 30)));
        assert!(!resolved.assumed_pm);
        assert_eq!(
            resolved.bound,
            Some(Bound {
                side: Side::After,
                day: Some(date(17)),
                clock: Some(time(9, 30)),
            })
        );
    }

    #[test]
    fn a_bound_keeps_only_what_was_said() {
        // Only a day: no clock, and the target fields keep v4's reading.
        let day_only = read(Some("after 17th"), None);
        assert_eq!(day_only.day, Some(date(17)));
        assert_eq!(
            day_only.bound,
            Some(Bound {
                side: Side::After,
                day: Some(date(17)),
                clock: None,
            })
        );
        // Only a time: the target gets today's implicit day, the bound none.
        let time_only = read(None, Some("before 10"));
        assert_eq!(time_only.day, Some(date(15)));
        assert_eq!(
            time_only.bound,
            Some(Bound {
                side: Side::Before,
                day: None,
                clock: Some(time(22, 0)),
            })
        );
        // Nothing known is no bound.
        assert_eq!(read(None, Some("after boss")).bound, None);
        assert_eq!(read(Some("17th"), Some("9pm")).bound, None);
    }
}
