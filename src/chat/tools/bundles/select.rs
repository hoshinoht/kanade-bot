//! Which bundles a question starts with, chosen by code: the local
//! pre-screen's intent label when it ran, deterministic rules always, and the
//! card the member replies to. Every source only adds, so an unclear
//! question gets more tools, never fewer.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use super::Bundle;

/// The pre-screen's intent label (the same single local request).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Read,
    Strategy,
    RunChange,
    WeeklyChange,
    Unclear,
}

/// A proposal card the question is about (replied to, or the channel's
/// focus card).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardContext {
    Run,
    Weekly,
}

/// What routing may look at; all trusted or defused text.
#[derive(Clone, Copy, Debug, Default)]
pub struct Signals<'a> {
    /// The member's question.
    pub text: &'a str,
    /// `None` when the pre-screen was skipped (e.g. under load).
    pub intent: Option<Intent>,
    pub card: Option<CardContext>,
}

fn words(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("pattern")
}

static STRATEGY: LazyLock<Regex> = LazyLock::new(|| {
    words(
        r"\b(?:strat|strats|strategy|strategies|mechanics?|guides?|patterns?|phases?|tips?|dodge|survive|how (?:do|to|should) (?:i|we|you) (?:beat|clear|do|kill|survive))\b",
    )
});

static WEEKLY: LazyLock<Regex> = LazyLock::new(|| {
    words(
        r"\b(?:weekly|weeklies|every (?:week|mon|tue|wed|thu|fri|sat|sun)\w*|each week|recurring|fixed|permanent(?:ly)?|from now on|going forward|regular(?:ly)?|usual (?:time|night|day))\b",
    )
});

static RUN_CHANGE: LazyLock<Regex> = LazyLock::new(|| {
    words(
        r"\b(?:move|moved|moving|resched\w*|push(?:ed)?|postpone\w*|delay\w*|earlier|shift\w*|cancel\w*|call(?:ing)? (?:it )?off|skip\w*|add|set up|book|new run|rsvp|can'?t make|cannot make|can make|make it|count me|i'?m (?:in|out)|join|swap|sub)\b",
    )
});

/// The optional bundles to start a question with.
pub fn select(signals: Signals<'_>) -> BTreeSet<Bundle> {
    let mut out = BTreeSet::new();
    match signals.intent {
        Some(Intent::Strategy) => {
            out.insert(Bundle::Strategy);
        }
        Some(Intent::RunChange) => {
            out.insert(Bundle::RunWrites);
        }
        Some(Intent::WeeklyChange) => {
            out.insert(Bundle::FixedWrites);
        }
        Some(Intent::Unclear) => out.extend(Bundle::OPTIONAL),
        Some(Intent::Read) | None => {}
    }
    let text = signals.text;
    if STRATEGY.is_match(text) {
        out.insert(Bundle::Strategy);
    }
    if WEEKLY.is_match(text) {
        out.insert(Bundle::FixedWrites);
    }
    if RUN_CHANGE.is_match(text) {
        out.insert(Bundle::RunWrites);
    }
    match signals.card {
        Some(CardContext::Run) => {
            out.insert(Bundle::RunWrites);
        }
        Some(CardContext::Weekly) => {
            out.insert(Bundle::FixedWrites);
        }
        None => {}
    }
    out
}
