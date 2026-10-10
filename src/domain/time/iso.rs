use std::fmt::{self, Write};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

use super::zoned::{AwareDateTime, DateOutOfRange};
use crate::domain::pytext::repr;

/// ISO-8601 conversion failures, with v4's messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoError {
    /// A naive datetime reached a serialiser that stores UTC instants.
    Naive,
    /// The text is not an ISO datetime accepted by `datetime.fromisoformat`.
    Invalid { value: String },
    /// A well-formed field is out of range, e.g. `hour must be in 0..23`.
    OutOfRange { message: String },
    /// The UTC instant leaves years 1..=9999 (Python `OverflowError`).
    Overflow(DateOutOfRange),
}

impl From<DateOutOfRange> for IsoError {
    fn from(error: DateOutOfRange) -> Self {
        Self::Overflow(error)
    }
}

impl fmt::Display for IsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Naive => f.write_str("refusing to serialise a naive datetime"),
            Self::Invalid { value } => write!(f, "Invalid isoformat string: {}", repr(value)),
            Self::OutOfRange { message } => f.write_str(message),
            Self::Overflow(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for IsoError {}

/// A datetime parsed from ISO text, which may lack an offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoDateTime {
    Naive(NaiveDateTime),
    Aware(DateTime<FixedOffset>),
}

impl IsoDateTime {
    /// Python `datetime.fromisoformat` for `YYYY-MM-DD`, optionally followed by
    /// any one separator, `HH[[:]MM[[:]SS]][.fraction]` and `Z`/`±HH[[:]MM[[:]SS]]`.
    /// Basic-format and week dates and fractional-second offsets are refused.
    ///
    /// # Errors
    /// [`IsoError::Invalid`] for malformed text, [`IsoError::OutOfRange`] with
    /// Python's message for an impossible field.
    pub fn parse(value: &str) -> Result<Self, IsoError> {
        super::iso_parse::parse(value)
    }

    /// v4 `to_iso` over an untyped value: aware values become UTC ISO text.
    ///
    /// # Errors
    /// [`IsoError::Naive`] for a naive value, [`IsoError::Overflow`] as [`to_iso`].
    pub fn to_iso(&self) -> Result<String, IsoError> {
        match self {
            Self::Naive(_) => Err(IsoError::Naive),
            Self::Aware(at) => Ok(to_iso(at)?),
        }
    }
}

/// Serialise an instant as ISO-8601 UTC, e.g. `2026-09-09T04:00:00+00:00`.
///
/// # Errors
/// [`DateOutOfRange`] when the UTC year leaves 1..=9999.
pub fn to_iso<T: TimeZone>(at: &DateTime<T>) -> Result<String, DateOutOfRange> {
    Ok(format_iso(&utc_in_range(at)?, Some(Utc.fix())))
}

/// Parse ISO text into a UTC instant; naive text is read as UTC.
///
/// # Errors
/// As [`IsoDateTime::parse`], or [`IsoError::Overflow`] when the UTC year
/// leaves 1..=9999.
pub fn from_iso(value: &str) -> Result<DateTime<Utc>, IsoError> {
    let at = match IsoDateTime::parse(value)? {
        IsoDateTime::Naive(wall) => wall.and_utc(),
        IsoDateTime::Aware(at) => at.with_timezone(&Utc),
    };
    utc_in_range(&at)?;
    Ok(at)
}

/// v4's `astimezone(UTC)` overflows outside Python's year range.
fn utc_in_range<T: TimeZone>(at: &DateTime<T>) -> Result<NaiveDateTime, DateOutOfRange> {
    let utc = at.naive_utc();
    if (1..=9999).contains(&utc.year()) {
        Ok(utc)
    } else {
        Err(DateOutOfRange)
    }
}

/// The wall clock of `at` in `zone`, without zone information.
///
/// # Errors
/// [`DateOutOfRange`] when that wall clock leaves years 1..=9999.
pub fn local_naive(at: &impl AwareDateTime, zone: Tz) -> Result<NaiveDateTime, DateOutOfRange> {
    Ok(at.astimezone(zone)?.wall())
}

/// Python `isoformat()` of a naive datetime, e.g. `2026-09-09T12:00:00`.
pub fn isoformat_naive(wall: &NaiveDateTime) -> String {
    format_iso(wall, None)
}

/// Python `isoformat()` of an aware datetime in its own offset.
pub fn isoformat(at: &DateTime<FixedOffset>) -> String {
    let offset = *at.offset();
    match at.naive_utc().checked_add_offset(offset) {
        Some(wall) => format_iso(&wall, Some(offset)),
        // Only reachable at chrono's own limits, far outside v4's years.
        None => format_iso(&at.naive_utc(), Some(Utc.fix())),
    }
}

pub(crate) fn format_iso(wall: &NaiveDateTime, offset: Option<FixedOffset>) -> String {
    let mut out = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        wall.year(),
        wall.month(),
        wall.day(),
        wall.hour(),
        wall.minute(),
        wall.second()
    );
    let micros = wall.nanosecond() / 1_000;
    // Writing to a String cannot fail.
    if micros != 0 {
        let _ = write!(out, ".{micros:06}");
    }
    if let Some(offset) = offset {
        let seconds = offset.local_minus_utc();
        let sign = if seconds < 0 { '-' } else { '+' };
        let seconds = seconds.unsigned_abs();
        let _ = write!(out, "{sign}{:02}:{:02}", seconds / 3600, seconds / 60 % 60);
        if seconds % 60 != 0 {
            let _ = write!(out, ":{:02}", seconds % 60);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveTime};

    use super::*;

    #[test]
    fn utc_conversion_outside_python_years_overflows() {
        for text in ["0001-01-01T00:00:00+01:00", "9999-12-31T23:00:00-01:00"] {
            let error = from_iso(text).unwrap_err();
            assert_eq!(error, IsoError::Overflow(DateOutOfRange));
            assert_eq!(error.to_string(), "date value out of range");
        }
        let IsoDateTime::Aware(late) = IsoDateTime::parse("9999-12-31T23:00:00-05:00").unwrap()
        else {
            panic!("offset is aware");
        };
        assert_eq!(to_iso(&late), Err(DateOutOfRange));
        let edge = from_iso("9999-12-31T23:59:59.999999+00:00").unwrap();
        assert_eq!(to_iso(&edge).unwrap(), "9999-12-31T23:59:59.999999+00:00");
        assert_eq!(from_iso("0001-01-01T00:00:00Z").unwrap().year(), 1);
    }

    #[test]
    fn zoned_values_serialise_to_their_exact_instant() {
        use chrono_tz::America::New_York;

        use crate::domain::time::ZonedDateTime;

        let wall = NaiveDate::from_ymd_opt(9999, 12, 31)
            .unwrap()
            .and_hms_opt(23, 0, 0)
            .unwrap();
        let late = ZonedDateTime::new(wall, New_York).unwrap();
        assert_eq!(to_iso(&late.to_fixed()), Err(DateOutOfRange));
        let second = from_iso("2026-11-01T06:30:00Z").unwrap();
        let zoned = ZonedDateTime::from_instant(&second, New_York).unwrap();
        assert_eq!(
            to_iso(&zoned.to_fixed()).unwrap(),
            "2026-11-01T06:30:00+00:00"
        );
    }

    #[test]
    fn offsets_with_seconds_keep_them() {
        let offset = FixedOffset::east_opt(6 * 3600 + 55 * 60 + 25).unwrap();
        let wall = NaiveDate::from_ymd_opt(1900, 1, 1)
            .unwrap()
            .and_time(NaiveTime::MIN);
        assert_eq!(
            format_iso(&wall, Some(offset)),
            "1900-01-01T00:00:00+06:55:25"
        );
    }
}
