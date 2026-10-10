//! Python 3.12 `datetime.fromisoformat` for the forms v4 stores and reads.

use chrono::{FixedOffset, NaiveDate, NaiveTime, TimeZone};

use super::iso::{IsoDateTime, IsoError};

/// Parse `YYYY-MM-DD`, optionally followed by any one separator character,
/// `HH[[:]MM[[:]SS]][.fraction]` and `Z` or `±HH[[:]MM[[:]SS]]`.
///
/// Basic-format and week dates and fractional-second offsets, which Python also
/// accepts, are reported as invalid.
pub(super) fn parse(value: &str) -> Result<IsoDateTime, IsoError> {
    let invalid = || IsoError::Invalid {
        value: value.to_owned(),
    };
    let [year, month, day] = value.get(..10).and_then(date_fields).ok_or_else(invalid)?;
    let mut rest = value.get(10..).ok_or_else(invalid)?.chars();
    let (clock, offset) = if rest.next().is_none() {
        ([0; 4], None)
    } else {
        let rest = rest.as_str();
        match rest.find(['+', '-', 'Z']) {
            Some(at) => (
                clock_fields(&rest[..at]).ok_or_else(invalid)?,
                Some(offset_seconds(&rest[at..]).ok_or_else(invalid)?),
            ),
            None => (clock_fields(rest).ok_or_else(invalid)?, None),
        }
    };
    // Python builds the offset before the datetime, so its range error wins.
    let offset = offset
        .map(|seconds| {
            FixedOffset::east_opt(seconds).ok_or_else(|| {
                out_of_range(format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and \
                     timedelta(hours=24), not {}.",
                    timedelta_repr(seconds)
                ))
            })
        })
        .transpose()?;
    let wall = date(year, month, day)?.and_time(time(clock)?);
    Ok(match offset {
        None => IsoDateTime::Naive(wall),
        Some(offset) => IsoDateTime::Aware(
            offset
                .from_local_datetime(&wall)
                .single()
                .ok_or_else(invalid)?,
        ),
    })
}

fn out_of_range(message: impl Into<String>) -> IsoError {
    IsoError::OutOfRange {
        message: message.into(),
    }
}

fn date(year: u32, month: u32, day: u32) -> Result<NaiveDate, IsoError> {
    if year == 0 {
        return Err(out_of_range(format!("year {year} is out of range")));
    }
    if !(1..=12).contains(&month) {
        return Err(out_of_range("month must be in 1..12"));
    }
    i32::try_from(year)
        .ok()
        .and_then(|year| NaiveDate::from_ymd_opt(year, month, day))
        .ok_or_else(|| out_of_range("day is out of range for month"))
}

fn time([hour, minute, second, micros]: [u32; 4]) -> Result<NaiveTime, IsoError> {
    for (value, limit, name) in [
        (hour, 23, "hour"),
        (minute, 59, "minute"),
        (second, 59, "second"),
    ] {
        if value > limit {
            return Err(out_of_range(format!("{name} must be in 0..{limit}")));
        }
    }
    NaiveTime::from_hms_micro_opt(hour, minute, second, micros)
        .ok_or_else(|| out_of_range("microsecond must be in 0..999999"))
}

/// `repr(timedelta(seconds=...))`.
fn timedelta_repr(seconds: i32) -> String {
    let (days, seconds) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let mut fields = Vec::new();
    if days != 0 {
        fields.push(format!("days={days}"));
    }
    if seconds != 0 {
        fields.push(format!("seconds={seconds}"));
    }
    if fields.is_empty() {
        fields.push("0".to_owned());
    }
    format!("datetime.timedelta({})", fields.join(", "))
}

fn date_fields(text: &str) -> Option<[u32; 3]> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    Some([
        digits(&text[..4])?,
        digits(&text[5..7])?,
        digits(&text[8..])?,
    ])
}

/// Hour, minute, second and microsecond, not yet range-checked.
fn clock_fields(text: &str) -> Option<[u32; 4]> {
    let (hms, fraction) = match text.find(['.', ',']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let [hour, minute, second] = hms_fields(hms)?;
    let micros = match fraction {
        None => 0,
        Some(fraction) => {
            digits(fraction)?;
            // Digits past microseconds are dropped, as Python does.
            let kept = &fraction[..fraction.len().min(6)];
            digits(kept)? * 10u32.pow(6 - u32::try_from(kept.len()).ok()?)
        }
    };
    Some([hour, minute, second, micros])
}

fn offset_seconds(text: &str) -> Option<i32> {
    if text == "Z" {
        return Some(0);
    }
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let [hours, minutes, seconds] = hms_fields(&text[1..])?;
    Some(sign * i32::try_from(hours * 3600 + minutes * 60 + seconds).ok()?)
}

/// `HH`, `HHMM`, `HH:MM`, `HHMMSS` or `HH:MM:SS`, not yet range-checked.
fn hms_fields(text: &str) -> Option<[u32; 3]> {
    let parts: Vec<&str> = if text.contains(':') {
        text.split(':').collect()
    } else if text.len().is_multiple_of(2) && text.is_ascii() {
        (0..text.len())
            .step_by(2)
            .map(|at| &text[at..at + 2])
            .collect()
    } else {
        return None;
    };
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|part| part.len() != 2) {
        return None;
    }
    let mut out = [0; 3];
    for (slot, part) in out.iter_mut().zip(&parts) {
        *slot = digits(part)?;
    }
    Some(out)
}

/// A run of ASCII digits; Python's ISO parser rejects other decimal digits.
fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pytext::repr;
    use crate::domain::time::{isoformat, isoformat_naive};

    fn render(text: &str) -> String {
        match parse(text) {
            Ok(IsoDateTime::Naive(wall)) => isoformat_naive(&wall),
            Ok(IsoDateTime::Aware(at)) => isoformat(&at),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn accepted_forms_match_python() {
        for (text, expected) in [
            ("2026-09-09", "2026-09-09T00:00:00"),
            ("2026-09-09 12:30", "2026-09-09T12:30:00"),
            ("2026-09-09x1230", "2026-09-09T12:30:00"),
            ("2026-09-09T12", "2026-09-09T12:00:00"),
            ("2026-09-09T12.5", "2026-09-09T12:00:00.500000"),
            ("2026-09-09T12:00:00,1234567", "2026-09-09T12:00:00.123456"),
            ("2026-09-09T12Z", "2026-09-09T12:00:00+00:00"),
            ("2026-09-09T12:00-0530", "2026-09-09T12:00:00-05:30"),
            ("2026-09-09T12:00+013000", "2026-09-09T12:00:00+01:30"),
            ("2026-09-09T12:00+01:30:30", "2026-09-09T12:00:00+01:30:30"),
        ] {
            assert_eq!(render(text), expected, "{text}");
        }
    }

    #[test]
    fn malformed_text_is_invalid() {
        for text in [
            "",
            "2026-9-09",
            "2026-09-09T",
            "2026-09-09 ",
            "2026-09-09T1:00",
            "2026-09-09T12:0000",
            "2026-09-09T12:00:00.",
            "2026-09-09T12:00+",
            "2026-09-09T12:00Z05",
            "2026-09-09T١٢:00",
        ] {
            assert_eq!(
                render(text),
                format!("Invalid isoformat string: {}", repr(text)),
                "{text}"
            );
        }
    }

    #[test]
    fn out_of_range_fields_use_python_messages() {
        for (text, expected) in [
            ("0000-01-01", "year 0 is out of range"),
            ("2026-13-01T25:00", "month must be in 1..12"),
            ("2026-02-30", "day is out of range for month"),
            ("2026-09-09T24:00", "hour must be in 0..23"),
            ("2026-09-09T23:60", "minute must be in 0..59"),
            ("2026-09-09T23:59:60", "second must be in 0..59"),
            (
                "2026-13-01T12:00+24:00",
                "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not datetime.timedelta(days=1).",
            ),
            (
                "2026-09-09T12:00-25:00",
                "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not datetime.timedelta(days=-2, seconds=82800).",
            ),
        ] {
            assert_eq!(render(text), expected, "{text}");
        }
    }
}
