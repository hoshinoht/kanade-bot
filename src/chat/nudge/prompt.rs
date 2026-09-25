//! The rewrite prompt: a code-owned instruction, the persona's
//! `compact.nudge_rewrite` (else nothing beyond the voice cue), the member's
//! profile voice, the mood, and the seed line with its placeholders unfilled.
//! It carries no member, channel, boss or schedule data.

use crate::chat::persona::{CompiledPersona, NudgeMood, VoiceSource};
use crate::infrastructure::llm::Message;

/// Code-owned; persona files cannot loosen it (SFW and brevity hold for every profile).
pub const NUDGE_REWRITE_INSTRUCTION: &str = "Rewrite the one line you are given so it sounds like the character described below. Keep it short, friendly and safe for work, whatever the character's style. Keep its meaning and its mood. Keep every {boss}, {day} and {time} exactly as written and add no other braces. Reply with that one line only, at most 140 characters: no quotes, links, URLs, mentions or markdown.";

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
    /// `seed` is the unfilled template; values are substituted after the rewrite.
    pub fn build(persona: &CompiledPersona, mood: NudgeMood, seed: &str) -> Self {
        let mut parts = vec![NUDGE_REWRITE_INSTRUCTION.to_owned()];
        if let Some(character) = persona.nudge_rewrite() {
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
