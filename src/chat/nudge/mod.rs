//! Persona-voiced self-service nudges. A seed line is rotated per channel from
//! the resolved persona's pools, optionally rewritten by the small `rewrite`
//! model (lowest priority, never waits, 2 s, output checked, seed on any
//! failure), then filled and followed by the code-owned action and link. At
//! most one lead-in per member per boss week.

mod compose;
mod governed;
mod prompt;
mod rewrite;
mod rotation;
mod safety;

pub use compose::{
    EDIT_RUN_ACTION, LineSource, Nudge, NudgeFacts, Nudger, REQUEST_CHANGE_ACTION, SeedReason,
    action, mood_for, render,
};
pub use governed::{DynRewrite, GovernedRewriter, REWRITE_MAX_OUTPUT_TOKENS, SharedRewriter};
pub use prompt::{
    GENTLE_MOOD, NUDGE_REWRITE_INSTRUCTION, PLAYFUL_MOOD, RewritePrompt, VOICE_LABEL,
};
pub use rewrite::{
    NoRewrite, NudgeRewriter, REWRITE_DEADLINE, Rejection, RewriteFailure, accept_rewrite,
};
pub use rotation::{MAX_CHANNELS, RECENT_PER_CHANNEL, SeedRotation};
pub use safety::{
    DENY_INSIDE, DENY_LIST, DENY_SEA, DENY_SOUNDALIKE, denied_word, has_format_char, has_invite,
    has_markup,
};
