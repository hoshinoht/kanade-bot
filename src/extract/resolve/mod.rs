//! Turning the model's literal `day_ref`/`time_ref` into a date and time (v4
//! `bot/extract/resolve.py`), against the latest evidence message's timestamp.
//!
//! Chat is parsed here rather than by a general date parser because guessing
//! wrong silently moves a run: anything unrecognised resolves to nothing and
//! the card says TBD. A bare hour 1-11 means pm and a bare 12 means midnight
//! ([`PM_CUTOFF`]); this is deliberately not `weeks::parse_hhmm`, which reads
//! typed slash-command input as 24-hour.

mod clock;
mod day;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};
use chrono_tz::Tz;

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
/// ("today", "tmr", a date) never rolls. Comparisons and rolls are on the wall
/// clock, as v4 compares datetimes sharing a zone.
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
    let Some((clock, assumed_pm)) = clock else {
        return Ok(Resolved {
            day: day.map(|day| day.day),
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
    })
}
