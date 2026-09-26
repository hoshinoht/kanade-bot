//! The day-of heading in the persona's voice (user decision 2026-09-26, a
//! named difference from v4). The seed is v4's `Today — {day}`; the small
//! `rewrite` model may reword it through the nudge rewriter (lowest priority,
//! never waits, [`REWRITE_DEADLINE`], output checked by [`accept_rewrite`]),
//! and `{day}` is filled afterwards. The prompt holds the persona and the
//! seed only: no member, boss or schedule data. Any failure uses the seed.

use std::sync::Arc;

use serde_json::json;

use crate::chat::nudge::{
    NudgeRewriter, REWRITE_DEADLINE, RewritePrompt, SharedRewriter, accept_rewrite,
};
use crate::chat::persona::{CompiledPersona, NudgeMood};
use crate::runtime::logging;

/// v4's heading, with the day left for after the rewrite.
pub const DAY_OF_HEADING_SEED: &str = "Today — {day}";
const DAY: &str = "{day}";

/// The guild default persona (bundle, no member profile), if chat has one.
pub type PersonaSource = Arc<dyn Fn() -> Option<CompiledPersona> + Send + Sync>;

/// Where a heading came from, for the `day_of_heading` log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadingSource {
    Rewrite,
    Seed,
}

impl HeadingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewrite => "rewrite",
            Self::Seed => "seed",
        }
    }
}

/// v4's heading for `day`.
pub fn seed_heading(day: &str) -> String {
    DAY_OF_HEADING_SEED.replace(DAY, day)
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
            Ok(Err(_)) => (seed(), HeadingSource::Seed, "unavailable"),
            Ok(Ok(text)) => match accept_rewrite(&text, DAY_OF_HEADING_SEED) {
                Ok(line) => (line, HeadingSource::Rewrite, "accepted"),
                Err(_) => (seed(), HeadingSource::Seed, "rejected"),
            },
        }
    }
}
