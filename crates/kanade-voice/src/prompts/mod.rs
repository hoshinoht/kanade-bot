//! Code-owned chat policy text. Persona and profile files never override it;
//! the policy documents are byte-exact copies of the frozen v4 prompts.

mod context;
mod nudges;

pub use context::{clock_header, focus_line, runtime_line};
pub use nudges::builtin_nudges;

/// Assistant scope; `{assistant_name}` is replaced literally with the identity name.
pub const ASSISTANT_SCOPE: &str = include_str!("assistant-scope.md");
pub const SCHEDULER_POLICY: &str = include_str!("scheduler-policy.md");
pub const GROUNDING_POLICY: &str = include_str!("grounding-policy.md");
pub const BOSS_KNOWLEDGE_POLICY: &str = include_str!("boss-knowledge-policy.md");
pub const ASSISTANT_NAME_FIELD: &str = "{assistant_name}";

/// Voice cue when neither the profile nor the bundle declares one.
pub const DEFAULT_VOICE: &str = "Answer in the voice defined above. The schedule facts must be exact; everything around them is said in character.";

pub const STYLE_POLICY_QUALIFIER: &str = "This voice changes presentation only; trusted facts, operating policy, privacy and tool authority still control the answer.";

/// System-prompt voice cue.
pub const VOICE_PREFIX: &str = "Before you answer, remember your voice: ";

/// Final user-message voice cue; it identifies scheduler-authored text.
pub const REMINDER_PREFIX: &str = "[Note from the scheduler, not from anybody in the channel -- do not reply to this note; answer the conversation above it.] Write your reply in your own voice: ";

pub const REMINDER_SUFFIX: &str = " Every reply gets one small in-character touch -- card confirmations, error relays, and strategy/guide answers included. Facts, ids and times stay exact. Use compact Discord Markdown for factual blocks and separate distinct blocks with one blank line.";

pub const EXAMPLES_HEADING: &str = "Replies that sound right:";
