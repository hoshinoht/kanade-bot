//! Trusted schedule-scope defaults read from the member's own question
//! (v4 `_schedule_defaults`); they override what the model passes.

use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};
use crate::domain::members::MemberProfile;

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
/// The asker in the first person (`D-MIXED-PEOPLE`).
static FIRST_PERSON: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:i|me|my|mine|myself|we|us)\b"));
static MENTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"<@!?(\d+)>"));

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

/// The question with the bot's own user and role mentions removed.
fn without_bot(text: &str, bot_user_id: Option<&str>, self_role_id: Option<&str>) -> String {
    let mut cleaned = text.to_owned();
    if let Some(bot) = bot_user_id.filter(|id| !id.is_empty()) {
        let mention = pattern(&format!("<@!?{}>", regex::escape(bot)));
        cleaned = mention.replace_all(&cleaned, " ").into_owned();
    }
    if let Some(role) = self_role_id.filter(|id| !id.is_empty()) {
        let mention = pattern(&format!("<@&{}>", regex::escape(role)));
        cleaned = mention.replace_all(&cleaned, " ").into_owned();
    }
    cleaned
}

/// `name` as a whole word in `lowered` (both lowercase): v4-style word
/// bounds, so "Dune" never matches inside "Dunes".
fn names_word(lowered: &str, name: &str) -> Option<usize> {
    lowered.match_indices(name).map(|(at, _)| at).find(|&at| {
        let before = lowered[..at].chars().next_back();
        let after = lowered[at + name.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// `D-MIXED-PEOPLE`: the other roster members (role holders) a question
/// names by mention, display name, nickname or alias when it also refers to
/// the asker in the first person ("what do Bramble and I have on wed?").
/// Empty for self-only and third-person-only questions, which keep their
/// usual reads. Ordered by where each is first named.
pub fn schedule_people<'a>(
    text: &str,
    bot_user_id: Option<&str>,
    self_role_id: Option<&str>,
    author_id: &str,
    roster: impl IntoIterator<Item = &'a MemberProfile>,
) -> Vec<String> {
    let cleaned = without_bot(text, bot_user_id, self_role_id);
    if !FIRST_PERSON.is_match(&cleaned) {
        return Vec::new();
    }
    let lowered = cleaned.to_lowercase();
    let mentioned: Vec<(usize, &str)> = MENTION
        .captures_iter(&cleaned)
        .filter_map(|found| Some((found.get(0)?.start(), found.get(1)?.as_str())))
        .collect();
    let mut named: Vec<(usize, &str)> = Vec::new();
    for profile in roster {
        let member = &profile.member;
        if !member.has_role || member.user_id == author_id {
            continue;
        }
        let by_mention = mentioned
            .iter()
            .find(|(_, id)| *id == member.user_id)
            .map(|(at, _)| *at);
        let by_name = [member.display_name.as_deref(), member.nickname.as_deref()]
            .into_iter()
            .flatten()
            .chain(profile.aliases.iter().map(String::as_str))
            .map(|name| crate::domain::pytext::strip(name).to_lowercase())
            .filter(|name| !name.is_empty())
            .filter_map(|name| names_word(&lowered, &name))
            .min();
        if let Some(at) = by_mention.into_iter().chain(by_name).min() {
            named.push((at, member.user_id.as_str()));
        }
    }
    named.sort_unstable();
    named.into_iter().map(|(_, id)| id.to_owned()).collect()
}

/// Defaults for a complete question, with the bot's own mentions removed.
pub fn schedule_defaults(
    text: &str,
    bot_user_id: Option<&str>,
    self_role_id: Option<&str>,
) -> ScheduleDefaults {
    let cleaned = without_bot(text, bot_user_id, self_role_id);
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
