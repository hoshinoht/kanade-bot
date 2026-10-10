use std::fmt;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Datelike, FixedOffset, MappedLocalTime, NaiveDateTime, Offset, TimeZone};
use chrono_tz::Tz;

use super::iso::isoformat;

/// A result outside v4's representable years 1..=9999 (Python `OverflowError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateOutOfRange;

impl fmt::Display for DateOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("date value out of range")
    }
}

impl std::error::Error for DateOutOfRange {}

/// A wall-clock datetime attached to a named zone, as v4's `ZoneInfo` datetimes.
///
/// The wall clock is kept verbatim, even inside a DST gap. Like Python's `fold`,
/// a flag picks the offset of an ambiguous or skipped wall clock: `false` is the
/// offset before the transition, `true` the one after. Only
/// [`ZonedDateTime::from_instant`] sets it, for the second pass through a
/// repeated hour, so instants round-trip exactly.
///
/// Equality and hashing use wall clock and zone and ignore the fold, as Python
/// compares and hashes datetimes sharing a `tzinfo`.
#[derive(Clone, Copy, Debug)]
pub struct ZonedDateTime {
    wall: NaiveDateTime,
    zone: Tz,
    fold: bool,
}

impl PartialEq for ZonedDateTime {
    fn eq(&self, other: &Self) -> bool {
        self.wall == other.wall && self.zone == other.zone
    }
}

impl Eq for ZonedDateTime {}

impl Hash for ZonedDateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.wall.hash(state);
        self.zone.hash(state);
    }
}

impl ZonedDateTime {
    /// Attach `zone` to a wall clock with `fold=0` (Python `replace(tzinfo=zone)`).
    ///
    /// # Errors
    /// [`DateOutOfRange`] when the year is outside 1..=9999.
    pub fn new(wall: NaiveDateTime, zone: Tz) -> Result<Self, DateOutOfRange> {
        if (1..=9999).contains(&wall.year()) {
            Ok(Self {
                wall,
                zone,
                fold: false,
            })
        } else {
            Err(DateOutOfRange)
        }
    }

    /// The wall clock of `at` in `zone`; the second pass through a repeated
    /// hour gets `fold=1`, as Python's `astimezone` sets it.
    ///
    /// # Errors
    /// [`DateOutOfRange`] when the UTC instant or that wall clock is outside
    /// years 1..=9999 (Python computes the UTC value first and overflows there).
    pub fn from_instant<T: TimeZone>(at: &DateTime<T>, zone: Tz) -> Result<Self, DateOutOfRange> {
        let utc = at.naive_utc();
        if !(1..=9999).contains(&utc.year()) {
            return Err(DateOutOfRange);
        }
        let offset = zone.offset_from_utc_datetime(&utc).fix();
        let wall = utc.checked_add_offset(offset).ok_or(DateOutOfRange)?;
        let mut zoned = Self::new(wall, zone)?;
        let (before, after) = local_offsets(zone, wall);
        zoned.fold = before != after && offset == after;
        Ok(zoned)
    }

    pub fn wall(&self) -> NaiveDateTime {
        self.wall
    }

    pub fn zone(&self) -> Tz {
        self.zone
    }

    /// Python's `fold`: whether an ambiguous wall clock is its second occurrence.
    pub fn fold(&self) -> bool {
        self.fold
    }

    /// The UTC offset of this wall clock, honouring the fold.
    pub fn offset(&self) -> FixedOffset {
        let (before, after) = local_offsets(self.zone, self.wall);
        if self.fold { after } else { before }
    }

    /// The instant with its offset; a gap wall clock keeps its digits.
    pub fn to_fixed(&self) -> DateTime<FixedOffset> {
        let offset = self.offset();
        let utc = self
            .wall
            .checked_sub_offset(offset)
            .expect("a year 1..=9999 wall clock minus a sub-day offset is representable");
        DateTime::from_naive_utc_and_offset(utc, offset)
    }

    /// Python `datetime.isoformat()`, e.g. `2026-03-15T00:00:00-04:00`.
    pub fn isoformat(&self) -> String {
        isoformat(&self.to_fixed())
    }
}

/// An aware datetime that can be viewed in a guild zone.
pub trait AwareDateTime {
    /// Python `astimezone(zone)`: a value already in `zone` keeps its wall clock
    /// unchanged; anything else converts by instant.
    ///
    /// # Errors
    /// [`DateOutOfRange`] when the converted wall clock leaves years 1..=9999.
    fn astimezone(&self, zone: Tz) -> Result<ZonedDateTime, DateOutOfRange>;
}

impl<T: TimeZone> AwareDateTime for DateTime<T> {
    fn astimezone(&self, zone: Tz) -> Result<ZonedDateTime, DateOutOfRange> {
        ZonedDateTime::from_instant(self, zone)
    }
}

impl AwareDateTime for ZonedDateTime {
    fn astimezone(&self, zone: Tz) -> Result<ZonedDateTime, DateOutOfRange> {
        if self.zone == zone {
            Ok(*self)
        } else {
            ZonedDateTime::from_instant(&self.to_fixed(), zone)
        }
    }
}

/// Offsets before and after the transition that makes `wall` ambiguous or
/// skipped; both are the same when `wall` occurs exactly once.
fn local_offsets(zone: Tz, wall: NaiveDateTime) -> (FixedOffset, FixedOffset) {
    match zone.offset_from_local_datetime(&wall) {
        MappedLocalTime::Single(offset) => (offset.fix(), offset.fix()),
        // Clocks went back: the pre-transition offset is the larger one.
        MappedLocalTime::Ambiguous(a, b) => {
            let (a, b) = (a.fix(), b.fix());
            if a.local_minus_utc() >= b.local_minus_utc() {
                (a, b)
            } else {
                (b, a)
            }
        }
        // Clocks jumped forward: probing either side yields both offsets; the
        // pre-transition one is the smaller.
        MappedLocalTime::None => {
            let probe = |offset: FixedOffset| {
                wall.checked_sub_offset(offset)
                    .map(|utc| zone.offset_from_utc_datetime(&utc).fix())
                    .unwrap_or(offset)
            };
            let first = probe(zone.offset_from_utc_datetime(&wall).fix());
            let second = probe(first);
            if first.local_minus_utc() <= second.local_minus_utc() {
                (first, second)
            } else {
                (second, first)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use chrono_tz::America::New_York;

    use super::*;

    fn wall(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn gap_wall_clock_keeps_its_digits_with_the_pre_transition_offset() {
        let gap = ZonedDateTime::new(wall(2026, 3, 8, 2, 30), New_York).unwrap();
        assert_eq!(gap.isoformat(), "2026-03-08T02:30:00-05:00");
        assert_eq!(gap.astimezone(New_York).unwrap(), gap);
    }

    #[test]
    fn from_instant_rejects_a_utc_value_before_year_one() {
        // Python: fromisoformat('0001-01-01T00:30:00+01:00').astimezone(KL) -> OverflowError.
        let at = utc("0001-01-01T00:30:00+01:00");
        assert_eq!(
            ZonedDateTime::from_instant(&at, chrono_tz::Asia::Kuala_Lumpur),
            Err(DateOutOfRange)
        );
    }

    #[test]
    fn ambiguous_wall_clock_uses_the_first_occurrence() {
        let fold = ZonedDateTime::new(wall(2026, 11, 1, 1, 30), New_York).unwrap();
        assert_eq!(fold.isoformat(), "2026-11-01T01:30:00-04:00");
    }

    fn utc(text: &str) -> DateTime<FixedOffset> {
        match crate::domain::time::IsoDateTime::parse(text).unwrap() {
            crate::domain::time::IsoDateTime::Aware(at) => at,
            crate::domain::time::IsoDateTime::Naive(_) => panic!("{text} must be aware"),
        }
    }

    #[test]
    fn both_passes_through_a_repeated_hour_round_trip() {
        for (instant, local, fold) in [
            (
                "2026-11-01T05:30:00+00:00",
                "2026-11-01T01:30:00-04:00",
                false,
            ),
            (
                "2026-11-01T06:30:00+00:00",
                "2026-11-01T01:30:00-05:00",
                true,
            ),
        ] {
            let at = utc(instant);
            let zoned = ZonedDateTime::from_instant(&at, New_York).unwrap();
            assert_eq!(zoned.fold(), fold, "{instant}");
            assert_eq!(zoned.isoformat(), local);
            assert_eq!(zoned.to_fixed(), at);
            let back = zoned.astimezone(chrono_tz::UTC).unwrap();
            assert_eq!(back.isoformat(), instant);
        }
    }

    #[test]
    fn instants_round_trip_across_a_whole_fold_day() {
        let start = utc("2026-10-31T00:00:00+00:00");
        for minutes in (0..48 * 60).step_by(15) {
            let at = start + chrono::TimeDelta::minutes(minutes);
            let zoned = ZonedDateTime::from_instant(&at, New_York).unwrap();
            assert_eq!(zoned.to_fixed(), at, "{at}");
        }
    }

    #[test]
    fn same_zone_equality_ignores_the_fold_like_python() {
        let first = ZonedDateTime::from_instant(&utc("2026-11-01T05:30:00Z"), New_York).unwrap();
        let second = ZonedDateTime::from_instant(&utc("2026-11-01T06:30:00Z"), New_York).unwrap();
        assert_eq!(first, second);
        assert_ne!(first.to_fixed(), second.to_fixed());
        let hash = |value: &ZonedDateTime| {
            let mut state = std::hash::DefaultHasher::new();
            value.hash(&mut state);
            state.finish()
        };
        assert_eq!(hash(&first), hash(&second));
    }

    #[test]
    fn years_outside_python_range_are_refused() {
        assert_eq!(
            ZonedDateTime::new(wall(10000, 1, 1, 0, 0), New_York),
            Err(DateOutOfRange)
        );
    }
}
