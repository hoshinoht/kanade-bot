use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use regex::Regex;

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

static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\d{4})-(\d{2})-(\d{2})$"));
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

/// The words left once `next` and noise are blanked out.
fn cleaned_words(text: &str) -> Vec<String> {
    let cleaned = NEXT.replace_all(text, " ");
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
