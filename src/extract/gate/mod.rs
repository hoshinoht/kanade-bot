//! The deterministic keyword gate (v4 `bot/extract/gate.py`).
//!
//! Every watched message is scored here before any model call. Strong signals
//! (`boss`, `time`, `day`, `verb`, `here`) wake the model on their own; weak ones
//! (`soon`, `run`, `agree`, `mention`) only keep a message in the burst, since
//! "ok" means something only next to a scheduling conversation.

mod bosses;
mod scan;

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

pub use bosses::{BossHit, BossLexicon, canonical_bosses, find_bosses};
pub use scan::{explicit_rsvp, find_days, find_mentions, find_times};

use super::text::pattern;

/// One gate signal; variants are in name order so sets sort like v4's reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Signal {
    Agree,
    Boss,
    Day,
    Here,
    Mention,
    Run,
    Soon,
    Time,
    Verb,
}

impl Signal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agree => "agree",
            Self::Boss => "boss",
            Self::Day => "day",
            Self::Here => "here",
            Self::Mention => "mention",
            Self::Run => "run",
            Self::Soon => "soon",
            Self::Time => "time",
            Self::Verb => "verb",
        }
    }

    /// Enough on its own to run an extraction.
    pub fn is_strong(self) -> bool {
        matches!(
            self,
            Self::Boss | Self::Time | Self::Day | Self::Verb | Self::Here
        )
    }
}

/// What the gate saw in one message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GateResult {
    pub signals: BTreeSet<Signal>,
    pub bosses: Vec<BossHit>,
    pub times: Vec<String>,
    pub days: Vec<String>,
    pub mentions: Vec<String>,
}

impl GateResult {
    /// Worth keeping in the burst and showing to the model.
    pub fn hit(&self) -> bool {
        !self.signals.is_empty()
    }

    /// Worth waking the model for on its own.
    pub fn strong(&self) -> bool {
        self.signals.iter().any(|signal| signal.is_strong())
    }

    /// Sorted signal names joined by `,`, or `-`.
    pub fn reasons(&self) -> String {
        if self.signals.is_empty() {
            return "-".to_owned();
        }
        let names: Vec<&str> = self.signals.iter().map(|signal| signal.as_str()).collect();
        names.join(",")
    }
}

static HERE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)@(?:here|everyone)\b"));

/// Score one message.
pub fn evaluate<S: AsRef<str>>(
    text: &str,
    lexicon: &BossLexicon<'_>,
    roster_ids: &[S],
) -> GateResult {
    let bosses = find_bosses(text, lexicon);
    let times = find_times(text);
    let days = find_days(text);
    let mentions = find_mentions(text, roster_ids);
    let tokens = scan::masked_tokens(text);
    let has = |words: &[&str]| tokens.iter().any(|token| words.contains(&token.as_str()));

    let mut signals = BTreeSet::new();
    let mut flag = |on: bool, signal: Signal| {
        if on {
            signals.insert(signal);
        }
    };
    flag(!bosses.is_empty(), Signal::Boss);
    flag(!times.is_empty(), Signal::Time);
    flag(!days.is_empty(), Signal::Day);
    flag(has(&scan::SOON_WORDS), Signal::Soon);
    flag(has(&scan::SCHEDULE_VERBS), Signal::Verb);
    flag(has(&scan::ACTIVITY_VERBS), Signal::Run);
    flag(
        tokens.iter().any(|token| scan::is_agree(token)),
        Signal::Agree,
    );
    flag(HERE.is_match(text), Signal::Here);
    flag(!mentions.is_empty(), Signal::Mention);

    GateResult {
        signals,
        bosses,
        times,
        days,
        mentions,
    }
}

/// Is this burst worth one model call? Any strong hit is; a burst of bare
/// answers only when the channel was just talking about scheduling.
pub fn should_extract(burst: &[GateResult], context_is_scheduling: bool) -> bool {
    burst.iter().any(GateResult::strong)
        || (context_is_scheduling && burst.iter().any(GateResult::hit))
}

/// A mention or `@here` plus a boss or a time: extract without waiting for the
/// debounce (v4 `pipeline.urgent`).
pub fn urgent(result: &GateResult) -> bool {
    let has_target =
        result.signals.contains(&Signal::Here) || result.signals.contains(&Signal::Mention);
    has_target && (result.signals.contains(&Signal::Boss) || result.signals.contains(&Signal::Time))
}
