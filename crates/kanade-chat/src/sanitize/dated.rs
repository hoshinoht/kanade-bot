//! The hard facts a reply line can state about listed runs: times and dates
//! (`D-GROUND-FILTERED`, user decision 2026-10-03, "keep wording, card
//! under"). A line names a run by a time that, narrowed by a stated date or
//! weekday, picks out one listed run; a time or date no listed run has is
//! the only fact that replaces a line. Nothing else is read.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};

static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
static WHEN: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"\*([A-Z][a-z]{2}) ([0-9]{1,2}) ([A-Z][a-z]{2}) · ([0-9]{1,2}):([0-9]{2})\*")
});
static BOSS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\*\*(.+?)\*\*"));
static TIME: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b([0-9]{1,2}):([0-9]{2})\b"));
/// Month names and their usual abbreviations only, so `maybe 2` or
/// `mario 3` is never a date.
const MONTHS: &str = r"(jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)";
static DAY_MONTH: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(&format!(r"\b([0-9]{{1,2}})(?:st|nd|rd|th)?\s+{MONTHS}\b")));
static MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(&format!(
        r"\b{MONTHS}\.?\s+([0-9]{{1,2}})(?:st|nd|rd|th)?\b"
    ))
});
static WEEKDAY: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(r"\b(mon|tue|wed|thu|fri|sat|sun)(?:day|s|sday|nesday|r|rs|rsday|urday)?\b")
});

/// One listed run's date and time.
pub(super) struct Dated {
    pub(super) id: String,
    /// The record's bold boss label as written.
    pub(super) label: String,
    weekday: String,
    date: (u32, String),
    time: (u32, u32),
}

/// The listing's records that carry a `*Dow DD Mon · HH:MM*` facts line.
pub(super) fn records(schedule: &str) -> Vec<Dated> {
    schedule
        .split("\n\n")
        .filter_map(|paragraph| {
            let id = RUN_ID.captures(paragraph)?[1].to_lowercase();
            let when = WHEN.captures(paragraph)?;
            Some(Dated {
                id,
                label: BOSS
                    .captures(paragraph)
                    .map(|found| found[1].to_owned())
                    .unwrap_or_default(),
                weekday: when[1].to_lowercase(),
                date: (when[2].parse().ok()?, when[3].to_lowercase()),
                time: (when[4].parse().ok()?, when[5].parse().ok()?),
            })
        })
        .collect()
}

fn times(text: &str) -> BTreeSet<(u32, u32)> {
    TIME.captures_iter(text)
        .filter_map(|c| Some((c[1].parse().ok()?, c[2].parse().ok()?)))
        .collect()
}

/// `(day, month)` dates, the month as its first three letters. `Oct 21:00`
/// is not the 21st (the lookahead is emulated by checking the next char).
fn dates(text: &str) -> BTreeSet<(u32, String)> {
    let lowered = text.to_ascii_lowercase();
    let month = |name: &str| name.chars().take(3).collect::<String>();
    let mut found: BTreeSet<(u32, String)> = DAY_MONTH
        .captures_iter(&lowered)
        .filter_map(|c| Some((c[1].parse().ok()?, month(&c[2]))))
        .collect();
    for c in MONTH_DAY.captures_iter(&lowered) {
        let Some(span) = c.get(0) else { continue };
        if lowered[span.end()..].starts_with(':') {
            continue;
        }
        if let Ok(day) = c[2].parse() {
            found.insert((day, month(&c[1])));
        }
    }
    found
}

/// Every time and date the listing states.
pub(super) struct Listed {
    times: BTreeSet<(u32, u32)>,
    dates: BTreeSet<(u32, String)>,
}

impl Listed {
    pub(super) fn of(schedule: &str) -> Self {
        Listed {
            times: times(schedule),
            dates: dates(schedule),
        }
    }

    /// Whether `line` states a time or date no listed run has.
    pub(super) fn contradicted(&self, line: &str) -> bool {
        !times(line).is_subset(&self.times) || !dates(line).is_subset(&self.dates)
    }
}

/// The runs `line` names by a time that, narrowed by its stated dates (else
/// weekdays), picks out exactly one listed run.
pub(super) fn named_by_time(line: &str, records: &[Dated]) -> BTreeSet<String> {
    let dates = dates(line);
    let weekdays: BTreeSet<String> = WEEKDAY
        .captures_iter(&line.to_ascii_lowercase())
        .map(|c| c[1].to_owned())
        .collect();
    times(line)
        .into_iter()
        .filter_map(|time| {
            let found: Vec<&Dated> = records
                .iter()
                .filter(|r| r.time == time)
                .filter(|r| {
                    if !dates.is_empty() {
                        dates.contains(&r.date)
                    } else {
                        weekdays.is_empty() || weekdays.contains(&r.weekday)
                    }
                })
                .collect();
            match found[..] {
                [run] => Some(run.id.clone()),
                _ => None,
            }
        })
        .collect()
}
