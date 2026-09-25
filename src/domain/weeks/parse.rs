use std::fmt;

use chrono::{NaiveTime, Weekday};

use crate::domain::pytext::{decimal, is_digit, is_space, repr, strip};

/// CPython's default `int()` limit on decimal string length.
const INT_MAX_STR_DIGITS: usize = 4300;

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

/// Weekday and clock-time input errors, with v4's `ValueError` messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeekParseError {
    /// The text names no weekday; `value` is the input as given.
    UnknownWeekday { value: String },
    /// A numeric weekday outside 0..=6, in decimal without leading zeros.
    WeekdayOutOfRange { value: String },
    /// The text is not a clock time; `value` is the input as given.
    InvalidTime { value: String },
    /// All digits but not all decimal (e.g. `²`); `literal` is the stripped,
    /// lowercased text v4 handed to `int()`.
    InvalidIntLiteral { literal: String },
    /// A decimal index longer than CPython's `int()` digit limit.
    IntTooLong { digits: usize },
}

impl fmt::Display for WeekParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownWeekday { value } => write!(
                f,
                "unknown weekday {}; expected one of mon, tue, wed, thu, fri, sat, sun",
                repr(value)
            ),
            Self::WeekdayOutOfRange { value } => write!(f, "weekday out of range: {value}"),
            Self::InvalidTime { value } => write!(
                f,
                "expected a time like 21:30, 2130 or 9:30pm, got {}",
                repr(value)
            ),
            // CPython formats the literal with `%.200R`: its repr cut to 200 characters.
            Self::InvalidIntLiteral { literal } => {
                let shown: String = repr(literal).chars().take(200).collect();
                write!(f, "invalid literal for int() with base 10: {shown}")
            }
            Self::IntTooLong { digits } => write!(
                f,
                "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: \
                 value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
            ),
        }
    }
}

impl std::error::Error for WeekParseError {}

/// A Monday-zero weekday index, as v4 stores it.
///
/// # Errors
/// [`WeekParseError::WeekdayOutOfRange`] outside 0..=6.
pub fn weekday_from_index(index: i64) -> Result<Weekday, WeekParseError> {
    usize::try_from(index)
        .ok()
        .and_then(|at| WEEKDAYS.get(at).copied())
        .ok_or_else(|| WeekParseError::WeekdayOutOfRange {
            value: index.to_string(),
        })
}

/// Turn `thu`, `Thursday`, `thurs` or `3` into a weekday.
///
/// Any Unicode decimal digits are read as an index, as v4 does; other digit
/// characters fail as v4's `int()` does.
///
/// # Errors
/// [`WeekParseError::UnknownWeekday`], [`WeekParseError::WeekdayOutOfRange`],
/// [`WeekParseError::InvalidIntLiteral`] or [`WeekParseError::IntTooLong`].
pub fn parse_weekday(value: &str) -> Result<Weekday, WeekParseError> {
    let key = strip(value).to_lowercase();
    if !key.is_empty() && key.chars().all(is_digit) {
        let Some(digits) = decimal_digits(&key) else {
            return Err(WeekParseError::InvalidIntLiteral { literal: key });
        };
        if digits.len() > INT_MAX_STR_DIGITS {
            return Err(WeekParseError::IntTooLong {
                digits: digits.len(),
            });
        }
        let significant: Vec<u8> = digits.into_iter().skip_while(|&digit| digit == 0).collect();
        return match significant.as_slice() {
            [] => Ok(WEEKDAYS[0]),
            [digit] if usize::from(*digit) < WEEKDAYS.len() => Ok(WEEKDAYS[usize::from(*digit)]),
            _ => Err(WeekParseError::WeekdayOutOfRange {
                value: significant
                    .iter()
                    .map(|&digit| char::from(b'0' + digit))
                    .collect(),
            }),
        };
    }
    let weekday = match key.as_str() {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tuesday" => Weekday::Tue,
        "wed" | "wednesday" | "weds" => Weekday::Wed,
        "thu" | "thursday" | "thurs" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => {
            return Err(WeekParseError::UnknownWeekday {
                value: value.to_owned(),
            });
        }
    };
    Ok(weekday)
}

/// Parse a clock time the way people type it.
///
/// Accepts `09:00`, `9:05`, `9.30`, 24-hour compact `2130` / `930`, and 12-hour
/// `9pm`, `9:30pm`, `930pm`, `12am`. Compact digits are 24-hour (`930` is 09:30).
/// Any Unicode decimal digits count, as in v4's regex.
///
/// # Errors
/// [`WeekParseError::InvalidTime`] for anything else or an impossible time.
pub fn parse_hhmm(value: &str) -> Result<NaiveTime, WeekParseError> {
    let invalid = || WeekParseError::InvalidTime {
        value: value.to_owned(),
    };
    let text = strip(value);
    let split = text
        .find(|c: char| decimal(c).is_none())
        .unwrap_or(text.len());
    let (digits, rest) = text.split_at(split);
    let digits: Vec<u8> = decimal_digits(digits).ok_or_else(invalid)?;
    let (mut hour, minute, suffix) = match digits.len() {
        3 | 4 => {
            let (hour, minute) = digits.split_at(digits.len() - 2);
            (
                number(hour),
                number(minute),
                meridiem(rest).ok_or_else(invalid)?,
            )
        }
        1 | 2 => {
            let (minute, rest) = match rest.strip_prefix([':', '.']) {
                Some(tail) => {
                    let mut chars = tail.chars();
                    let pair = [chars.next(), chars.next()].map(|c| c.and_then(decimal_digit));
                    match pair {
                        [Some(tens), Some(ones)] => (number(&[tens, ones]), chars.as_str()),
                        _ => return Err(invalid()),
                    }
                }
                None => (0, rest),
            };
            (number(&digits), minute, meridiem(rest).ok_or_else(invalid)?)
        }
        _ => return Err(invalid()),
    };
    if let Some(suffix) = suffix {
        if !(1..=12).contains(&hour) {
            return Err(invalid());
        }
        match (suffix, hour) {
            (Meridiem::Pm, 1..=11) => hour += 12,
            (Meridiem::Am, 12) => hour = 0,
            _ => {}
        }
    }
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(invalid)
}

#[derive(Clone, Copy)]
enum Meridiem {
    Am,
    Pm,
}

/// The optional `\s*(am|pm|a|p)` tail; `None` when the tail is anything else.
fn meridiem(rest: &str) -> Option<Option<Meridiem>> {
    let suffix = rest.trim_start_matches(is_space);
    if suffix.is_empty() {
        return Some(None);
    }
    let lowered = suffix.to_ascii_lowercase();
    match lowered.as_str() {
        "am" | "a" => Some(Some(Meridiem::Am)),
        "pm" | "p" => Some(Some(Meridiem::Pm)),
        _ => None,
    }
}

fn decimal_digit(c: char) -> Option<u8> {
    decimal(c).and_then(|digit| u8::try_from(digit).ok())
}

/// Digit values when every character is a decimal digit.
fn decimal_digits(text: &str) -> Option<Vec<u8>> {
    text.chars().map(decimal_digit).collect()
}

/// At most four digits, so the total cannot overflow.
fn number(digits: &[u8]) -> u32 {
    digits
        .iter()
        .fold(0, |total, &digit| total * 10 + u32::from(digit))
}

#[cfg(test)]
mod tests {
    use chrono::Timelike;

    use super::*;

    fn hhmm(value: &str) -> String {
        let time = parse_hhmm(value).unwrap();
        format!("{:02}:{:02}", time.hour(), time.minute())
    }

    #[test]
    fn clock_forms_match_v4() {
        for (text, expected) in [
            ("09:00", "09:00"),
            ("9:05", "09:05"),
            ("9.30", "09:30"),
            ("2130", "21:30"),
            ("930", "09:30"),
            ("9pm", "21:00"),
            ("9:30 PM", "21:30"),
            ("12am", "00:00"),
            ("12pm", "12:00"),
            ("12:00a", "00:00"),
            (" 0930 ", "09:30"),
            ("９:30", "09:30"),
            ("٩:٣٠p", "21:30"),
        ] {
            assert_eq!(hhmm(text), expected, "{text}");
        }
        for text in [
            "", "13pm", "0am", "2360", "24:00", "12345", "9:3", "9:300", "9x", "9 : 30", "9:٣",
            "²", "²:30", "①", "9:²0", "፩:30",
        ] {
            assert!(parse_hhmm(text).is_err(), "{text}");
        }
    }

    #[test]
    fn weekday_indices_and_aliases() {
        assert_eq!(parse_weekday(" Thursday "), Ok(Weekday::Thu));
        assert_eq!(parse_weekday("006"), Ok(Weekday::Sun));
        assert_eq!(parse_weekday("٣"), Ok(Weekday::Thu));
        for (text, literal) in [
            ("²", "'²'"),
            (" 1² ", "'1²'"),
            ("①", "'①'"),
            ("፩", "'፩'"),
            ("٣²", "'٣²'"),
        ] {
            assert_eq!(
                parse_weekday(text).unwrap_err().to_string(),
                format!("invalid literal for int() with base 10: {literal}")
            );
        }
        let long = parse_weekday(&"¹".repeat(300)).unwrap_err().to_string();
        assert_eq!(long.chars().count(), 240);
        assert!(long.ends_with("¹¹¹"));
        assert!(
            parse_weekday("²x")
                .unwrap_err()
                .to_string()
                .starts_with("unknown weekday '²x'")
        );
        assert_eq!(
            parse_weekday(&"0".repeat(4301)).unwrap_err().to_string(),
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 \
             digits; use sys.set_int_max_str_digits() to increase the limit"
        );
        let mixed = format!("{}²", "1".repeat(4301));
        assert!(
            parse_weekday(&mixed)
                .unwrap_err()
                .to_string()
                .starts_with("invalid literal")
        );
        assert_eq!(
            parse_weekday(&"1".repeat(4300)).unwrap_err().to_string(),
            format!("weekday out of range: {}", "1".repeat(4300))
        );
        assert_eq!(
            parse_weekday("0010").unwrap_err().to_string(),
            "weekday out of range: 10"
        );
        assert_eq!(
            weekday_from_index(-1).unwrap_err().to_string(),
            "weekday out of range: -1"
        );
        assert_eq!(weekday_from_index(3), Ok(Weekday::Thu));
    }
}
