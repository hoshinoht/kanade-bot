use std::sync::LazyLock;

use regex::Regex;

use crate::domain::schedule::RsvpState;
use crate::extract::text::{NON_SPACE, SPACE, int, pattern, split_tail};

pub(super) const DAY_WORDS: [&str; 28] = [
    "mon",
    "monday",
    "tue",
    "tues",
    "tuesday",
    "wed",
    "weds",
    "wednesday",
    "thu",
    "thur",
    "thurs",
    "thursday",
    "fri",
    "friday",
    "sat",
    "saturday",
    "sun",
    "sunday",
    "tmr",
    "tmrw",
    "tomorrow",
    "tomorow",
    "tonight",
    "tonite",
    "tnite",
    "today",
    "ytd",
    "yesterday",
];

/// Real, but so common in this chat that they never trigger on their own.
pub(super) const SOON_WORDS: [&str; 5] = ["now", "later", "ltr", "l8r", "soon"];

pub(super) const SCHEDULE_VERBS: [&str; 32] = [
    "amend",
    "amended",
    "shift",
    "shifted",
    "change",
    "changed",
    "chg",
    "postpone",
    "postponed",
    "reschedule",
    "rescheduled",
    "resched",
    "cancel",
    "cancelled",
    "canceled",
    "skip",
    "skipping",
    "otot",
    "fixed",
    "schedule",
    "scheduled",
    "lockin",
    "delay",
    "delayed",
    "push",
    "swap",
    "reminder",
    "temp",
    "sub",
    "split",
    "arrange",
    "organise",
];

/// Weaker than [`SCHEDULE_VERBS`]: only counts next to something else.
pub(super) const ACTIVITY_VERBS: [&str; 5] = ["run", "runs", "clear", "bossing", "prac"];

const RSVP_NO: [&str; 7] = ["no", "nope", "kenot", "cannot", "cant", "cmi", "bobian"];

const RSVP_YES: [&str; 22] = [
    "can",
    "cancan",
    "ok",
    "okay",
    "oke",
    "okie",
    "okei",
    "okey",
    "ogei",
    "sure",
    "ya",
    "yaya",
    "yea",
    "yeah",
    "ye",
    "yes",
    "yup",
    "yupp",
    "yep",
    "confirm",
    "confirmed",
    "cfm",
];

const RSVP_OTHER_ACTIONS: [&str; 2] = ["cover", "take"];

/// `can`/`ok`/`kenot`: an answer, not a proposal.
pub(super) fn is_agree(token: &str) -> bool {
    RSVP_YES.contains(&token) || RSVP_NO.contains(&token)
}

static WORD: LazyLock<Regex> = LazyLock::new(|| pattern("[a-z0-9]+"));
/// `cc9`, `ch 7`: channel numbers, never times.
static CHANNEL_REF: LazyLock<Regex> =
    LazyLock::new(|| pattern(&format!(r"(?i)\b(?:cc|ch|c){SPACE}?\d{{1,2}}\b")));
/// Discord ids, phone numbers, item counts.
static LONG_NUMBER: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b\d{5,}\b"));
/// `$200` is not 2 a.m.
static PRICE: LazyLock<Regex> =
    LazyLock::new(|| pattern(&format!(r"[$\x{{a3}}\x{{20ac}}]{SPACE}?\d+(?:[.,]\d+)?")));
static MENTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"<@!?(\d+)>"));
static URL: LazyLock<Regex> = LazyLock::new(|| pattern(&format!("https?://{NON_SPACE}+")));
static EMOJI_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"<a?:\w+:\d+>"));
/// `lock in` is two words; `lockin` is the verb.
static LOCK_IN: LazyLock<Regex> = LazyLock::new(|| pattern(&format!(r"(?i)\block{SPACE}+in\b")));

/// Most specific first; the last (bare compact digits) is validated separately.
static TIME_PATTERNS: LazyLock<[Regex; 6]> = LazyLock::new(|| {
    let s = SPACE;
    let meridiem = r"(?:a\.?m\.?|p\.?m\.?)";
    [
        // 1030~11+pm / 8~1130 / 9-10pm: a range, the start is what matters
        pattern(&format!(
            r"(?i)\b\d{{1,2}}(?:[:.]?\d{{2}})?{s}*\+?{s}*[~\-]{s}*\d{{1,4}}{s}*\+?{s}*{meridiem}?"
        )),
        // 9:30pm / 9.30 pm / 9 pm / 9+pm / 12am
        pattern(&format!(
            r"(?i)\b\d{{1,2}}(?:[:.]\d{{2}})?{s}*\+?{s}*{meridiem}"
        )),
        // 21:30 / 9:30 / 9.30
        pattern(r"\b\d{1,2}[:.]\d{2}\b"),
        // 1130pm / 930 pm
        pattern(&format!(r"(?i)\b\d{{3,4}}{s}*\+?{s}*{meridiem}")),
        // at 11
        pattern(&format!(r"(?i)\bat{s}+\d{{1,2}}\b")),
        // bare compact 930 / 2130
        pattern(r"\b\d{3,4}\b"),
    ]
});

/// ASCII words, as v4's `[a-z0-9]+` finds them.
pub(super) fn words(text: &str) -> impl Iterator<Item = &str> {
    WORD.find_iter(text).map(|found| found.as_str())
}

/// Blank out things that look numeric but never mean a time.
fn mask(text: &str) -> String {
    let mut out = URL.replace_all(text, " ").into_owned();
    for masker in [&*EMOJI_ID, &*MENTION, &*CHANNEL_REF, &*PRICE, &*LONG_NUMBER] {
        out = masker.replace_all(&out, " ").into_owned();
    }
    out
}

/// Lowercased words with `lock in` joined.
fn tokens(text: &str) -> Vec<String> {
    let joined = LOCK_IN
        .replace_all(&text.to_lowercase(), "lockin")
        .into_owned();
    words(&joined).map(str::to_owned).collect()
}

/// The tokens [`super::evaluate`] and [`explicit_rsvp`] read: masked first.
pub(super) fn masked_tokens(text: &str) -> Vec<String> {
    tokens(&mask(text))
}

/// `930` yes, `290` no (minute 90), `2026` no (a year).
fn plausible_compact(digits: &str) -> bool {
    let (hour, minute) = split_tail(digits, 2);
    let (Some(hour), Some(minute)) = (int(hour), int(minute)) else {
        return false;
    };
    if minute > 59 || hour > 23 {
        return false;
    }
    let is_year =
        digits.chars().count() == 4 && int(digits).is_some_and(|n| (1900..=2100).contains(&n));
    !is_year
}

/// Clock expressions in `text`, as the literal (masked, stripped) substrings.
pub fn find_times(text: &str) -> Vec<String> {
    let masked = mask(text);
    let patterns = &*TIME_PATTERNS;
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (index, time) in patterns.iter().enumerate() {
        for found in time.find_iter(&masked) {
            let literal = crate::domain::pytext::strip(found.as_str());
            if index == patterns.len() - 1 && !plausible_compact(literal) {
                continue;
            }
            // Already covered by an earlier, more specific pattern.
            if spans
                .iter()
                .any(|&(start, end, _)| start <= found.start() && found.start() < end)
            {
                continue;
            }
            spans.push((found.start(), found.end(), literal.to_owned()));
        }
    }
    spans.sort();
    spans.into_iter().map(|(_, _, literal)| literal).collect()
}

/// Weekday and relative-day words, in order, de-duplicated.
pub fn find_days(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in masked_tokens(text) {
        if DAY_WORDS.contains(&token.as_str()) && !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

/// `<@id>` mentions of roster members; every mention when the roster is empty.
pub fn find_mentions<S: AsRef<str>>(text: &str, roster_ids: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for found in MENTION.captures_iter(text) {
        let uid = &found[1];
        let known = roster_ids.is_empty() || roster_ids.iter().any(|id| id.as_ref() == uid);
        if known && !out.iter().any(|seen| seen == uid) {
            out.push(uid.to_owned());
        }
    }
    out
}

/// A short, unambiguous attendance answer (`yes`/`no` only).
pub fn explicit_rsvp(text: &str) -> Option<RsvpState> {
    let tokens = masked_tokens(text);
    let has = |words: &[&str]| tokens.iter().any(|token| words.contains(&token.as_str()));
    if tokens.is_empty()
        || tokens.len() > 8
        || text.contains('?')
        || has(&["anot"])
        || has(&SCHEDULE_VERBS)
        || has(&RSVP_OTHER_ACTIONS)
    {
        return None;
    }
    if has(&RSVP_NO) {
        return Some(RsvpState::No);
    }
    has(&RSVP_YES).then_some(RsvpState::Yes)
}
