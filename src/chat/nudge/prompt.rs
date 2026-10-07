//! The rewrite prompts: a code-owned instruction, the persona's rewrite text
//! (`compact.nudge_rewrite` for nudges, `compact.header_rewrite` for reminder
//! headers; else nothing beyond the voice cue), the member's profile voice,
//! the mood, and the seed line with its placeholders unfilled. They carry no
//! member, channel, boss or schedule data.

use crate::chat::persona::{CompiledPersona, NudgeMood, VoiceSource};
use crate::infrastructure::llm::Message;

/// Code-owned; persona files cannot loosen it (SFW and brevity hold for every profile).
pub const NUDGE_REWRITE_INSTRUCTION: &str = "Rewrite the one line you are given so it sounds like the character described below. Keep it short, friendly and safe for work, whatever the character's style. Keep its meaning and its mood. Keep every {boss}, {day} and {time} exactly as written and add no other braces. Reply with that one line only, at most 140 characters: no quotes, links, URLs, mentions or markdown.";

/// Code-owned, and placed first so it outranks the persona's header text
/// (which may ask for markdown dates the header gate refuses).
const HEADER_REWRITE_INSTRUCTION: &str = "Rewrite the one reminder header line you are given so it sounds like the character described below. Keep it short (at most eight words besides any {day}), friendly and safe for work, whatever the character's style. Keep its meaning and its mood. Keep {day} exactly as written when the line has it and add no other braces. Add no dates, times, numbers, boss names or attendance news of your own. Reply with that one plain-text line only: no quotes, links, URLs, mentions, line breaks or markdown. Never use asterisks, underscores, backticks or other formatting, even if the character notes below ask for it.";

pub const PLAYFUL_MOOD: &str = "Mood: playful. Light teasing is fine.";
/// Mood beats the profile: teasing profiles stay kind here.
pub const GENTLE_MOOD: &str = "Mood: gentle. Something went wrong or the member is frustrated: be kind and reassuring, never teasing, whatever the voice says.";

pub const VOICE_LABEL: &str = "Voice: ";

#[derive(Clone, PartialEq, Eq)]
pub struct RewritePrompt {
    system: String,
    seed: String,
}

impl std::fmt::Debug for RewritePrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RewritePrompt")
            .field("system_bytes", &self.system.len())
            .field("seed_bytes", &self.seed.len())
            .finish()
    }
}

impl RewritePrompt {
    /// A self-service nudge rewrite. `seed` is the unfilled template; values
    /// are substituted after the rewrite.
    pub fn build(persona: &CompiledPersona, mood: NudgeMood, seed: &str) -> Self {
        Self::compose(
            NUDGE_REWRITE_INSTRUCTION,
            persona.nudge_rewrite(),
            persona,
            mood,
            seed,
        )
    }

    /// A reminder header rewrite (day-of heading, countdown or digest
    /// phrase), guided by `compact.header_rewrite`, else `nudge_rewrite`.
    pub fn header(persona: &CompiledPersona, mood: NudgeMood, seed: &str) -> Self {
        Self::compose(
            HEADER_REWRITE_INSTRUCTION,
            persona.prompt_compact().or_else(|| persona.nudge_rewrite()),
            persona,
            mood,
            seed,
        )
    }

    fn compose(
        instruction: &str,
        character: Option<&str>,
        persona: &CompiledPersona,
        mood: NudgeMood,
        seed: &str,
    ) -> Self {
        let mut parts = vec![instruction.to_owned()];
        if let Some(character) = character {
            parts.push(character.trim().to_owned());
        }
        // The chat default voice cue is about chat replies, not a character.
        if persona.provenance().voice != VoiceSource::Default {
            parts.push(format!("{VOICE_LABEL}{}", persona.effective_voice()));
        }
        parts.push(
            match mood {
                NudgeMood::Playful => PLAYFUL_MOOD,
                NudgeMood::Gentle => GENTLE_MOOD,
            }
            .to_owned(),
        );
        Self {
            system: parts.join("\n\n"),
            seed: seed.to_owned(),
        }
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    pub fn seed(&self) -> &str {
        &self.seed
    }

    pub fn messages(&self) -> Vec<Message> {
        vec![
            Message::System {
                content: self.system.clone(),
            },
            Message::User {
                content: self.seed.clone(),
            },
        ]
    }
}
