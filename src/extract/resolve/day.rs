use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use regex::Regex;

use super::bound::BOUND_WORDS;
use crate::domain::pytext::strip;
use crate::domain::time::DateOutOfRange;
use crate::extract::text::{int, pattern};

/// Every weekday spelling this guild uses (v4 `resolve.WEEKDAY_ALIASES`).
pub const WEEKDAY_ALIASES: [(&str, Weekday); 19] = [
    ("mon", Weekday::Mon),
    ("tue", Weekday::Tue),
    ("wed", Weekday::Wed),
    ("thu", Weekday::Thu),
    ("fri", Weekday::Fri),
    ("sat", Weekday::Sat),
    ("sun", Weekday::Sun),
    ("monday", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("tues", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("weds", Weekday::Wed),
    ("wedns", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("thur", Weekday::Thu),
    ("thurs", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sunday", Weekday::Sun),
];

/// The day the message was sent.
const TODAY_WORDS: [&str; 6] = [
    "today",
    "tonight",
    "tonite",
    "tnite",
    "this evening",
    "this night",
];
/// The same day, but vaguely enough that a past time rolls into tomorrow.
const SOON_WORDS: [&str; 7] = ["now", "later", "ltr", "l8r", "soon", "in a bit", "just now"];
const TOMORROW_WORDS: [&str; 8] = [
    "tmr",
    "tmrw",
    "tmmr",
    "tmr night",
    "tomorrow",
    "tomorow",
    "tomm",
    "2mr",
];
const YESTERDAY_WORDS: [&str; 2] = ["ytd", "yesterday"];
const MONTH_ALIASES: [(&str, u32); 24] = [
    ("jan", 1),
    ("january", 1),
    ("feb", 2),
    ("february", 2),
    ("mar", 3),
    ("march", 3),
    ("apr", 4),
    ("april", 4),
    ("may", 5),
    ("jun", 6),
    ("june", 6),
    ("jul", 7),
    ("july", 7),
    ("aug", 8),
    ("august", 8),
    ("sep", 9),
    ("sept", 9),
    ("september", 9),
    ("oct", 10),
    ("october", 10),
    ("nov", 11),
    ("november", 11),
    ("dec", 12),
    ("december", 12),
];

static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{4})-(\d{2})-(\d{2})$"));
/// `17/10`: day first, as this SG/MY guild writes dates.
static DAY_SLASH_MONTH: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b(\d{1,2})/(\d{1,2})\b"));
static DAY_NUMBER: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{1,2})(st|nd|rd|th)?$"));
/// Words that only qualify a day and never change it.
static DAY_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\b(?:this|coming|on|the|at|around|about|ard|by|from|night|nite|evening|morning|afternoon|onwards?|ish|latest|earliest|sharp|pls|please)\b",
    )
});
static NEXT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bnext\b"));
static NON_WORD: LazyLock<Regex> = LazyLock::new(|| pattern("[^a-z0-9]+"));

pub(super) struct DayRef {
    pub day: NaiveDate,
    /// A specific day was named, so a past clock time must not roll forward.
    pub explicit: bool,
}

fn weekday_alias(word: &str) -> Option<Weekday> {
    WEEKDAY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == word)
        .map(|&(_, weekday)| weekday)
}

fn month_alias(word: &str) -> Option<u32> {
    MONTH_ALIASES
        .iter()
        .find(|(alias, _)| *alias == word)
        .map(|&(_, month)| month)
}

/// The words left once `next`, bound words ("after") and noise are blanked out.
fn cleaned_words(text: &str) -> Vec<String> {
    let cleaned = NEXT.replace_all(text, " ");
    let cleaned = BOUND_WORDS.replace_all(&cleaned, " ");
    let cleaned = DAY_NOISE.replace_all(&cleaned, " ");
    NON_WORD
        .split(&cleaned)
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn shift(day: NaiveDate, days: i64) -> Result<NaiveDate, DateOutOfRange> {
    day.checked_add_signed(TimeDelta::days(days))
        .filter(|day| (1..=9999).contains(&day.year()))
        .ok_or(DateOutOfRange)
}

fn iso_date(text: &str) -> Option<Option<NaiveDate>> {
    let found = ISO_DATE.captures(text)?;
    let part = |index: usize| int(&found[index]);
    let date = match (part(1), part(2), part(3)) {
        (Some(year), Some(month), Some(day)) if year >= 1 => i32::try_from(year)
            .ok()
            .and_then(|year| NaiveDate::from_ymd_opt(year, month, day)),
        _ => None,
    };
    Some(date)
}

/// The first `day` of a month on or after `anchor` (with `month`, of a year):
/// `17th` said on the 15th is this month's, said on the 20th next month's.
/// A day that month lacks (`31st` in September) resolves to nothing rather
/// than skipping to a month that has it.
fn next_date(anchor: NaiveDate, month: Option<u32>, day: u32) -> Option<NaiveDate> {
    let (year, month) = match month {
        Some(month) if (month, day) >= (anchor.month(), anchor.day()) => (anchor.year(), month),
        Some(month) => (anchor.year() + 1, month),
        None if day >= anchor.day() => (anchor.year(), anchor.month()),
        None if anchor.month() == 12 => (anchor.year() + 1, 1),
        None => (anchor.year(), anchor.month() + 1),
    };
    NaiveDate::from_ymd_opt(year, month, day).filter(|date| (1..=9999).contains(&date.year()))
}

/// A day of the month: `17th`, `17`, `17 oct`, `oct 17`, `17/10`, optionally
/// beside the weekday it falls on (`sat 17th`). `None`: not a date phrase;
/// `Some(None)`: a date that does not exist or is not on its weekday.
fn day_of_month(text: &str, anchor: NaiveDate) -> Option<Option<NaiveDate>> {
    let (mut day, mut month, rest) = match DAY_SLASH_MONTH.captures(text) {
        Some(found) => {
            let span = found.get(0)?;
            let rest = format!("{} {}", &text[..span.start()], &text[span.end()..]);
            (Some(int(&found[1])?), Some(int(&found[2])?), rest)
        }
        None => (None, None, text.to_owned()),
    };
    let mut bare = false;
    let mut weekday = None;
    for word in cleaned_words(&rest) {
        if day.is_none()
            && let Some(found) = DAY_NUMBER.captures(&word)
        {
            day = Some(int(&found[1])?);
            bare = found.get(2).is_none();
        } else if month.is_none()
            && let Some(named) = month_alias(&word)
        {
            month = Some(named);
        } else if weekday.is_none()
            && let Some(named) = weekday_alias(&word)
        {
            weekday = Some(named);
        } else {
            return None;
        }
    }
    let day = day.filter(|day| (1..=31).contains(day))?;
    // `sat 10` is a weekday and a stray hour, not the 10th.
    if bare && month.is_none() && weekday.is_some() {
        return None;
    }
    let date = next_date(anchor, month, day)
        .filter(|date| weekday.is_none_or(|weekday| date.weekday() == weekday));
    Some(date)
}

pub(super) fn parse_day(
    day_ref: Option<&str>,
    anchor: NaiveDate,
) -> Result<Option<DayRef>, DateOutOfRange> {
    let Some(day_ref) = day_ref else {
        return Ok(None);
    };
    let text = strip(day_ref).to_lowercase();
    if text.is_empty() {
        return Ok(None);
    }
    if let Some(date) = iso_date(&text) {
        return Ok(date.map(|day| DayRef {
            day,
            explicit: true,
        }));
    }

    let wants_next = NEXT.is_match(&text);
    let words = cleaned_words(&text);
    let phrase = words.join(" ");
    let first = words.first().map(String::as_str);
    let names =
        |set: &[&str]| set.contains(&phrase.as_str()) || first.is_some_and(|w| set.contains(&w));
    let at =
        |days: i64, explicit: bool| shift(anchor, days).map(|day| Some(DayRef { day, explicit }));
    if names(&TODAY_WORDS) {
        return at(0, true);
    }
    if names(&SOON_WORDS) {
        return at(0, false);
    }
    if names(&TOMORROW_WORDS) {
        return at(1, true);
    }
    if names(&YESTERDAY_WORDS) {
        return at(-1, true);
    }
    if let Some(date) = day_of_month(&text, anchor) {
        return Ok(date.map(|day| DayRef {
            day,
            explicit: true,
        }));
    }
    for word in &words {
        let Some(target) = weekday_alias(word) else {
            continue;
        };
        let from = i64::from(anchor.weekday().num_days_from_monday());
        let mut ahead = (i64::from(target.num_days_from_monday()) - from).rem_euclid(7);
        if wants_next && ahead == 0 {
            ahead = 7;
        }
        // "wed" said on a Wednesday is today; the caller rolls it a week on
        // once the clock time has passed.
        return at(ahead, ahead != 0);
    }
    Ok(None)
}

/// `day_ref` names a weekday, so a past time rolls a week rather than a day.
pub(super) fn names_weekday(day_ref: Option<&str>) -> bool {
    day_ref.is_some_and(|text| {
        cleaned_words(&text.to_lowercase())
            .iter()
            .any(|word| weekday_alias(word).is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn day(text: &str, anchor: NaiveDate) -> Option<(NaiveDate, bool)> {
        parse_day(Some(text), anchor)
            .unwrap()
            .map(|found| (found.day, found.explicit))
    }

    #[test]
    fn a_day_of_the_month_is_an_explicit_date() {
        // Thu 15 Oct 2026.
        let anchor = date(2026, 10, 15);
        for text in [
            "17th",
            "17",
            "the 17th",
            "on 17th",
            "after 17th",
            "17 oct",
            "17th october",
            "oct 17",
            "17/10",
            "sat 17th",
            "sat 17/10",
        ] {
            assert_eq!(
                day(text, anchor),
                Some((date(2026, 10, 17), true)),
                "{text}"
            );
        }
    }

    #[test]
    fn a_day_of_the_month_never_points_back() {
        let anchor = date(2026, 10, 15);
        assert_eq!(day("15th", anchor), Some((date(2026, 10, 15), true)));
        assert_eq!(day("14th", anchor), Some((date(2026, 11, 14), true)));
        assert_eq!(day("31st", anchor), Some((date(2026, 10, 31), true)));
        assert_eq!(day("5 jan", anchor), Some((date(2027, 1, 5), true)));
        assert_eq!(day("14/10", anchor), Some((date(2027, 10, 14), true)));
        assert_eq!(
            day("5th", date(2026, 12, 20)),
            Some((date(2027, 1, 5), true))
        );
    }

    #[test]
    fn a_date_that_does_not_exist_resolves_to_nothing() {
        // September has no 31st; it does not skip to 31 October.
        assert_eq!(day("31st", date(2026, 9, 15)), None);
        // Said on 31 Jan, the 30th is February's, which has none.
        assert_eq!(day("30th", date(2026, 1, 31)), None);
        assert_eq!(day("31/9", date(2026, 9, 1)), None);
        assert_eq!(day("17/13", date(2026, 10, 15)), None);
        // 17 Oct 2026 is a Saturday, not a Friday.
        assert_eq!(day("fri 17th", date(2026, 10, 15)), None);
    }

    #[test]
    fn a_stray_number_beside_a_weekday_is_not_a_date() {
        let anchor = date(2026, 10, 15);
        assert_eq!(day("sat 9", anchor), Some((date(2026, 10, 17), true)));
        assert_eq!(day("in 2 days", anchor), None);
        assert_eq!(day("0930", anchor), None);
    }
}
