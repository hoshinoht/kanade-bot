//! Boss tokens and parties from an untrusted model (v4
//! `tools/participants.py`): the asker is the default, the bot is never a
//! member, and nothing is guessed.

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde_json::Value;

use super::ToolWorld;
use super::format::boss_label;
use super::roster::{bossers, resolve_participant_text};
use crate::chat::tools::{ToolContext, ToolError, ToolResult};
use crate::domain::pytext::strip;
use crate::domain::schedule::validate_participants as roster_gate;

static TOKENISH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z][A-Za-z0-9]+\b").expect("pattern"));
static FIRST_PERSON: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"\b(?:me|myself|i)\b")
        .case_insensitive(true)
        .build()
        .expect("pattern")
});

/// Words that join names; forgiven only when they matched nobody.
const JOINING_WORDS: [&str; 23] = [
    "a", "add", "along", "also", "and", "both", "for", "it", "just", "me", "on", "party", "please",
    "plus", "run", "team", "the", "then", "too", "us", "with", "&", "+",
];

/// v4 truthy spellings for `weekly`.
const TRUTHY: [&str; 7] = ["true", "yes", "y", "1", "weekly", "recurring", "fixed"];

/// Python `str(value or "")` for a JSON argument.
pub fn py_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Number(number)) => {
            if number.as_f64() == Some(0.0) {
                String::new()
            } else {
                number.to_string()
            }
        }
        Some(Value::Array(items)) if items.is_empty() => String::new(),
        Some(Value::Object(map)) if map.is_empty() => String::new(),
        Some(other) => other.to_string(),
    }
}

/// A list argument joined with `, `, else [`py_text`].
fn party_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => py_text(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => py_text(other),
    }
}

/// Canonical boss tokens, or a refusal that asks instead of choosing.
pub fn validate_bosses(world: &ToolWorld<'_>, text: &str) -> ToolResult<Vec<String>> {
    if strip(text).is_empty() {
        return Err(ToolError::new("Ask them which boss they mean."));
    }
    world.catalog.parse(text).map_err(|error| {
        ToolError(format!(
            "{}. Ask them which one they mean -- do not choose a difficulty for them. Ask in words ('Easy, Normal or Hard Bellona?') and pass the short form (HBellona) back to the tool. The short forms are for the tool only -- never show them to a member.",
            spell_out(world, error.message())
        ))
    })
}

/// Annotate canonical tokens in a refusal with the words a member would say.
fn spell_out(world: &ToolWorld<'_>, message: &str) -> String {
    TOKENISH
        .replace_all(message, |found: &regex::Captures<'_>| {
            let token = &found[0];
            match world.catalog.detail(token) {
                Some(_) => format!("{token} ({})", boss_label(token)),
                None => token.to_owned(),
            }
        })
        .into_owned()
}

/// Drop bot references from a party field; reports whether one was there.
fn without_the_bot(ctx: &ToolContext, text: &str) -> (String, bool) {
    let mut cleaned = text.to_owned();
    if let Some(bot) = ctx.bot_user_id.as_deref().filter(|id| !id.is_empty()) {
        let escaped = regex::escape(bot);
        let mention = Regex::new(&format!("<@!?{escaped}>")).expect("pattern");
        cleaned = mention.replace_all(&cleaned, " ").into_owned();
        let bare = Regex::new(&format!(r"\b{escaped}\b")).expect("pattern");
        cleaned = bare.replace_all(&cleaned, " ").into_owned();
    }
    let mut names: Vec<&str> = ctx.bot_names.iter().map(String::as_str).collect();
    names.dedup();
    for name in names.into_iter().filter(|name| !name.is_empty()) {
        let pattern = RegexBuilder::new(&format!(r"\b{}\b", regex::escape(name)))
            .case_insensitive(true)
            .build()
            .expect("pattern");
        cleaned = pattern.replace_all(&cleaned, " ").into_owned();
    }
    let changed = cleaned != text;
    (cleaned, changed)
}

/// A new party: the asker by default, never the model's guess and never the bot.
pub fn validate_participants(
    world: &ToolWorld<'_>,
    ctx: &ToolContext,
    value: Option<&Value>,
) -> ToolResult<Vec<String>> {
    let raw = party_text(value);
    let (without_bot, named_the_bot) = without_the_bot(ctx, &raw);
    let asker = format!("<@{}>", ctx.author_id);
    let raw = FIRST_PERSON.replace_all(&without_bot, regex::NoExpand(&asker));
    if strip(&raw).is_empty() {
        return Ok(vec![ctx.author_id.clone()]);
    }
    let roster = bossers(world.members);
    let resolution = resolve_participant_text(&raw, &roster);
    let strangers: Vec<&str> = resolution
        .unknown
        .iter()
        .map(String::as_str)
        .filter(|word| !JOINING_WORDS.contains(&word.to_lowercase().as_str()))
        .collect();
    if !strangers.is_empty() {
        return Err(ToolError(format!(
            "Nobody on the roster matches {}. Ask them who should be on it, or leave it as just them.",
            strangers.join(", ")
        )));
    }
    if !resolution.ambiguous.is_empty() {
        let options: Vec<String> = resolution
            .ambiguous
            .iter()
            .map(|(key, names)| format!("{key}: {}", names.join(", ")))
            .collect();
        return Err(ToolError(format!(
            "Ask them which they mean -- {}.",
            options.join("; ")
        )));
    }
    let bot = ctx.bot_user_id.as_deref().unwrap_or_default();
    let people: Vec<String> = resolution
        .ids
        .into_iter()
        .filter(|uid| uid != bot)
        .collect();
    if people.is_empty() {
        return Ok(vec![ctx.author_id.clone()]);
    }
    let mut named = roster_gate(&world.directory, &people)
        .map_err(|error| ToolError(format!("{error}. Ask them who should be on it.")))?;
    if named_the_bot && !named.contains(&ctx.author_id) {
        named.insert(0, ctx.author_id.clone());
    }
    Ok(named)
}

/// A replacement weekly party, or `None` to keep the current one.
pub fn new_party(
    world: &ToolWorld<'_>,
    ctx: &ToolContext,
    value: Option<&Value>,
) -> ToolResult<Option<Vec<String>>> {
    let raw = party_text(value);
    let (without_bot, _) = without_the_bot(ctx, &raw);
    if strip(&without_bot).is_empty() {
        return Ok(None);
    }
    validate_participants(world, ctx, value).map(Some)
}

/// The model's tolerant boolean for `weekly`.
pub fn is_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => TRUTHY.contains(&strip(text).to_lowercase().as_str()),
        _ => false,
    }
}
