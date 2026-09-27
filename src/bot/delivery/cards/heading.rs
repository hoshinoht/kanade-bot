//! Persona-voiced reminder headers. Day-of keeps its original rewrite contract;
//! countdown and digest accept only a small, non-factual interjection.

use std::sync::Arc;

use serde_json::json;

use crate::chat::nudge::{
    NudgeRewriter, REWRITE_DEADLINE, RewriteFailure, RewritePrompt, SharedRewriter, accept_rewrite,
};
use crate::chat::persona::{CompiledPersona, NudgeMood};
use crate::domain::catalog::BossTable;
use crate::runtime::logging;

/// v4's heading, with the day left for after the rewrite.
pub const DAY_OF_HEADING_SEED: &str = "Today — {day}";
pub const COUNTDOWN_PHRASE_SEED: &str = "Onward!";
pub const DIGEST_PHRASE_SEED: &str = "Let's go!";
const DAY: &str = "{day}";
const MAX_PHRASE_CHARS: usize = 48;

const SAFE_PHRASES: &[&[&str]] = &[
    &["ah"],
    &["aha"],
    &["aah"],
    &["oh"],
    &["ooh"],
    &["woo"],
    &["yay"],
    &["whee"],
    &["yippee"],
    &["hurray"],
    &["hooray"],
    &["tada"],
    &["wow"],
    &["whoa"],
    &["hey"],
    &["hiya"],
    &["yo"],
    &["eek"],
    &["eep"],
    &["hmm"],
    &["hm"],
    &["ha"],
    &["haha"],
    &["hehe"],
    &["gosh"],
    &["onward"],
    &["go"],
    &["let's", "go"],
    &["here", "we", "go"],
    &["ta", "da"],
    &["woo", "hoo"],
    &["woohoo"],
    &["yahoo"],
    &["oh", "my"],
    &["oh", "wow"],
    &["waku"],
    &["waku", "waku"],
    &["わくわく"],
    &["やった"],
    &["よし"],
    &["よっしゃ"],
    &["ふふ"],
    &["えへへ"],
];

const FACT_WORDS: &[&str] = &[
    "today",
    "tonight",
    "tomorrow",
    "yesterday",
    "week",
    "day",
    "date",
    "time",
    "hour",
    "minute",
    "second",
    "morning",
    "afternoon",
    "evening",
    "night",
    "noon",
    "midnight",
    "am",
    "pm",
    "next",
    "last",
    "now",
    "later",
    "soon",
    "monday",
    "mon",
    "tuesday",
    "tue",
    "tues",
    "wednesday",
    "wed",
    "thursday",
    "thu",
    "thur",
    "thurs",
    "friday",
    "fri",
    "saturday",
    "sat",
    "sunday",
    "sun",
    "january",
    "jan",
    "february",
    "feb",
    "march",
    "mar",
    "april",
    "apr",
    "may",
    "june",
    "jun",
    "july",
    "jul",
    "august",
    "aug",
    "september",
    "sep",
    "sept",
    "october",
    "oct",
    "november",
    "nov",
    "december",
    "dec",
    "yes",
    "no",
    "confirm",
    "confirmed",
    "confirmation",
    "unconfirmed",
    "decline",
    "declined",
    "out",
    "pending",
    "answered",
    "answer",
    "ready",
    "set",
    "done",
    "cleared",
    "clear",
    "complete",
    "completed",
    "planned",
    "risk",
    "cancel",
    "cancelled",
    "canceled",
    "waiting",
    "everyone",
];

/// The guild default persona (bundle, no member profile), if chat has one.
pub type PersonaSource = Arc<dyn Fn() -> Option<CompiledPersona> + Send + Sync>;

/// Where a heading came from, for the `day_of_heading` log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadingSource {
    Rewrite,
    Seed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhraseKind {
    Countdown,
    Digest,
}

impl PhraseKind {
    pub fn seed(self) -> &'static str {
        match self {
            Self::Countdown => COUNTDOWN_PHRASE_SEED,
            Self::Digest => DIGEST_PHRASE_SEED,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Countdown => "countdown",
            Self::Digest => "digest",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhraseRejection {
    UnsafeLine,
    FactualTerm,
    CatalogTerm,
    NotAnInterjection,
}

impl HeadingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewrite => "rewrite",
            Self::Seed => "seed",
        }
    }
}

/// The `day_of_heading` log reason for a rewriter failure; operator
/// settings stay distinguishable from outages.
pub fn failure_reason(failure: RewriteFailure) -> &'static str {
    match failure {
        RewriteFailure::Unavailable => "unavailable",
        RewriteFailure::Refused => "refused",
        RewriteFailure::Misconfigured => "misconfigured",
    }
}

/// v4's heading for `day`.
pub fn seed_heading(day: &str) -> String {
    DAY_OF_HEADING_SEED.replace(DAY, day)
}

/// Accept only a short, code-bounded interjection; unknown prose is not
/// treated as safe merely because it lacks an obvious date or status word.
pub fn accept_phrase(
    output: &str,
    seed: &str,
    catalog: Option<&BossTable>,
) -> Result<String, PhraseRejection> {
    let line = accept_rewrite(output, seed).map_err(|_| PhraseRejection::UnsafeLine)?;
    if line.chars().any(char::is_numeric) {
        return Err(PhraseRejection::FactualTerm);
    }
    let tokens = phrase_tokens(&line).ok_or(PhraseRejection::NotAnInterjection)?;
    if tokens.is_empty() || line.chars().count() > MAX_PHRASE_CHARS || tokens.len() > 3 {
        return Err(PhraseRejection::NotAnInterjection);
    }
    if tokens
        .iter()
        .any(|token| FACT_WORDS.contains(&token.as_str()))
    {
        return Err(PhraseRejection::FactualTerm);
    }
    if catalog.is_some_and(|catalog| names_catalog_entry(&tokens, catalog)) {
        return Err(PhraseRejection::CatalogTerm);
    }
    if !SAFE_PHRASES.contains(
        &tokens
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
    ) {
        return Err(PhraseRejection::NotAnInterjection);
    }
    Ok(line)
}

fn phrase_tokens(line: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for character in line.chars() {
        if character.is_alphabetic() {
            word.extend(character.to_lowercase());
        } else if matches!(character, '\'' | '’') && !word.is_empty() {
            word.push('\'');
        } else if character.is_whitespace()
            || matches!(character, '!' | '?' | '.' | ',' | '！' | '？' | '。' | '，')
        {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
        } else {
            return None;
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    Some(tokens)
}

fn names_catalog_entry(tokens: &[String], catalog: &BossTable) -> bool {
    let contains = |value: &str| {
        let words = value
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        !words.is_empty()
            && tokens
                .windows(words.len())
                .any(|window| window.iter().zip(&words).all(|(left, right)| left == right))
    };
    for difficulty in catalog.difficulties() {
        if contains(difficulty.label()) || contains(difficulty.letter()) {
            return true;
        }
    }
    catalog.bosses().iter().any(|boss| {
        contains(boss.short())
            || contains(boss.full())
            || boss.aliases().iter().any(|alias| contains(alias))
            || boss
                .difficulties()
                .iter()
                .any(|letter| contains(&boss.canonical(letter)))
    })
}

/// The rewriter and persona; either missing means the seed.
#[derive(Clone, Default)]
pub struct HeadingRewrite {
    pub rewriter: Option<SharedRewriter>,
    pub persona: Option<PersonaSource>,
}

impl std::fmt::Debug for HeadingRewrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadingRewrite")
            .field("rewriter", &self.rewriter.is_some())
            .field("persona", &self.persona.is_some())
            .finish()
    }
}

impl HeadingRewrite {
    /// The prompt a rewrite would send, if one would be attempted.
    pub fn prompt(&self) -> Option<RewritePrompt> {
        self.rewriter.as_ref()?;
        let persona = (self.persona.as_ref()?)()?;
        Some(RewritePrompt::build(
            &persona,
            NudgeMood::Playful,
            DAY_OF_HEADING_SEED,
        ))
    }

    /// The heading for `day` (e.g. `Fri 25 Sep`); never slower than the
    /// rewrite deadline. Logs the source, never the text.
    pub async fn choose(&self, day: &str) -> (String, HeadingSource) {
        let (line, source, reason) = self.attempt().await;
        logging::event(
            "INFO",
            "day_of_heading",
            json!({"source": source.as_str(), "reason": reason}),
        );
        (line.replace(DAY, day), source)
    }

    /// A phrase stored before a countdown or digest claim. Its prompt contains
    /// only the persona and the code-owned seed.
    pub async fn choose_phrase(
        &self,
        kind: PhraseKind,
        catalog: Option<&BossTable>,
    ) -> (String, HeadingSource) {
        let seed = kind.seed();
        let (phrase, source, reason) = self.attempt_phrase(seed, catalog).await;
        logging::event(
            "INFO",
            "reminder_header_phrase",
            json!({"kind": kind.as_str(), "source": source.as_str(), "reason": reason}),
        );
        (phrase, source)
    }

    async fn attempt_phrase(
        &self,
        seed: &str,
        catalog: Option<&BossTable>,
    ) -> (String, HeadingSource, &'static str) {
        let fallback = || seed.to_owned();
        let Some(rewriter) = &self.rewriter else {
            return (fallback(), HeadingSource::Seed, "no_rewriter");
        };
        let Some(persona) = self.persona.as_ref().and_then(|source| source()) else {
            return (fallback(), HeadingSource::Seed, "no_persona");
        };
        let prompt = RewritePrompt::build(&persona, NudgeMood::Playful, seed);
        let call = rewriter.rewrite(&prompt, REWRITE_DEADLINE);
        match tokio::time::timeout(REWRITE_DEADLINE, call).await {
            Err(_) => (fallback(), HeadingSource::Seed, "timeout"),
            Ok(Err(failure)) => (fallback(), HeadingSource::Seed, failure_reason(failure)),
            Ok(Ok(text)) => match accept_phrase(&text, seed, catalog) {
                Ok(phrase) => (phrase, HeadingSource::Rewrite, "accepted"),
                Err(_) => (fallback(), HeadingSource::Seed, "rejected"),
            },
        }
    }

    async fn attempt(&self) -> (String, HeadingSource, &'static str) {
        let seed = || DAY_OF_HEADING_SEED.to_owned();
        let Some(rewriter) = &self.rewriter else {
            return (seed(), HeadingSource::Seed, "no_rewriter");
        };
        let Some(prompt) = self.prompt() else {
            return (seed(), HeadingSource::Seed, "no_persona");
        };
        let call = rewriter.rewrite(&prompt, REWRITE_DEADLINE);
        match tokio::time::timeout(REWRITE_DEADLINE, call).await {
            Err(_) => (seed(), HeadingSource::Seed, "timeout"),
            Ok(Err(failure)) => (seed(), HeadingSource::Seed, failure_reason(failure)),
            Ok(Ok(text)) => match accept_rewrite(&text, DAY_OF_HEADING_SEED) {
                Ok(line) => (line, HeadingSource::Rewrite, "accepted"),
                Err(_) => (seed(), HeadingSource::Seed, "rejected"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};

    fn catalog() -> BossTable {
        BossTable::from_spec(&CatalogSpec {
            difficulties: vec![DifficultySpec {
                prefix: "x".into(),
                label: "Extreme".into(),
            }],
            bosses: vec![BossSpec {
                short: "Kalos".into(),
                full: Some("Gatekeeper Kalos".into()),
                aliases: vec!["The Gatekeeper".into()],
                difficulties: Some(vec!["x".into()]),
                ..BossSpec::default()
            }],
        })
        .unwrap()
    }

    #[test]
    fn each_failure_logs_its_own_reason() {
        assert_eq!(failure_reason(RewriteFailure::Unavailable), "unavailable");
        assert_eq!(failure_reason(RewriteFailure::Refused), "refused");
        assert_eq!(
            failure_reason(RewriteFailure::Misconfigured),
            "misconfigured"
        );
    }

    #[test]
    fn phrase_gate_accepts_only_safe_interjections() {
        let catalog = catalog();
        for phrase in ["Waku waku!", "Let's go!", "Onward!"] {
            assert_eq!(
                accept_phrase(phrase, COUNTDOWN_PHRASE_SEED, Some(&catalog)),
                Ok(phrase.to_owned()),
                "{phrase}"
            );
        }
        for fact in [
            "20:14",
            "Thu 10 Sep",
            "Confirmed!",
            "yes!",
            "XKalos!",
            "Gatekeeper Kalos",
            "Two runs are ready",
            "https://example.test",
            "<@1001>",
            "**wow**",
        ] {
            assert!(
                accept_phrase(fact, COUNTDOWN_PHRASE_SEED, Some(&catalog)).is_err(),
                "{fact}"
            );
        }
    }
}
