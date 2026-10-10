//! Which silent staging line a question shows while it is answered (v4
//! `progress.placeholder_for` over `strategy.route_strategy_intent`). Pure:
//! no model, store or Discord. v4's patterns are kept verbatim except that
//! Python's `\s` is spelled `[\s\x1C-\x1F]` so it matches the same
//! characters; `\w` remains Rust's Unicode word class (named in
//! `tests/chat/staging.rs`).

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::chat::persona::{Staging, StagingState, py_strip};
use crate::domain::catalog::{BossReference, BossTable};

/// v4 grounds at most this many bosses at once; more is unresolved.
const MAX_BOSSES: usize = 3;

/// Characters v4 strips from each coordinated target.
const TARGET_TRIM: &[char] = &[
    ' ', '?', '!', '.', '\'', '"', '`', '*', '(', ')', '[', ']', '{', '}',
];

fn pattern(source: &str) -> Regex {
    RegexBuilder::new(source)
        .case_insensitive(true)
        .build()
        .expect("staging pattern")
}

static WRITE_HINT: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"\b(move|moves|moving|cancel|otot|rsvp|weekly|fixed|proposal|amend|reschedule)\b")
});

static SCHEDULE_HINT: LazyLock<Regex> = LazyLock::new(|| {
    pattern(concat!(
        r"\b(what'?s on|what is on|when is|schedule|tonight|today|tomorrow|",
        r"this week|next week|my runs|for me|monday|tuesday|wednesday|thursday|",
        r"friday|saturday|sunday|mon|tue|wed|thu|fri|sat|sun)\b",
    ))
});

static STRONG_CUE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(concat!(
        r"\b(?:attacks?|beat|defeat|guide|mechanics?|moves|patterns?|phase|gauge|parry|",
        r"dodge|requirements?|strategy|survive|survival|tips?)\b",
    ))
});

static WATCH_CUE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\bwatch[\s\x1C-\x1F]+(?:out[\s\x1C-\x1F]+)?for\b"));

static HOW_TO_MECHANICS: LazyLock<Regex> = LazyLock::new(|| {
    pattern(concat!(
        r"\bhow[\s\x1C-\x1F]+(?:(?:do|can|should)[\s\x1C-\x1F]+(?:\w+[\s\x1C-\x1F]+){0,3}",
        r"|to[\s\x1C-\x1F]+(?:\w+[\s\x1C-\x1F]+){0,2})",
        r"(?:clear|fight|handle|approach)\b",
    ))
});

static TARGET_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"[\s\x1C-\x1F]*(?:,|;|/|&|\+|\||•|\bvs\.?\b|\band\b)[\s\x1C-\x1F]*"));

/// Discord markup must not split targets: a role mention `<@&id>` has `&`.
static MENTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"<[@#][&!#]?\d+>|<a?:\w+:\d+>"));

static ACTION_CONTINUATION: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"^(?:avoid|parry|dodge|survive|handle|deal[\s\x1C-\x1F]+with|learn|understand|reveal|ignore)\b",
    )
});

static POLITE_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^(?:please|pls|thank(?:s|[\s\x1C-\x1F]+you))$"));

/// v4's strategy routing decision, without its clarification replies.
#[derive(Debug, PartialEq, Eq)]
enum Strategy {
    /// No strategy cue (v4 `not_strategy`).
    NotAsked,
    Unresolved,
    Resolved(Vec<BossReference>),
}

/// The staging line for `text`: named bosses (at most three, resolved only
/// through the catalog) → `guide_named` (falling back to `guide` when the
/// render is unsafe); any other strategy question → `guide`; then v4's write
/// and schedule hints; else `generic`.
pub fn staging_line(text: &str, bosses: &BossTable, staging: &Staging) -> String {
    let cleaned = py_strip(text);
    if cleaned.is_empty() {
        return staging.line(StagingState::Generic).to_owned();
    }
    let state = match route(cleaned, bosses) {
        Strategy::Resolved(references) => {
            let names: Vec<&str> = references.iter().map(|r| r.short.as_str()).collect();
            return staging.guide_named_for(&names.join(", "));
        }
        Strategy::Unresolved => StagingState::Guide,
        Strategy::NotAsked if WRITE_HINT.is_match(cleaned) => StagingState::Write,
        Strategy::NotAsked if SCHEDULE_HINT.is_match(cleaned) => StagingState::Schedule,
        Strategy::NotAsked => StagingState::Generic,
    };
    staging.line(state).to_owned()
}

fn has_strategy_cue(text: &str) -> bool {
    STRONG_CUE.is_match(text) || WATCH_CUE.is_match(text) || HOW_TO_MECHANICS.is_match(text)
}

fn route(text: &str, bosses: &BossTable) -> Strategy {
    if !has_strategy_cue(text) {
        return Strategy::NotAsked;
    }
    let references = match coordinated_segments(text, bosses) {
        Some(segments) => resolve_segments(&segments, bosses),
        None => bosses.resolve_reference(text).ok().map(|found| vec![found]),
    };
    match references {
        Some(references) if !references.is_empty() && references.len() <= MAX_BOSSES => {
            Strategy::Resolved(references)
        }
        _ => Strategy::Unresolved,
    }
}

/// Explicitly coordinated targets, when the wording supplies them; `None`
/// for a single target or wording before the first named boss.
fn coordinated_segments(text: &str, bosses: &BossTable) -> Option<Vec<String>> {
    let cleaned = MENTION.replace_all(text, " ");
    let mut segments: Vec<String> = TARGET_SPLIT
        .split(&cleaned)
        .map(|piece| piece.trim_matches(TARGET_TRIM))
        .filter(|piece| !piece.is_empty())
        .map(str::to_owned)
        .collect();
    if segments.len() < 2 {
        return None;
    }
    if segments
        .last()
        .is_some_and(|last| POLITE_SUFFIX.is_match(last))
    {
        segments.pop();
    }
    while segments.len() > 1
        && segments
            .last()
            .is_some_and(|last| ACTION_CONTINUATION.is_match(last))
    {
        segments.pop();
    }
    if segments.len() < 2 || bosses.resolve_reference(&segments[0]).is_err() {
        return None;
    }
    Some(segments)
}

fn resolve_segments(segments: &[String], bosses: &BossTable) -> Option<Vec<BossReference>> {
    let mut found = Vec::new();
    for segment in segments {
        let reference = bosses.resolve_reference(segment).ok()?;
        if !add_reference(&mut found, reference) {
            return None;
        }
    }
    Some(found)
}

/// Add a boss once, keeping an explicitly stated difficulty; two different
/// stated difficulties are ambiguous (`false`).
fn add_reference(found: &mut Vec<BossReference>, reference: BossReference) -> bool {
    let Some(existing) = found.iter_mut().find(|e| e.short == reference.short) else {
        found.push(reference);
        return true;
    };
    let compatible = existing.difficulty.is_none()
        || existing.difficulty == reference.difficulty
        || reference.difficulty.is_none();
    if existing.difficulty.is_none() && reference.difficulty.is_some() {
        *existing = reference;
    }
    compatible
}
