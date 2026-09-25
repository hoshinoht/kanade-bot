//! What the member reads when there is no answer.

use crate::chat::answer::AnswerFailure;
use crate::chat::persona::CompiledPersona;
use crate::chat::sanitize::FAILURE_REPLY;

/// Code-owned default for a content-filtered answer when the persona has
/// no `failures.content_blocked` line; never the provider's refusal text.
pub const CONTENT_BLOCKED_REPLY: &str = "Sorry — I can't help with that one.";

/// The fixed line for a question that produced no reply: the persona's
/// content-blocked line (or the neutral default) for blocked content, v4's
/// `FAILURE_REPLY` for everything else.
pub fn failure_reply<'a>(failure: Option<&AnswerFailure>, persona: &'a CompiledPersona) -> &'a str {
    match failure {
        Some(AnswerFailure::ContentBlocked) => persona
            .content_blocked_line()
            .unwrap_or(CONTENT_BLOCKED_REPLY),
        _ => FAILURE_REPLY,
    }
}
