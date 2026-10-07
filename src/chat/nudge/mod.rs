//! Persona-voiced self-service nudges. A seed line is rotated per channel from
//! the resolved persona's pools, optionally rewritten by the small `rewrite`
//! model (lowest priority, never waits, 2 s, output checked, seed on any
//! failure), then filled and followed by the code-owned action and link. At
//! most one lead-in per member per boss week.

mod compose;
mod governed;
mod log;
mod prompt;
mod rewrite;
mod rotation;
mod safety;

pub use compose::{
    EDIT_RUN_ACTION, LineSource, Nudge, NudgeFacts, Nudger, REQUEST_CHANGE_ACTION, SeedReason,
    WordSource, action, mood_for, render,
};
pub use governed::{
    DynRewrite, GovernedRewriter, REWRITE_MAX_OUTPUT_TOKENS, RewriteReserve, SharedRewriter,
};
pub use log::{
    LOG_WRITE_DEADLINE, RewriteAttempt, RewriteSink, SharedRewriteSink, StoreRewriteSink,
    failure_verdict,
};
pub use prompt::{
    GENTLE_MOOD, NUDGE_REWRITE_INSTRUCTION, PLAYFUL_MOOD, RewritePrompt, VOICE_LABEL,
};
pub use rewrite::{
    CUSTOM_WORD, NoRewrite, NudgeRewriter, REWRITE_DEADLINE, Rejection, RewriteDetail,
    RewriteFailure, RewriteOutcome, accept_rewrite, accept_rewrite_with,
};
pub use rotation::{MAX_CHANNELS, RECENT_PER_CHANNEL, SeedRotation};
pub use safety::{
    DENY_INSIDE, DENY_LIST, DENY_SEA, DENY_SOUNDALIKE, EXACT_ONLY, Hit, RUN_STRICT, SAFE_FORMS,
    WordFilter, builtin_words, denied_word, has_format_char, has_invite, has_markup,
    is_builtin_word,
};
