//! One nudge: claim the member's weekly tip, pick a seed, try the rewrite,
//! fill `{boss}`/`{day}`/`{time}`, and append the code-owned action and link.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{
    prompt::RewritePrompt,
    rewrite::{NudgeRewriter, REWRITE_DEADLINE, Rejection, RewriteFailure, accept_rewrite},
    rotation::SeedRotation,
    safety,
};
use crate::chat::persona::{
    CompiledPersona, NudgeMood, NudgePurpose, NudgeSource, check_nudge_line, fill_nudge,
};
use crate::chat::prompts::builtin_nudges;
use crate::domain::model_log::ModelLogStore;
use crate::domain::notify::WeekReset;
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

/// Why the seed line was used; the caller logs it (it never holds model text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedReason {
    Unavailable,
    /// The provider's content filter or a refusal.
    Refused,
    /// An operator setting prevents rewrites; worth flagging.
    Misconfigured,
    TimedOut,
    Rejected(Rejection),
    /// The rewrite was fine but became unsafe once `{boss}`/`{day}`/`{time}` were filled.
    UnsafeFill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineSource {
    Rewritten,
    Seed(SeedReason),
    /// The filled seed was unsafe too: a placeholder-free built-in line.
    FieldFree,
}

impl LineSource {
    /// Stable log label; never includes model text (a deny-list hit names only the class).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewritten => "rewritten",
            Self::FieldFree => "field_free",
            Self::Seed(reason) => match reason {
                SeedReason::Unavailable => "seed_unavailable",
                SeedReason::Refused => "seed_refused",
                SeedReason::Misconfigured => "seed_misconfigured",
                SeedReason::TimedOut => "seed_timed_out",
                SeedReason::UnsafeFill => "seed_unsafe_fill",
                SeedReason::Rejected(Rejection::LineRules) => "seed_rejected_line_rules",
                SeedReason::Rejected(Rejection::Placeholders) => "seed_rejected_placeholders",
                SeedReason::Rejected(Rejection::Markup) => "seed_rejected_markup",
                SeedReason::Rejected(Rejection::Denied(_)) => "seed_rejected_denied",
            },
        }
    }
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

    /// The lead-in only if `member_id` has no tip yet in the boss week holding
    /// `now` under `reset` (claimed before any model call, so load follows the
    /// tip limit). A clock outside the supported years gives no tip.
    pub async fn tip<S: ModelLogStore>(
        &self,
        store: &S,
        member_id: &str,
        reset: &WeekReset,
        now: DateTime<Utc>,
        persona: &CompiledPersona,
        facts: &NudgeFacts<'_>,
    ) -> Result<Option<Nudge>, StoreError> {
        let Ok(week) = reset.current_week(now) else {
            return Ok(None);
        };
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
            Ok(Err(RewriteFailure::Unavailable)) => {
                (seed.to_owned(), LineSource::Seed(SeedReason::Unavailable))
            }
            Ok(Err(RewriteFailure::Refused)) => {
                (seed.to_owned(), LineSource::Seed(SeedReason::Refused))
            }
            Ok(Err(RewriteFailure::Misconfigured)) => {
                (seed.to_owned(), LineSource::Seed(SeedReason::Misconfigured))
            }
            Ok(Ok(output)) => match accept_rewrite(&output, seed) {
                Ok(line) => (line, LineSource::Rewritten),
                Err(rejection) => (
                    seed.to_owned(),
                    LineSource::Seed(SeedReason::Rejected(rejection)),
                ),
            },
        };
        let fill = |template: &str| {
            let filled = fill_nudge(template, facts.boss, facts.day, facts.time);
            safe_fill(template, &filled, facts).then_some(filled)
        };
        let (lead_in, line) = match fill(&template) {
            Some(filled) => (filled, line),
            None => match (line, fill(seed)) {
                (LineSource::Rewritten, Some(filled)) => {
                    (filled, LineSource::Seed(SeedReason::UnsafeFill))
                }
                _ => {
                    let plain = builtin
                        .iter()
                        .find(|line| !line.contains('{'))
                        .unwrap_or(&builtin[0]);
                    ((*plain).to_owned(), LineSource::FieldFree)
                }
            },
        };
        Nudge {
            lead_in,
            line,
            seeds: seeds.source,
        }
    }
}

/// Like staging's `guide_named_for`: values carrying mention, link, markup or
/// format characters are unsafe, and the filled line must still pass the seed
/// rules (catching mentions assembled across a placeholder boundary). Values
/// only matter when the template uses them.
fn safe_fill(template: &str, filled: &str, facts: &NudgeFacts<'_>) -> bool {
    let values = [
        ("{boss}", facts.boss),
        ("{day}", facts.day),
        ("{time}", facts.time),
    ];
    let values_safe = values
        .iter()
        .filter(|(field, _)| template.contains(field))
        .all(|(_, value)| safe_value(value));
    values_safe && check_nudge_line(filled).is_ok() && !safety::has_format_char(filled)
}

fn safe_value(value: &str) -> bool {
    let lower = value.to_lowercase();
    !value.trim().is_empty()
        && !value
            .chars()
            .any(|c| c.is_control() || "@<>{}[]#".contains(c))
        && !safety::has_markup(value)
        && !safety::has_format_char(value)
        && !safety::has_invite(value)
        && !lower.contains("://")
        && !lower.contains("www.")
}
