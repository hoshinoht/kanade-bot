//! Fact-check an id-less sentence against the listing's records by date and
//! time (`D-GROUND-FILTERED`, user decisions 2026-10-03). Fail-closed: the
//! sentence is kept only when every fact-like token in it was read and
//! matches the one run it picks; anything left over falls back.

mod boss;

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};
use crate::domain::catalog::BossTable;
use boss::bosses_match;

static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
static WHEN: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"\*([A-Z][a-z]{2}) ([0-9]{1,2}) ([A-Z][a-z]{2}) · ([0-9]{1,2}):([0-9]{2})\*")
});
static STATUS: LazyLock<Regex> = LazyLock::new(|| pattern(r" · `([a-z_]+)`"));
static TALLY: LazyLock<Regex> = LazyLock::new(|| pattern(r"`([0-9]+)/([0-9]+) yes`"));
static CHANNEL: LazyLock<Regex> = LazyLock::new(|| pattern(r"<#([0-9]+)>"));
static BOSS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\*\*(.+?)\*\*"));

static TIME: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b([0-9]{1,2}):([0-9]{2})\b"));
const MONTHS: &str = "jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec";
static DAY_MONTH: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(&format!(
        r"\b([0-9]{{1,2}})(?:st|nd|rd|th)?\s+({MONTHS})[a-z]*\.?(?:\s|$|[^\w])"
    ))
});
static MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(&format!(
        r"\b({MONTHS})[a-z]*\.?\s+([0-9]{{1,2}})(?:st|nd|rd|th)?\b"
    ))
});
static WEEKDAY: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(r"\b(mon|tue|wed|thu|fri|sat|sun)(?:day|s|sday|nesday|r|rs|rsday|urday)?\b")
});
/// A tally and the `have said yes` after it.
static SAID_TALLY: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(
        r"\b([0-9]+)\s*(?:/|of|out\s+of)\s*([0-9]+)\b(?:\s+(?:have\s+|has\s+)?(?:said\s+)?yes\b)?",
    )
});
/// Answer words; each must lie inside a read tally (`0 of 3 have said yes`).
static SAID_ANSWER: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(r"\b(?:yes|rsvp[a-z]*|answered|replied|responded|declined|accepted|signed\s+up)\b")
});
/// Status words; `own time` is the `otot` status in words.
static SAID_STATUS: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(r"\b(planned|confirmed|at[\s_-]+risk|otot|done|cancell?ed|own[\s-]+time)\b")
});
static SAID_CHANNEL: LazyLock<Regex> = LazyLock::new(|| pattern(r"<#([0-9]+)>|\[#([0-9]+)\]"));
/// Facts no check reads, so their presence alone falls back: a relative day
/// (grounding has no clock), a negation, an am/pm marker, a `#name` channel
/// (the listing has only `<#id>`), a bare 8-hex run id, and counts, dates
/// and statuses in words that no check reads (number words, ordinals,
/// quantifiers, `un`-statuses and status synonyms).
static UNREAD: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(concat!(
        r"\b(?:today|tonight|tonite|tomorrow|tmrw?|tmw|yesterday|(?:this|next|last|coming)\s+(?:week|weekend|month|morning|afternoon|evening|night|mon|tue|wed|thu|fri|sat|sun)[a-z]*|in\s+[0-9]+\s+(?:days?|weeks?)|day\s+after)\b",
        r"|\b(?:not|never|cannot|no\s+longer)\b|n['’]t\b",
        r"|\b[ap]\.\s?m\b|\bpm\b|[0-9]\s*am\b",
        r"|(?:^|[^<\[\w])#[a-z_][\w-]*",
        r"|\b[0-9a-f]{8}\b",
        r"|\b(?:zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|dozen|half|both|couple|several|few)\b",
        r"|\b(?:(?:twenty|thirty)[\s-]?)?(?:first|second|third|fourth|fifth|sixth|seventh|eighth|ninth)\b",
        r"|\b(?:tenth|eleventh|twelfth|thirteenth|fourteenth|fifteenth|sixteenth|seventeenth|eighteenth|nineteenth|twentieth|thirtieth)\b",
        r"|\b(?:everyone|everybody|nobody|no[\s-]?one|none|all|anyone|anybody|someone|somebody)\b",
        r"|\bun(?:confirmed|planned|scheduled|decided|answered)\b",
        r"|\b(?:call(?:ed|ing)?\s+off|postponed?|rescheduled?|moved|finished|completed?|cleared|over|happened|ended|delayed|pushed\s+back|scrapped|tentative|pending|wrapped)\b",
    ))
});

/// One listed run's checkable facts.
pub(super) struct Dated {
    id: String,
    boss: String,
    weekday: String,
    day: u32,
    month: String,
    time: (u32, u32),
    status: Option<String>,
    tally: Option<(u32, u32)>,
    channel: Option<String>,
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
                boss: BOSS
                    .captures(paragraph)
                    .map(|found| found[1].to_lowercase())
                    .unwrap_or_default(),
                weekday: when[1].to_lowercase(),
                day: when[2].parse().ok()?,
                month: when[3].to_lowercase(),
                time: (when[4].parse().ok()?, when[5].parse().ok()?),
                status: STATUS.captures(paragraph).map(|found| found[1].to_owned()),
                tally: TALLY
                    .captures(paragraph)
                    .and_then(|found| Some((found[1].parse().ok()?, found[2].parse().ok()?))),
                channel: CHANNEL.captures(paragraph).map(|found| found[1].to_owned()),
            })
        })
        .collect()
}

/// Byte flags of what the checks have read.
struct Read(Vec<bool>);

impl Read {
    fn mark(&mut self, start: usize, end: usize) {
        self.0[start..end].iter_mut().for_each(|read| *read = true);
    }

    /// A digit, in any script, no check read.
    fn digit_left(&self, lowered: &str) -> bool {
        lowered
            .char_indices()
            .any(|(at, c)| c.is_numeric() && !self.0[at])
    }

    fn covers(&self, start: usize, end: usize) -> bool {
        self.0[start..end].iter().all(|read| *read)
    }
}

/// `(day, month)` dates a sentence names with their spans; `MONTH_DAY` never
/// reads `Oct 21:00` as the 21st (the lookahead is emulated by checking the
/// next char).
fn dates(lowered: &str) -> Vec<((u32, String), (usize, usize))> {
    let mut found: Vec<((u32, String), (usize, usize))> = DAY_MONTH
        .captures_iter(lowered)
        .filter_map(|c| {
            let span = c.get(0)?;
            Some((
                (c[1].parse().ok()?, c[2].to_owned()),
                (span.start(), span.end()),
            ))
        })
        .collect();
    for c in MONTH_DAY.captures_iter(lowered) {
        let Some(span) = c.get(0) else { continue };
        if lowered[span.end()..].starts_with(':') {
            continue;
        }
        if let Ok(day) = c[2].parse() {
            found.push(((day, c[1].to_owned()), (span.start(), span.end())));
        }
    }
    found
}

/// The id of the run a sentence identifies by its one time (narrowed by a
/// stated date, else weekday, else unique, else the boss it names), when
/// every fact-like token in it was read and matches that run: time, date,
/// weekday, tally, status, channel, catalog boss, difficulty and bold span,
/// with no relative day, negation, am/pm, `#name` or id, and no digit left
/// unread. `None` (fall back) otherwise.
pub(super) fn named_by_date(
    line: &str,
    records: &[Dated],
    catalog: &BossTable,
) -> Option<BTreeSet<String>> {
    // ASCII lowering keeps byte offsets shared with `line`.
    let lowered = line.to_ascii_lowercase();
    if UNREAD.is_match(&lowered) {
        return None;
    }
    let mut read = Read(vec![false; lowered.len()]);
    let mut times: BTreeSet<(u32, u32)> = BTreeSet::new();
    for c in TIME.captures_iter(&lowered) {
        times.insert((c[1].parse().ok()?, c[2].parse().ok()?));
        let span = c.get(0)?;
        read.mark(span.start(), span.end());
    }
    let [time] = times.into_iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    let dates = dates(&lowered);
    let weekdays: BTreeSet<String> = WEEKDAY
        .captures_iter(&lowered)
        .map(|c| c[1].to_owned())
        .collect();
    let mut found: Vec<&Dated> = records.iter().filter(|r| r.time == time).collect();
    if !dates.is_empty() {
        found.retain(|r| {
            dates
                .iter()
                .any(|(date, _)| *date == (r.day, r.month.clone()))
        });
    } else if !weekdays.is_empty() {
        found.retain(|r| weekdays.contains(&r.weekday));
    }
    if found.len() > 1 {
        found.retain(|r| bosses_match(&lowered, line, &r.boss, catalog));
    }
    let [run] = found[..] else { return None };
    for ((day, month), (start, end)) in &dates {
        if (*day, month.as_str()) != (run.day, run.month.as_str()) {
            return None;
        }
        read.mark(*start, *end);
    }
    if weekdays.iter().any(|weekday| *weekday != run.weekday) {
        return None;
    }
    for c in SAID_TALLY.captures_iter(&lowered) {
        if c[1].parse().ok().zip(c[2].parse().ok()) != run.tally {
            return None;
        }
        let span = c.get(0)?;
        read.mark(span.start(), span.end());
    }
    if SAID_ANSWER
        .find_iter(&lowered)
        .any(|m| !read.covers(m.start(), m.end()))
    {
        return None;
    }
    for c in SAID_STATUS.captures_iter(&lowered) {
        let said = c[1]
            .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        let said = match said.as_str() {
            "own_time" => "otot".to_owned(),
            cancel if cancel.starts_with("cancel") => "cancelled".to_owned(),
            _ => said,
        };
        if run.status.as_deref() != Some(said.as_str()) {
            return None;
        }
    }
    for c in SAID_CHANNEL.captures_iter(&lowered) {
        let said = c.get(1).or(c.get(2)).map(|m| m.as_str());
        if run.channel.as_deref() != said {
            return None;
        }
        let span = c.get(0)?;
        read.mark(span.start(), span.end());
    }
    (!read.digit_left(&lowered) && bosses_match(&lowered, line, &run.boss, catalog))
        .then(|| BTreeSet::from([run.id.clone()]))
}
