//! Lifecycle facts for the operator log, handed to [`Answerer::observe`].
//! They carry ids, counts and classes only: never question or reply text,
//! nor member ids (`interaction_id` links to the chat-log row).
//!
//! [`Answerer::observe`]: super::Answerer::observe

use crate::chat::answer::Generation;
use crate::chat::gate::{ChatDecision, POOL_SPENT, RATE_LIMITED, STAFF_ONLY};
use crate::chat::persona::CompileProvenance;
use crate::domain::model_log::ChatInteraction;
use crate::infrastructure::llm::Effort;

use super::Setup;

pub enum ChatEvent<'a> {
    Admitted {
        interaction_id: &'a str,
        thread: bool,
        /// 1-based queue position; `None` when it runs at once.
        position: Option<usize>,
    },
    /// A message that summoned the bot but was not taken.
    Ignored { reason: &'static str },
    /// Concluded after a model attempt (answered or failed).
    Finished {
        interaction: &'a ChatInteraction,
        generation: &'a Generation,
        persona: &'a CompileProvenance,
        model: &'a str,
        reasoning: Option<Effort>,
    },
    Cancelled {
        interaction_id: &'a str,
        reason: &'static str,
    },
    /// `Setup.enabled` or `Setup.ready` differs from the last reading
    /// (the first reading included).
    SetupChanged { enabled: bool, ready: bool },
}

/// The log reason for a summons the gate refused; `None` for refusals that
/// say nothing about this guild's chat (another guild, a DM).
pub(super) fn ignored_reason(decision: &ChatDecision, setup: &Setup) -> Option<&'static str> {
    Some(match decision.reason {
        "the author is a bot" | "the bot's own message" => "bot_author",
        "chat_mode is off" if setup.enabled => "not_ready",
        "chat_mode is off" => "disabled",
        "the chat pilot is not configured" => "not_ready",
        "not a chat channel" => "not_chat_category",
        "the author does not hold the chat role" => "no_pilot_role",
        STAFF_ONLY => "staff_only",
        RATE_LIMITED | POOL_SPENT => "rate_limited",
        _ => return None,
    })
}
