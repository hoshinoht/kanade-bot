//! The rewrite prompts: a code-owned instruction, the persona's rewrite text
//! (`compact.nudge_rewrite` for nudges, `compact.header_rewrite` for reminder
//! headers; else nothing beyond the voice cue), the member's profile voice,
//! the mood, and the seed line with its placeholders unfilled. They carry no
//! member, channel or schedule data. A reminder header may also carry a
//! [`HeaderFacts`] block right after the instruction: the party's estimate
//! totals per boss (bosses numbered, never named), the `{cN}` placeholders
//! offered for top carriers with their share and, with teasing on, one more
//! placeholder that may be teased lightly (share and the admins' running-joke
//! note); never a name, IGN, alias or id. Without it the header prompt is
//! exactly what it was before the block, and without a tease slot the block
//! is exactly what it was before teasing.

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

/// Prefixes the seed in the request: a bare short line ("Let's go!") reads
/// as a conversation turn, and a model then asks for the line instead of
/// rewriting it (2026-10-08: 3 of 5 replies to a bare "Let's go!").
pub const SEED_LABEL: &str = "Line to rewrite: ";

/// One boss on a header's card as the model sees it: party-level figures
/// only, numbered by its place on the card.
#[derive(Clone, Debug, PartialEq)]
pub struct BossFigures {
    /// The party's combined estimate in %, shown with one decimal.
    pub total: f64,
    /// Estimated clear minutes, only from a stored whole-party total.
    pub minutes: Option<f64>,
    /// Some figure assumes a 100% critical rate.
    pub crit_assumed: bool,
    /// The stored whole-party total's verdict, when there is one.
    pub possible: Option<bool>,
    /// Every party member has an estimate for this boss.
    pub complete: bool,
}

/// A placeholder the line may use for a top carrier, with their share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedSlot {
    /// `{c1}`, `{c2}`, …; code fills in the name after the gate.
    pub placeholder: String,
    /// Their whole-% share of the party's estimate on this card.
    pub share: u32,
}

/// The one member offered for light teasing (D11): opted in, in the party
/// with an estimate, the lowest share among those who opted in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeaseSlot {
    /// The next `{cN}` after the carriers'; code fills in the name.
    pub placeholder: String,
    /// Their whole-% share of the party's estimate on this card.
    pub share: u32,
    /// The admins' running-joke note, re-checked against the roster's
    /// names just before the prompt is built (D10).
    pub note: Option<String>,
}

/// The estimate context of a reminder header rewrite (D2/D3/D7/D11).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeaderFacts {
    pub bosses: Vec<BossFigures>,
    /// Empty in the gentle mood: no call-outs.
    pub slots: Vec<OfferedSlot>,
    /// Only with teasing on, never in the gentle mood; `None` leaves the
    /// block exactly as it is without teasing.
    pub tease: Option<TeaseSlot>,
}

impl HeaderFacts {
    /// The block placed after the header instruction.
    pub fn block(&self) -> String {
        let mut lines =
            vec!["Party estimate for this card, for tone only (never restate it):".to_owned()];
        for (index, boss) in self.bosses.iter().enumerate() {
            let mut parts = vec![format!("party total {:.1}%", boss.total)];
            if let Some(minutes) = boss.minutes {
                parts.push(format!("about {minutes:.1} minutes to clear"));
            }
            match boss.possible {
                Some(true) => parts.push("a clear is technically possible".to_owned()),
                Some(false) => parts.push("a clear is not technically possible yet".to_owned()),
                None => {}
            }
            if boss.crit_assumed {
                parts.push("critical rate assumed".to_owned());
            }
            if !boss.complete {
                parts.push("not complete: someone has no estimate".to_owned());
            }
            lines.push(format!("- boss {}: {}", index + 1, parts.join(", ")));
        }
        let offered: Vec<String> = self
            .slots
            .iter()
            .map(|slot| {
                format!(
                    "{} ({}% of the party's estimate)",
                    slot.placeholder, slot.share
                )
            })
            .collect();
        match &self.tease {
            None if self.slots.is_empty() => lines.push(
                "Speak to the whole party; do not single anyone out and add no braces.".to_owned(),
            ),
            None => {
                lines.push(format!(
                    "Top carriers you may praise, by placeholder only: {}. Use none, one or both, each at most once, copied exactly; they are the only braces you may add.",
                    offered.join(", ")
                ));
                lines.push(
                    "Positive call-outs only: praise an offered placeholder or the whole party; never point at the lowest or at anyone else."
                        .to_owned(),
                );
            }
            Some(tease) => tease_lines(&mut lines, &offered, tease),
        }
        lines.push("Never restate a figure, a percentage, a number or a boss.".to_owned());
        if self
            .bosses
            .iter()
            .any(|boss| boss.crit_assumed || !boss.complete)
        {
            lines.push(
                "Hedge: the estimate assumes a critical rate or misses someone, so sound hopeful, not certain."
                    .to_owned(),
            );
        }
        lines.join("\n")
    }
}

/// The call-out lines with a tease slot: the carriers to praise, the one
/// placeholder that may be teased (with its note), the two-slot cap and
/// positive call-outs for everyone else.
fn tease_lines(lines: &mut Vec<String>, offered: &[String], tease: &TeaseSlot) {
    let slot = &tease.placeholder;
    if !offered.is_empty() {
        lines.push(format!(
            "Top carriers you may praise, by placeholder only: {}. Use none, one or both, each at most once, copied exactly.",
            offered.join(", ")
        ));
    }
    let mut line = format!(
        "{slot} ({}% of the party's estimate) has asked to be teased: you may tease {slot} lightly and kindly, by placeholder only, at most once, copied exactly.",
        tease.share
    );
    if let Some(note) = &tease.note {
        line.push_str(&format!(" Their running joke, for flavour: \"{note}\"."));
    }
    lines.push(line);
    if offered.is_empty() {
        lines.push(format!(
            "{slot} is the only placeholder and the only brace you may add. Tease only {slot}; speak kindly to the rest of the party and single no one else out."
        ));
    } else {
        lines.push(format!(
            "Use at most two placeholders in all; they are the only braces you may add. Tease only {slot}: everyone else gets positive call-outs only, praise for a top carrier or the whole party; never point at anyone else."
        ));
    }
}

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
    /// phrase), guided by `compact.header_rewrite`, else `nudge_rewrite`;
    /// `facts` adds the estimate block after the instruction (`None`: the
    /// prompt is exactly the context-free one).
    pub fn header(
        persona: &CompiledPersona,
        mood: NudgeMood,
        seed: &str,
        facts: Option<&HeaderFacts>,
    ) -> Self {
        let instruction = match facts {
            None => HEADER_REWRITE_INSTRUCTION.to_owned(),
            Some(facts) => format!("{HEADER_REWRITE_INSTRUCTION}\n\n{}", facts.block()),
        };
        Self::compose(
            &instruction,
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

    /// The line itself, placeholders unfilled.
    pub fn seed(&self) -> &str {
        &self.seed
    }

    /// The system prompt, then the seed under [`SEED_LABEL`].
    pub fn messages(&self) -> Vec<Message> {
        vec![
            Message::System {
                content: self.system.clone(),
            },
            Message::User {
                content: format!("{SEED_LABEL}{}", self.seed),
            },
        ]
    }

    /// [`Self::messages`] as one text for the Rewrites log: each message
    /// under its role label (`[system]`, `[user]`), separated by a blank line.
    pub fn transcript(&self) -> String {
        self.messages()
            .into_iter()
            .filter_map(|message| match message {
                Message::System { content } => Some(format!("[system]\n{content}")),
                Message::User { content } => Some(format!("[user]\n{content}")),
                Message::Assistant { .. } | Message::Tool { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::persona::{PersonaId, parse_bundle};

    /// The model is told which text is the line, so a short seed that reads
    /// like a chat turn is not answered as one; the seed itself stays bare.
    #[test]
    fn the_request_labels_the_line_to_rewrite() {
        let bundle = parse_bundle(
            include_str!("../../../config/personas/bundles/kanade.yaml"),
            &PersonaId::parse("kanade").unwrap(),
        )
        .unwrap();
        let persona = CompiledPersona::compile(&bundle, None);
        for prompt in [
            RewritePrompt::header(&persona, NudgeMood::Playful, "Let's go!", None),
            RewritePrompt::build(&persona, NudgeMood::Gentle, "Fix {boss} here."),
        ] {
            let messages = prompt.messages();
            let Some(Message::User { content }) = messages.last() else {
                panic!("the seed is the last, user message: {messages:?}");
            };
            assert_eq!(content, &format!("Line to rewrite: {}", prompt.seed()));
            assert!(!prompt.seed().starts_with(SEED_LABEL));
        }
    }

    /// The logged prompt is exactly the messages sent, under role labels.
    #[test]
    fn the_transcript_is_the_sent_messages_under_role_labels() {
        let bundle = parse_bundle(
            include_str!("../../../config/personas/bundles/kanade.yaml"),
            &PersonaId::parse("kanade").unwrap(),
        )
        .unwrap();
        let persona = CompiledPersona::compile(&bundle, None);
        let prompt = RewritePrompt::header(&persona, NudgeMood::Playful, "Let's go!", None);
        assert_eq!(
            prompt.transcript(),
            format!(
                "[system]\n{}\n\n[user]\nLine to rewrite: Let's go!",
                prompt.system()
            )
        );
    }

    fn kanade() -> CompiledPersona {
        let bundle = parse_bundle(
            include_str!("../../../config/personas/bundles/kanade.yaml"),
            &PersonaId::parse("kanade").unwrap(),
        )
        .unwrap();
        CompiledPersona::compile(&bundle, None)
    }

    /// Without estimate context the header prompt is byte for byte the one
    /// sent before the context block existed (1.0.0-beta.8): the
    /// instruction, the persona's header text, the voice cue and the mood.
    #[test]
    fn a_header_prompt_without_context_is_unchanged() {
        const BETA8_INSTRUCTION: &str = "Rewrite the one reminder header line you are given so it sounds like the character described below. Keep it short (at most eight words besides any {day}), friendly and safe for work, whatever the character's style. Keep its meaning and its mood. Keep {day} exactly as written when the line has it and add no other braces. Add no dates, times, numbers, boss names or attendance news of your own. Reply with that one plain-text line only: no quotes, links, URLs, mentions, line breaks or markdown. Never use asterisks, underscores, backticks or other formatting, even if the character notes below ask for it.";
        const BETA8_PLAYFUL: &str = "Mood: playful. Light teasing is fine.";
        let persona = kanade();
        let mut parts = vec![BETA8_INSTRUCTION.to_owned()];
        if let Some(text) = persona.prompt_compact().or_else(|| persona.nudge_rewrite()) {
            parts.push(text.trim().to_owned());
        }
        if persona.provenance().voice != VoiceSource::Default {
            parts.push(format!("Voice: {}", persona.effective_voice()));
        }
        parts.push(BETA8_PLAYFUL.to_owned());
        let golden = format!(
            "[system]\n{}\n\n[user]\nLine to rewrite: Today — {{day}}",
            parts.join("\n\n")
        );
        let prompt = RewritePrompt::header(&persona, NudgeMood::Playful, "Today — {day}", None);
        assert_eq!(prompt.transcript(), golden);
    }

    fn facts(slots: &[(&str, u32)], crit: bool, complete: bool) -> HeaderFacts {
        HeaderFacts {
            bosses: vec![
                BossFigures {
                    total: 112.44,
                    minutes: Some(26.7),
                    crit_assumed: crit,
                    possible: Some(true),
                    complete,
                },
                BossFigures {
                    total: 61.0,
                    minutes: None,
                    crit_assumed: false,
                    possible: None,
                    complete: true,
                },
            ],
            slots: slots
                .iter()
                .map(|(placeholder, share)| OfferedSlot {
                    placeholder: (*placeholder).to_owned(),
                    share: *share,
                })
                .collect(),
            tease: None,
        }
    }

    /// The block follows the instruction, numbers the bosses, offers the
    /// placeholders with their share and hedges only when asked to.
    #[test]
    fn the_estimate_block_follows_the_instruction() {
        let persona = kanade();
        let plain = RewritePrompt::header(&persona, NudgeMood::Playful, "Onward!", None);
        let with = facts(&[("{c1}", 48), ("{c2}", 31)], false, true);
        let prompt = RewritePrompt::header(&persona, NudgeMood::Playful, "Onward!", Some(&with));
        let (instruction, rest) = plain.system().split_once("\n\n").unwrap();
        assert_eq!(
            prompt.system(),
            format!("{instruction}\n\n{}\n\n{rest}", with.block())
        );
        let block = with.block();
        assert!(block.contains(
            "- boss 1: party total 112.4%, about 26.7 minutes to clear, a clear is technically possible"
        ));
        assert!(block.contains("- boss 2: party total 61.0%\n"));
        assert!(block.contains("{c1} (48% of the party's estimate), {c2} (31%"));
        assert!(!block.contains("Hedge"));
        let hedged = facts(&[], true, false).block();
        assert!(hedged.contains("critical rate assumed, not complete"));
        assert!(hedged.contains("Hedge:"));
        assert!(hedged.contains("do not single anyone out"));
        assert!(!hedged.contains('{'));
    }

    /// Without a tease slot the call-out lines are exactly header-context's
    /// (beta.9 before teasing), with carriers or without.
    #[test]
    fn without_a_tease_slot_the_block_is_unchanged() {
        const BOSSES: &str = "Party estimate for this card, for tone only (never restate it):\n- boss 1: party total 112.4%, about 26.7 minutes to clear, a clear is technically possible\n- boss 2: party total 61.0%\n";
        const TAIL: &str = "\nNever restate a figure, a percentage, a number or a boss.";
        assert_eq!(
            facts(&[("{c1}", 48), ("{c2}", 31)], false, true).block(),
            format!(
                "{BOSSES}Top carriers you may praise, by placeholder only: {{c1}} (48% of the party's estimate), {{c2}} (31% of the party's estimate). Use none, one or both, each at most once, copied exactly; they are the only braces you may add.\nPositive call-outs only: praise an offered placeholder or the whole party; never point at the lowest or at anyone else.{TAIL}"
            )
        );
        assert_eq!(
            facts(&[], false, true).block(),
            format!(
                "{BOSSES}Speak to the whole party; do not single anyone out and add no braces.{TAIL}"
            )
        );
    }

    /// A tease slot is offered after the carriers with its share and note,
    /// teasing is allowed for that placeholder only, and the cap of two
    /// placeholders is stated.
    #[test]
    fn a_tease_slot_is_the_only_one_that_may_be_teased() {
        let tease = |note: Option<&str>| {
            Some(TeaseSlot {
                placeholder: "{c3}".into(),
                share: 9,
                note: note.map(str::to_owned),
            })
        };
        let with = HeaderFacts {
            tease: tease(Some("Always lost on the way to the portal")),
            ..facts(&[("{c1}", 48), ("{c2}", 31)], false, true)
        };
        let block = with.block();
        assert!(block.contains("{c1} (48% of the party's estimate), {c2} (31%"));
        assert!(block.contains(
            "{c3} (9% of the party's estimate) has asked to be teased: you may tease {c3} lightly"
        ));
        assert!(
            block.contains("running joke, for flavour: \"Always lost on the way to the portal\"")
        );
        assert!(block.contains("Use at most two placeholders in all"));
        assert!(block.contains("Tease only {c3}"));
        assert!(!block.contains("never point at the lowest"));
        // No carriers (an unsafe IGN): the tease slot is the only brace.
        let alone = HeaderFacts {
            tease: tease(None),
            ..facts(&[], false, true)
        }
        .block();
        assert!(alone.contains("{c3} is the only placeholder"));
        assert!(!alone.contains("running joke") && !alone.contains("Top carriers"));
        assert!(!alone.contains("add no braces"));
    }
}
