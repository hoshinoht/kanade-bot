//! Compile a resolved bundle/profile pair into the chat presentation document.
//! Compilation is pure: no I/O and no model calls.

use std::fmt;

use super::{
    NudgeMood, NudgePurpose, NudgeSource, Nudges,
    id::{PersonaId, ProfileId},
    loader::Source,
    markdown::{good_examples, identity_name, is_placeholder, strip},
    resolver::ProfileSource,
    schema::{Bundle, Profile, Staging},
    snapshot::ResolvedPersona,
    staging::stripped,
};
use crate::chat::prompts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceSource {
    Profile,
    Bundle,
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExampleSource {
    Profile,
    Bundle,
    None,
}

/// Operator diagnostics for one compilation; never model-visible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileProvenance {
    pub bundle: PersonaId,
    pub profile: Option<ProfileId>,
    /// Set when compiled from a per-turn resolution.
    pub profile_source: Option<ProfileSource>,
    pub bundle_file: Option<Source>,
    pub profile_file: Option<Source>,
    pub voice: VoiceSource,
    pub examples: ExampleSource,
}

/// Seed lines for one nudge, most specific pool first found.
#[derive(Clone, PartialEq, Eq)]
pub struct NudgeSeeds<'a> {
    pub lines: Vec<&'a str>,
    pub source: NudgeSource,
}

/// Immutable presentation document for one member's turn.
#[derive(Clone, PartialEq, Eq)]
pub struct CompiledPersona {
    identity: String,
    behaviour: String,
    profile_prompt: Option<String>,
    profile_voice: Option<String>,
    effective_voice: String,
    examples: Vec<String>,
    staging_lines: Staging,
    prompt: String,
    prompt_compact: Option<String>,
    nudge_rewrite: Option<String>,
    profile_nudges: Option<Nudges>,
    bundle_nudges: Option<Nudges>,
    provenance: CompileProvenance,
}

/// A declared voice, unless it is blank or an unfilled `<...>` template slot.
fn usable_voice(voice: Option<&String>) -> Option<&str> {
    voice
        .map(|voice| strip(voice))
        .filter(|voice| !voice.is_empty() && !is_placeholder(voice))
}

fn owned_text(text: &str) -> Option<String> {
    let text = strip(text);
    (!text.is_empty()).then(|| text.to_owned())
}

fn examples_block(examples: &[String]) -> String {
    if examples.is_empty() {
        return String::new();
    }
    let mut block = prompts::EXAMPLES_HEADING.to_owned();
    for example in examples {
        block.push_str("\n- ");
        block.push_str(example);
    }
    block
}

/// Persona presentation first, then the code-owned policies, in v4 order.
fn presentation(identity: &str, behaviour: &str, profile: Option<&str>, examples: &str) -> String {
    let scope = strip(prompts::ASSISTANT_SCOPE)
        .replace(prompts::ASSISTANT_NAME_FIELD, identity_name(identity));
    let parts = [
        identity,
        behaviour,
        profile.unwrap_or(""),
        examples,
        &scope,
        strip(prompts::SCHEDULER_POLICY),
        strip(prompts::GROUNDING_POLICY),
        strip(prompts::BOSS_KNOWLEDGE_POLICY),
    ];
    join_parts(&parts)
}

/// v4 `"\n\n".join(part for part in parts if part)`.
pub(crate) fn join_parts(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl CompiledPersona {
    pub fn compile(bundle: &Bundle, profile: Option<&Profile>) -> Self {
        let identity = strip(&bundle.identity).to_owned();
        let behaviour = strip(&bundle.prompt).to_owned();
        let profile_prompt = profile.and_then(|profile| owned_text(&profile.prompt));
        let profile_voice =
            profile.and_then(|profile| usable_voice(profile.voice.as_ref()).map(str::to_owned));
        let (effective_voice, voice) = match (&profile_voice, usable_voice(bundle.voice.as_ref())) {
            (Some(voice), _) => (voice.clone(), VoiceSource::Profile),
            (None, Some(voice)) => (voice.to_owned(), VoiceSource::Bundle),
            (None, None) => (prompts::DEFAULT_VOICE.to_owned(), VoiceSource::Default),
        };
        let profile_examples = profile_prompt
            .as_deref()
            .map(good_examples)
            .unwrap_or_default();
        let (examples, example_source) = if !profile_examples.is_empty() {
            (profile_examples, ExampleSource::Profile)
        } else {
            let examples = good_examples(&behaviour);
            let source = if examples.is_empty() {
                ExampleSource::None
            } else {
                ExampleSource::Bundle
            };
            (examples, source)
        };
        let staging = profile.map_or_else(
            || bundle.staging.clone(),
            |profile| profile.staging.apply(&bundle.staging),
        );
        let prompt = presentation(
            &identity,
            &behaviour,
            profile_prompt.as_deref(),
            &examples_block(&examples),
        );
        let compact = bundle.compact.as_ref();
        Self {
            prompt,
            prompt_compact: compact.and_then(|compact| compact.header_rewrite.clone()),
            nudge_rewrite: compact.and_then(|compact| compact.nudge_rewrite.clone()),
            staging_lines: stripped(&staging),
            identity,
            behaviour,
            profile_prompt,
            profile_voice,
            effective_voice,
            examples,
            profile_nudges: profile.and_then(|profile| profile.nudges.clone()),
            bundle_nudges: bundle.nudges.clone(),
            provenance: CompileProvenance {
                bundle: bundle.id.clone(),
                profile: profile.map(|profile| profile.id.clone()),
                profile_source: None,
                bundle_file: None,
                profile_file: None,
                voice,
                examples: example_source,
            },
        }
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn behaviour(&self) -> &str {
        &self.behaviour
    }

    pub fn profile_prompt(&self) -> Option<&str> {
        self.profile_prompt.as_deref()
    }

    /// The profile's own usable voice cue, for small rewrite prompts.
    pub fn profile_voice(&self) -> Option<&str> {
        self.profile_voice.as_deref()
    }

    /// Profile voice, then bundle voice, then the code-owned default.
    pub fn effective_voice(&self) -> &str {
        &self.effective_voice
    }

    /// Profile `Good` examples when present, otherwise the behaviour's.
    pub fn examples(&self) -> &[String] {
        &self.examples
    }

    /// Complete, stripped staging lines after profile inheritance.
    pub fn staging_lines(&self) -> &Staging {
        &self.staging_lines
    }

    /// Static full-chat presentation: persona text, examples and code-owned policies.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// Header-line rewrite prompt only; not a chat prompt or the final reminder.
    pub fn prompt_compact(&self) -> Option<&str> {
        self.prompt_compact.as_deref()
    }

    pub fn nudge_rewrite(&self) -> Option<&str> {
        self.nudge_rewrite.as_deref()
    }

    /// Profile pools, then bundle pools, then neutral built-ins.
    pub fn nudge_seeds(&self, purpose: NudgePurpose, mood: NudgeMood) -> NudgeSeeds<'_> {
        let declared = [
            (&self.profile_nudges, NudgeSource::Profile),
            (&self.bundle_nudges, NudgeSource::Bundle),
        ];
        for (nudges, source) in declared {
            if let Some(pool) = nudges.as_ref().and_then(|n| n.pool(purpose, mood)) {
                return NudgeSeeds {
                    lines: pool.iter().map(String::as_str).collect(),
                    source,
                };
            }
        }
        NudgeSeeds {
            lines: prompts::builtin_nudges(purpose, mood).to_vec(),
            source: NudgeSource::BuiltIn,
        }
    }

    pub fn provenance(&self) -> &CompileProvenance {
        &self.provenance
    }
}

impl ResolvedPersona<'_> {
    /// Compile this turn's persona, recording which files and selection supplied it.
    pub fn compile(&self) -> CompiledPersona {
        let mut compiled = CompiledPersona::compile(self.bundle, self.profile);
        compiled.provenance.profile_source = Some(self.profile_source);
        compiled.provenance.bundle_file = Some(self.bundle_source.clone());
        compiled.provenance.profile_file = self.profile_file.cloned();
        compiled
    }
}

/// Persona text is private; only sizes and provenance are printed.
impl fmt::Debug for CompiledPersona {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledPersona")
            .field("prompt_bytes", &self.prompt.len())
            .field("examples", &self.examples.len())
            .field("has_compact", &self.prompt_compact.is_some())
            .field("provenance", &self.provenance)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for NudgeSeeds<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NudgeSeeds")
            .field("lines", &self.lines.len())
            .field("source", &self.source)
            .finish()
    }
}
