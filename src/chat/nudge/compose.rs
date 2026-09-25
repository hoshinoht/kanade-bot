//! One nudge: claim the member's weekly tip, pick a seed, try the rewrite,
//! fill `{boss}`/`{day}`/`{time}`, and append the code-owned action and link.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{
    prompt::RewritePrompt,
    rewrite::{NudgeRewriter, REWRITE_DEADLINE, accept_rewrite},
    rotation::SeedRotation,
};
use crate::chat::persona::{CompiledPersona, NudgeMood, NudgePurpose, NudgeSource, fill_nudge};
use crate::chat::prompts::builtin_nudges;
use crate::domain::model_log::ModelLogStore;
use crate::domain::scheduler::StoreError;
use crate::infrastructure::llm::governor::Random;

pub const EDIT_RUN_ACTION: &str = "→ edit the run: ";
pub const REQUEST_CHANGE_ACTION: &str = "→ request a change: ";

/// Failures and frustration are always gentle, whatever the profile.
pub fn mood_for(failed: bool, frustrated: bool) -> NudgeMood {
    if failed || frustrated {
        NudgeMood::Gentle
    } else {
        NudgeMood::Playful
    }
}

pub fn action(purpose: NudgePurpose) -> &'static str {
    match purpose {
        NudgePurpose::SelfService => EDIT_RUN_ACTION,
        NudgePurpose::RequestForm => REQUEST_CHANGE_ACTION,
    }
}

/// `<lead-in> → edit the run: <link>`; without a lead-in (tip already given
/// this week) only the action and link.
pub fn render(lead_in: Option<&str>, purpose: NudgePurpose, link: &str) -> String {
    let action = action(purpose);
    match lead_in {
        Some(lead_in) => format!("{lead_in} {action}{link}"),
        None => format!("{action}{link}"),
    }
}

/// What the lead-in is about; substituted literally after any rewrite.
#[derive(Clone, Copy, Debug)]
pub struct NudgeFacts<'a> {
    pub channel_id: &'a str,
    pub purpose: NudgePurpose,
    pub mood: NudgeMood,
    pub boss: &'a str,
    pub day: &'a str,
    pub time: &'a str,
}

/// Why the seed line was used as-is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedReason {
    Unavailable,
    TimedOut,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineSource {
    Rewritten,
    Seed(SeedReason),
}

/// A filled lead-in; provenance is for logs only, never shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nudge {
    pub lead_in: String,
    pub line: LineSource,
    pub seeds: NudgeSource,
}

pub struct Nudger<R> {
    rotation: SeedRotation,
    rewriter: R,
}

impl<R> std::fmt::Debug for Nudger<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nudger").finish_non_exhaustive()
    }
}

impl<R: NudgeRewriter> Nudger<R> {
    pub fn new(random: Arc<dyn Random>, rewriter: R) -> Self {
        Self {
            rotation: SeedRotation::new(random),
            rewriter,
        }
    }

    /// The lead-in only if `member_id` has no tip yet in the boss week starting
    /// `week` (claimed before any model call, so load follows the tip limit).
    pub async fn tip<S: ModelLogStore>(
        &self,
        store: &S,
        member_id: &str,
        week: DateTime<Utc>,
        now: DateTime<Utc>,
        persona: &CompiledPersona,
        facts: &NudgeFacts<'_>,
    ) -> Result<Option<Nudge>, StoreError> {
        if !store.claim_tip(member_id, week, now).await? {
            return Ok(None);
        }
        Ok(Some(self.lead_in(persona, facts).await))
    }

    /// A rotated seed for the resolved persona (profile → bundle → built-in
    /// pools), rewritten when the model answers validly within the deadline.
    pub async fn lead_in(&self, persona: &CompiledPersona, facts: &NudgeFacts<'_>) -> Nudge {
        let seeds = persona.nudge_seeds(facts.purpose, facts.mood);
        let builtin = builtin_nudges(facts.purpose, facts.mood);
        let seed = self
            .rotation
            .pick(facts.channel_id, &seeds.lines)
            .unwrap_or(builtin[0]);
        let prompt = RewritePrompt::build(persona, facts.mood, seed);
        let attempt = tokio::time::timeout(
            REWRITE_DEADLINE,
            self.rewriter.rewrite(&prompt, REWRITE_DEADLINE),
        )
        .await;
        let (template, line) = match attempt {
            Err(_) => (seed.to_owned(), LineSource::Seed(SeedReason::TimedOut)),
            Ok(Err(_)) => (seed.to_owned(), LineSource::Seed(SeedReason::Unavailable)),
            Ok(Ok(output)) => match accept_rewrite(&output, seed) {
                Some(line) => (line, LineSource::Rewritten),
                None => (seed.to_owned(), LineSource::Seed(SeedReason::Rejected)),
            },
        };
        Nudge {
            lead_in: fill_nudge(&template, facts.boss, facts.day, facts.time),
            line,
            seeds: seeds.source,
        }
    }
}
