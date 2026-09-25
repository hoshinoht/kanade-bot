use chrono::{Datelike, NaiveDateTime, NaiveTime, TimeDelta, Weekday};
use chrono_tz::Tz;

use crate::domain::time::{AwareDateTime, DateOutOfRange, ZonedDateTime};

/// The most recent reset at or before `at`, as a wall clock in `zone`.
///
/// An instant exactly on a reset belongs to the week that reset starts.
///
/// # Errors
/// [`DateOutOfRange`] when the week start leaves years 1..=9999.
pub fn week_start(
    at: &impl AwareDateTime,
    zone: Tz,
    reset_weekday: Weekday,
    reset_time: NaiveTime,
) -> Result<ZonedDateTime, DateOutOfRange> {
    let local = at.astimezone(zone)?.wall();
    let back = days_after(reset_weekday, local.weekday());
    let mut candidate = shift_days(local.date().and_time(reset_time), -back)?;
    // Wall-clock comparison, as Python compares datetimes sharing a zone.
    if candidate > local {
        candidate = shift_days(candidate, -7)?;
    }
    ZonedDateTime::new(candidate, zone)
}

/// The exclusive end of the boss week starting at `week_start`: the same wall
/// clock seven days later, so DST changes do not move the reset.
///
/// # Errors
/// [`DateOutOfRange`] when the end leaves years 1..=9999.
pub fn week_end(
    week_start: &impl AwareDateTime,
    zone: Tz,
) -> Result<ZonedDateTime, DateOutOfRange> {
    ZonedDateTime::new(shift_days(week_start.astimezone(zone)?.wall(), 7)?, zone)
}

/// The current and next two boss-week starts from one captured clock.
///
/// # Errors
/// [`DateOutOfRange`] when any start leaves years 1..=9999.
pub fn materialised_week_starts(
    zone: Tz,
    reset_weekday: Weekday,
    reset_time: NaiveTime,
    now: &impl AwareDateTime,
) -> Result<[ZonedDateTime; 3], DateOutOfRange> {
    let current = week_start(now, zone, reset_weekday, reset_time)?;
    let following = week_end(&current, zone)?;
    Ok([current, following, week_end(&following, zone)?])
}

/// Guild-local Monday midnight at or before `at`.
///
/// # Errors
/// [`DateOutOfRange`] when the Monday leaves years 1..=9999.
pub fn calendar_week_start(
    at: &impl AwareDateTime,
    zone: Tz,
) -> Result<ZonedDateTime, DateOutOfRange> {
    let local = at.astimezone(zone)?.wall();
    let back = i64::from(local.weekday().num_days_from_monday());
    ZonedDateTime::new(
        shift_days(local.date().and_time(NaiveTime::MIN), -back)?,
        zone,
    )
}

/// The exclusive Monday midnight ending the calendar week at `start`.
///
/// # Errors
/// [`DateOutOfRange`] when the end leaves years 1..=9999.
pub fn calendar_week_end(
    start: &impl AwareDateTime,
    zone: Tz,
) -> Result<ZonedDateTime, DateOutOfRange> {
    week_end(start, zone)
}

/// Place a fixed run (e.g. Mon 21:30) inside the boss week starting at
/// `week_start`, pushing a slot that falls before the reset a week forward so the
/// result lies in `[week_start, week_end)`.
///
/// # Errors
/// [`DateOutOfRange`] when the slot leaves years 1..=9999.
pub fn slot_in_week(
    week_start: &impl AwareDateTime,
    zone: Tz,
    weekday: Weekday,
    at: NaiveTime,
) -> Result<ZonedDateTime, DateOutOfRange> {
    let local = week_start.astimezone(zone)?.wall();
    let ahead = days_after(local.weekday(), weekday);
    let mut candidate = shift_days(local.date().and_time(at), ahead)?;
    if candidate < local {
        candidate = shift_days(candidate, 7)?;
    }
    ZonedDateTime::new(candidate, zone)
}

/// Days from `from` forward to `to`, in 0..7.
fn days_after(from: Weekday, to: Weekday) -> i64 {
    i64::from((7 + to.num_days_from_monday() - from.num_days_from_monday()) % 7)
}

fn shift_days(wall: NaiveDateTime, days: i64) -> Result<NaiveDateTime, DateOutOfRange> {
    wall.checked_add_signed(TimeDelta::days(days))
        .ok_or(DateOutOfRange)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, FixedOffset, NaiveDate};
    use chrono_tz::{America::New_York, Asia::Kuala_Lumpur};

    use super::*;
    use crate::domain::time::IsoDateTime;

    fn at(text: &str) -> DateTime<FixedOffset> {
        match IsoDateTime::parse(text).unwrap() {
            IsoDateTime::Aware(at) => at,
            IsoDateTime::Naive(_) => panic!("{text} must be aware"),
        }
    }

    fn hm(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    fn start(text: &str, weekday: Weekday, time: NaiveTime, zone: Tz) -> String {
        week_start(&at(text), zone, weekday, time)
            .unwrap()
            .isoformat()
    }

    #[test]
    fn reset_transition_boundaries() {
        let midnight = hm(0, 0);
        for (instant, expected) in [
            (
                "2026-08-26T23:59:59.999999+08:00",
                "2026-08-20T00:00:00+08:00",
            ),
            ("2026-08-27T00:00:00+08:00", "2026-08-27T00:00:00+08:00"),
            (
                "2026-08-27T00:00:00.000001+08:00",
                "2026-08-27T00:00:00+08:00",
            ),
            ("2026-09-02T23:59:59+08:00", "2026-08-27T00:00:00+08:00"),
            ("2026-08-26T16:00:00Z", "2026-08-27T00:00:00+08:00"),
            ("2026-08-26T15:59:59Z", "2026-08-20T00:00:00+08:00"),
        ] {
            assert_eq!(
                start(instant, Weekday::Thu, midnight, Kuala_Lumpur),
                expected,
                "{instant}"
            );
        }
    }

    #[test]
    fn reset_inside_a_dst_gap_keeps_its_wall_clock_across_the_chain() {
        // Sunday 02:30 does not exist in New York on 2026-03-08.
        let now = at("2026-03-08T12:00:00-04:00");
        let starts = materialised_week_starts(New_York, Weekday::Sun, hm(2, 30), &now).unwrap();
        let text: Vec<String> = starts.iter().map(ZonedDateTime::isoformat).collect();
        assert_eq!(
            text,
            [
                "2026-03-08T02:30:00-05:00",
                "2026-03-15T02:30:00-04:00",
                "2026-03-22T02:30:00-04:00"
            ]
        );
    }

    #[test]
    fn autumn_fold_week_end_keeps_the_wall_clock() {
        let ws = at("2026-10-29T01:30:00-04:00");
        assert_eq!(
            week_end(&ws, New_York).unwrap().isoformat(),
            "2026-11-05T01:30:00-05:00"
        );
        let fold = at("2026-11-01T01:30:00-05:00");
        assert_eq!(
            week_start(&fold, New_York, Weekday::Sun, hm(1, 30))
                .unwrap()
                .isoformat(),
            "2026-11-01T01:30:00-04:00"
        );
    }

    #[test]
    fn slot_on_the_reset_itself_stays_in_the_week() {
        let ws = at("2026-08-27T12:00:00+08:00");
        let slot = slot_in_week(&ws, Kuala_Lumpur, Weekday::Thu, hm(12, 0)).unwrap();
        assert_eq!(slot.isoformat(), "2026-08-27T12:00:00+08:00");
    }

    #[test]
    fn python_year_range_is_enforced() {
        let wall = NaiveDate::from_ymd_opt(9999, 12, 30)
            .unwrap()
            .and_time(NaiveTime::MIN);
        let ws = ZonedDateTime::new(wall, Kuala_Lumpur).unwrap();
        assert_eq!(week_end(&ws, Kuala_Lumpur), Err(DateOutOfRange));
    }
}
