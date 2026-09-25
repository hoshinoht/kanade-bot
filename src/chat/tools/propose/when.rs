//! `wed 21:30` / `tomorrow 9:45pm` → an instant (v4 `service.parse_when`).
//!
//! v4 asked `dateparser` first and fell back to the extractor's resolver;
//! v5 has no general date parser, so the guild's own forms go through the
//! extractor's resolver directly: a bare clock time (today, or tomorrow once
//! past, pm assumed as v4), or `<day words> <clock>` including ISO dates,
//! with the typed hour read literally as `dateparser` did. Free-text dates
//! only `dateparser` read (`12 september 9pm`, `in 2 hours`) and
//! unrecognised day words are refused with v4's own message (known
//! difference K-WHEN-SUBSET).

use std::sync::LazyLock;

use chrono::{DateTime, Datelike, TimeDelta, Timelike, Utc};
use chrono_tz::Tz;
use regex::{Regex, RegexBuilder};

use crate::chat::tools::read::format::local;
use crate::domain::pytext::{split_whitespace, strip};
use crate::domain::schedule::utc_instant;
use crate::domain::time::ZonedDateTime;
use crate::domain::weeks::parse_hhmm;
use crate::extract::resolve::{parse_clock, resolve};

/// A misreading, not a plan (how a bare `2300` once became the year 2300).
const MAX_HORIZON_DAYS: i64 = 400;

static CLOCK_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(
        r"^\s*\d{1,2}[:.]?(?:\d{2})?\s*\+?\s*(?:[~\-]|to|till|until)?\s*(?:\d{1,2}[:.]?(?:\d{2})?\s*\+?\s*)?(?:a\.?m\.?|p\.?m\.?)?\s*$",
    )
    .case_insensitive(true)
    .build()
    .expect("pattern")
});

fn unreadable(text: &str) -> String {
    format!("couldn't read `{text}` as a date - try `wed 21:30` or `2026-09-02 21:30`")
}

/// A bare clock time said now: today, or tomorrow once it has gone.
fn bare_clock(text: &str, zone: Tz, now: DateTime<Utc>) -> Option<ZonedDateTime> {
    if !CLOCK_ONLY.is_match(text) {
        return None;
    }
    let (clock, _) = parse_clock(Some(text))?;
    let today = local(&now, zone);
    let mut wall = today.date().and_time(clock);
    if wall <= today {
        wall += TimeDelta::days(1);
    }
    ZonedDateTime::new(wall, zone).ok()
}

/// The clock is the last word; everything before it is the day.
fn day_and_clock(text: &str, zone: Tz, now: DateTime<Utc>) -> Option<ZonedDateTime> {
    let words: Vec<&str> = split_whitespace(text).collect();
    let (clock, day) = words.split_last()?;
    if day.is_empty() || parse_clock(Some(clock)).is_none() {
        return None;
    }
    let day = day.join(" ");
    // An unrecognised day would silently read as today; refuse it instead.
    resolve(Some(&day), None, &now, zone).ok()?.day?;
    let clock = literal_clock(clock);
    resolve(Some(&day), Some(&clock), &now, zone).ok()?.at
}

/// A typed day-and-time reads its hour literally (`wed 09:00` is 09:00, as
/// v4's `dateparser` read it); only the extractor's chat reading assumes pm.
fn literal_clock(clock: &str) -> String {
    match (parse_clock(Some(clock)), parse_hhmm(clock)) {
        (Some((_, true)), Ok(literal)) => {
            let (hour, minute) = (literal.hour(), literal.minute());
            match hour {
                0 => format!("12:{minute:02}am"),
                1..=11 => format!("{hour}:{minute:02}am"),
                12 => format!("12:{minute:02}pm"),
                _ => format!("{hour:02}:{minute:02}"),
            }
        }
        _ => clock.to_owned(),
    }
}

/// Parse a model-supplied day and time.
///
/// # Errors
/// v4's `couldn't read ... as a date` message (the caller appends advice).
pub fn parse_when(text: &str, zone: Tz, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let cleaned = strip(text);
    let at = bare_clock(cleaned, zone, now)
        .or_else(|| day_and_clock(cleaned, zone, now))
        .ok_or_else(|| unreadable(text))?;
    let at = utc_instant(&at).map_err(|_| unreadable(text))?;
    if at > now + TimeDelta::days(MAX_HORIZON_DAYS) {
        return Err(format!(
            "couldn't read `{text}` as a date - that lands in {}. Try `wed 21:30` or `2026-09-02 21:30`",
            local(&at, zone).year()
        ));
    }
    Ok(at)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Asia::Kuala_Lumpur;

    use super::*;

    fn now() -> DateTime<Utc> {
        // Wed 09 Sep 2026 12:00 +08:00.
        Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap()
    }

    fn read(text: &str) -> Result<String, String> {
        parse_when(text, Kuala_Lumpur, now()).map(|at| when(&at))
    }

    fn when(at: &DateTime<Utc>) -> String {
        crate::chat::tools::read::format::when_label(at, Kuala_Lumpur)
    }

    #[test]
    fn the_guilds_own_forms_resolve_forward() {
        assert_eq!(read("wed 21:30").unwrap(), "Wed 09 Sep 21:30");
        assert_eq!(read("wed 09:00").unwrap(), "Wed 16 Sep 09:00");
        assert_eq!(read("sat 10").unwrap(), "Sat 12 Sep 10:00");
        assert_eq!(read("sat 12").unwrap(), "Sat 12 Sep 12:00");
        assert_eq!(read("tomorrow 9:45pm").unwrap(), "Thu 10 Sep 21:45");
        assert_eq!(read("tmr 2300").unwrap(), "Thu 10 Sep 23:00");
        assert_eq!(read("2300").unwrap(), "Wed 09 Sep 23:00");
        assert_eq!(read("11am").unwrap(), "Thu 10 Sep 11:00");
        assert_eq!(read("2026-09-20 21:30").unwrap(), "Sun 20 Sep 21:30");
        assert_eq!(read("yesterday 21:00").unwrap(), "Tue 08 Sep 21:00");
    }

    #[test]
    fn dateparser_only_forms_are_refused_with_v4s_words() {
        for text in [
            "whenever",
            "",
            "12 september",
            "in 2 hours",
            "12 september 9pm",
        ] {
            assert_eq!(read(text), Err(unreadable(text)), "{text}");
        }
        assert!(
            read("2030-01-01 21:00")
                .unwrap_err()
                .contains("that lands in 2030")
        );
    }
}
