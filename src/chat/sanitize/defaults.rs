//! Trusted schedule-scope defaults read from the member's own question
//! (v4 `_schedule_defaults`); they override what the model passes.

use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};

/// Carried into the tool context for the whole answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScheduleDefaults {
    pub force_all_channels: bool,
    pub force_channel_scope: bool,
    pub force_group_schedule: bool,
    /// The asker's question is unambiguously about their own schedule.
    pub self_schedule_requested: bool,
    pub upcoming_only: bool,
    /// A singular "next run" question (v5, D-AUTO-FORWARD): one run, not a list.
    pub next_only: bool,
}

static TRAILING_PUNCTUATION: LazyLock<Regex> = LazyLock::new(|| pattern(r"[?!.,]+\s*\z"));
static ALL_CHANNELS: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:whole server|all channels)\b"));
static WHOLE_GROUP: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\b(?:whole group|everyone)\b"));
/// `fullmatch` of v4's `what(?:'s|s| is)\s+(?:on|for)\s+.+`.
static SCHEDULE_QUESTION: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\A(?:what(?:'s|s| is)\s+(?:on|for)\s+.+)\z"));
static CHANNEL_QUALIFIER: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:this channel|in here|here|our runs)\b"));
static PERSON_QUALIFIER: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(r"\b(?:for me|my runs|my schedule|am i|do i|i am|i'm|myself)\b|<@!?\d+>")
});
static SELF_REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:for me|my (?:boss )?(?:runs?|schedule))\b"));
/// `for <someone>`; the words that are not a person are rejected by
/// [`NOT_A_PERSON`] (v4's negative lookahead).
static FOR: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\bfor\s+"));
static NOT_A_PERSON: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(
        r"\A(?:this|next|today|tonight|tomorrow|tmr|tmrw|mon(?:day)?|tue(?:sday)?|wed(?:nesday)?|thu(?:rsday)?|fri(?:day)?|sat(?:urday)?|sun(?:day)?)\b",
    )
});
static PERSON_START: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\A(?:<@!?\d+>|[a-z])"));
static UPCOMING: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(
        r"\b(?:what(?:'s|’s| is)\s+left|runs?\s+left|remaining\s+runs?|upcoming\s+runs?|next\s+runs?)\b",
    )
});

/// Singular only: "next runs" and "next week" stay list reads.
static NEXT_RUN: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\bnext\s+(?:boss\s+)?run\b"));
static FOR_ME: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\bfor\s+me\b"));

/// Words a plain "next run" question may carry besides the phrase itself;
/// anything else (a boss, a day, "after", "and", a count) may narrow the
/// answer in ways `get_schedule` cannot, so the full list comes back.
const NEXT_RUN_WORDS: &[&str] = &[
    "when", "when's", "when’s", "whens", "what", "what's", "what’s", "whats", "is", "show", "tell",
    "me", "my", "our", "the", "in", "here", "this", "channel", "all", "channels", "do", "i",
    "have", "please",
];

/// A plain singular "next run" question ("when is my next run", "my next
/// boss run?", "when's our next run in here"); mentions and other words opt out.
fn singular_next_run(text: &str) -> bool {
    if !NEXT_RUN.is_match(text) {
        return false;
    }
    let remaining = NEXT_RUN.replace_all(text, " ");
    let remaining = FOR_ME.replace_all(&remaining, " ");
    if !remaining
        .chars()
        .all(|ch| ch.is_ascii_alphabetic() || ch.is_ascii_whitespace() || ch == '\'' || ch == '’')
    {
        return false;
    }
    remaining
        .split_whitespace()
        .all(|word| NEXT_RUN_WORDS.contains(&word.to_lowercase().as_str()))
}

fn names_a_person(text: &str) -> bool {
    FOR.find_iter(text).any(|found| {
        let rest = &text[found.end()..];
        !NOT_A_PERSON.is_match(rest) && PERSON_START.is_match(rest)
    })
}

const SELF_CONTEXT_WORDS: &[&str] = &[
    "what",
    "what's",
    "whats",
    "what’s",
    "is",
    "are",
    "on",
    "show",
    "list",
    "tell",
    "give",
    "me",
    "my",
    "the",
    "schedule",
    "run",
    "runs",
    "boss",
    "today",
    "tonight",
    "tomorrow",
    "tmr",
    "tmrw",
    "this",
    "next",
    "week",
    "in",
    "here",
    "channel",
    "all",
    "channels",
    "please",
    "when",
    "do",
    "i",
    "have",
    "about",
    "mon",
    "monday",
    "tue",
    "tuesday",
    "wed",
    "wednesday",
    "thu",
    "thursday",
    "fri",
    "friday",
    "sat",
    "saturday",
    "sun",
    "sunday",
];

/// Recover only a self-only schedule request. Unrecognized words or mixed
/// punctuation suppress the fallback rather than guessing another person.
fn self_only_schedule(text: &str) -> bool {
    if !SELF_REFERENCE.is_match(text) {
        return false;
    }
    let remaining = SELF_REFERENCE.replace_all(text, " ");
    if !remaining
        .chars()
        .all(|ch| ch.is_ascii_alphabetic() || ch.is_ascii_whitespace() || ch == '\'' || ch == '’')
    {
        return false;
    }
    remaining
        .split_whitespace()
        .all(|word| SELF_CONTEXT_WORDS.contains(&word.to_ascii_lowercase().as_str()))
}

/// Defaults for a complete question, with the bot's own mentions removed.
pub fn schedule_defaults(
    text: &str,
    bot_user_id: Option<&str>,
    self_role_id: Option<&str>,
) -> ScheduleDefaults {
    let mut cleaned = text.to_owned();
    if let Some(bot) = bot_user_id.filter(|id| !id.is_empty()) {
        let mention = pattern(&format!("<@!?{}>", regex::escape(bot)));
        cleaned = mention.replace_all(&cleaned, " ").into_owned();
    }
    if let Some(role) = self_role_id.filter(|id| !id.is_empty()) {
        let mention = pattern(&format!("<@&{}>", regex::escape(role)));
        cleaned = mention.replace_all(&cleaned, " ").into_owned();
    }
    let cleaned = TRAILING_PUNCTUATION.replace(&cleaned, "");
    let cleaned = crate::domain::pytext::strip(&cleaned);
    let all_channels = ALL_CHANNELS.is_match(cleaned);
    let whole_group = WHOLE_GROUP.is_match(cleaned);
    let complete_question = SCHEDULE_QUESTION.is_match(cleaned);
    let explicit_channel = CHANNEL_QUALIFIER.is_match(cleaned);
    let explicit_person = PERSON_QUALIFIER.is_match(cleaned) || names_a_person(cleaned);
    ScheduleDefaults {
        force_all_channels: all_channels || whole_group || (complete_question && !explicit_channel),
        force_channel_scope: complete_question && explicit_channel,
        force_group_schedule: (complete_question || whole_group) && !explicit_person,
        self_schedule_requested: self_only_schedule(cleaned) && !whole_group,
        upcoming_only: UPCOMING.is_match(cleaned),
        next_only: singular_next_run(cleaned),
    }
}
