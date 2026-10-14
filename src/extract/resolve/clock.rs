use std::sync::LazyLock;

use chrono::NaiveTime;
use regex::{Captures, Regex};

use crate::domain::pytext::strip;
use crate::extract::text::{SPACE, int, pattern, split_tail};

/// Bare hours at or below this are read as pm: this guild's runs are evening
/// runs, so `930`, `10` and `at 11` mean 21:30, 22:00 and 23:00.
pub const PM_CUTOFF: u32 = 11;

/// v4's `\b(a\.?m\.?|p\.?m\.?)\b|(?<=\d)\s*(am|pm)\b`. `regex` has no
/// lookbehind, so the digit is captured and put back on substitution; no
/// other match can start at that digit, so the matches are v4's.
static MERIDIEM: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"(?i)\b(a\.?m\.?|p\.?m\.?)\b|(\d){SPACE}*(am|pm)\b"
    ))
});
static TIME_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\b(?:at|around|about|ard|by|from|onwards?|ish|latest|plus|sharp|night|nite|evening|morning|afternoon|pm ish)\b",
    )
});
static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        r"^{SPACE}*(\d{{1,2}}(?:[:.]?\d{{2}})?){SPACE}*\+?{SPACE}*[~\-]{SPACE}*\d{{1,4}}{SPACE}*\+?{SPACE}*(.*)$"
    ))
});
static RANGE_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)[~\-\x{2013}\x{2014}]|\bto\b|\btill\b|\buntil\b"));
static NOT_CLOCK: LazyLock<Regex> = LazyLock::new(|| pattern(r"[^\d:.]"));
static HHMM: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{1,2})[:.](\d{2})$"));
static COMPACT: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{3,4})$"));
static HOUR: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{1,2})$"));

/// `(hour24, assumed)`; `assumed` marks the bare-hour pm default.
fn apply_meridiem(hour: u32, meridiem: Option<&str>) -> (u32, bool) {
    if let Some(meridiem) = meridiem {
        let letter = meridiem.to_lowercase().replace('.', "");
        return match (letter.chars().next(), hour) {
            (Some('p'), hour) if hour != 12 => (hour + 12, false),
            (Some('a'), 12) => (0, false),
            _ => (hour, false),
        };
    }
    match hour {
        1..=PM_CUTOFF => (hour + 12, true),
        // "12" at the end of a boss night is midnight, not lunchtime.
        12 => (0, true),
        _ => (hour, false),
    }
}

fn meridiem_of<'t>(found: &Captures<'t>) -> Option<&'t str> {
    found.get(1).or_else(|| found.get(3)).map(|m| m.as_str())
}

/// `9:30pm` -> `(21:30, false)`, `930` -> `(21:30, true)`; `None` for anything
/// that is not a clock time (`night`, `later`, `after boss`).
pub fn parse_clock(time_ref: Option<&str>) -> Option<(NaiveTime, bool)> {
    let text = strip(time_ref?).to_lowercase();
    if text.is_empty() {
        return None;
    }
    let meridiem = MERIDIEM
        .captures(&text)
        .and_then(|found| meridiem_of(&found));

    // A range is its start time; a meridiem at the end of it applies to it.
    let core = match RANGE.captures(&text) {
        Some(range) => range[1].to_owned(),
        None => MERIDIEM
            .replace_all(&text, |found: &Captures<'_>| match found.get(2) {
                Some(digit) => format!("{} ", digit.as_str()),
                None => " ".to_owned(),
            })
            .into_owned(),
    };
    let core = TIME_NOISE.replace_all(&core, " ");
    let core = match RANGE_SPLIT.find(&core) {
        Some(split) => &core[..split.start()],
        None => &core,
    };
    let digits = NOT_CLOCK.replace_all(core, "");
    let core = digits.trim_matches(['.', ':']);
    if core.is_empty() {
        return None;
    }

    let (hour, minute) = if let Some(found) = HHMM.captures(core) {
        (int(&found[1])?, int(&found[2])?)
    } else if let Some(found) = COMPACT.captures(core) {
        let (hour, minute) = split_tail(&found[1], 2);
        (int(hour)?, int(minute)?)
    } else {
        let found = HOUR.captures(core)?;
        (int(&found[1])?, 0)
    };
    if minute > 59 || hour > 23 {
        return None;
    }
    let (hour, assumed) = apply_meridiem(hour, meridiem);
    if hour > 23 {
        return None;
    }
    Some((NaiveTime::from_hms_opt(hour, minute, 0)?, assumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(text: &str) -> Option<(String, bool)> {
        parse_clock(Some(text)).map(|(at, assumed)| (at.to_string(), assumed))
    }

    #[test]
    fn spaced_meridiem_keeps_the_digit_it_follows() {
        // v4's lookbehind leaves the digit in place when the meridiem is removed.
        assert_eq!(clock("9 pm"), Some(("21:00:00".into(), false)));
        assert_eq!(clock("11\u{1f}am"), Some(("11:00:00".into(), false)));
        assert_eq!(clock("７pm"), Some(("19:00:00".into(), false)));
    }
}
