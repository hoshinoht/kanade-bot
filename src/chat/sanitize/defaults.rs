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
    pub upcoming_only: bool,
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

fn names_a_person(text: &str) -> bool {
    FOR.find_iter(text).any(|found| {
        let rest = &text[found.end()..];
        !NOT_A_PERSON.is_match(rest) && PERSON_START.is_match(rest)
    })
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
        upcoming_only: UPCOMING.is_match(cleaned),
    }
}
