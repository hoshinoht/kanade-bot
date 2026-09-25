//! Ephemeral per-channel conversation context (v4 `ChatPilot` history,
//! anchors, focus and reply chains) and the token budgets that bound it.
//! In memory only: a restart forgets it, as v4 did. Monotonic seconds are
//! passed in; nothing here reads a clock.

mod assemble;
mod budget;
mod state;

pub use assemble::{
    Parent, QuestionMessage, Reference, assemble, build_turns, question_turn, reply_chain,
    system_prompt,
};
pub use budget::{ContextBudgetError, budgeted};
pub use state::{Conversations, card_focus};

/// Bounded exchanges kept per channel.
pub const HISTORY_EXCHANGES: usize = 6;
/// How many replied-to parents a reply chain follows.
pub const REPLY_CHAIN_DEPTH: usize = 4;
/// Conversation tokens; the system prompt and tool results are separate.
pub const CONVERSATION_BUDGET_TOKENS: usize = 2500;
/// Replied-author lookups remembered.
pub const REFERENCE_CACHE: usize = 256;
/// Answered exchanges kept for re-anchoring a late reply.
pub const ANCHOR_CACHE: usize = 64;
/// Tokens held back so the model has room to reply.
pub const COMPLETION_RESERVE_TOKENS: usize = 1024;
/// The smallest conversation budget, whatever the system prompt costs.
pub const CONVERSATION_FLOOR_TOKENS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnRole {
    User,
    Assistant,
}

impl TurnRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One remembered line of a channel's conversation, rendered as the model
/// reads it (a member turn is `Name: text`, defused).
#[derive(Clone, Debug, PartialEq)]
pub struct ChatTurn {
    pub role: TurnRole,
    pub content: String,
    /// Dedupes history, reply-chain and anchored turns.
    pub message_id: Option<String>,
    /// Monotonic seconds; unstamped turns never expire.
    pub at: Option<f64>,
    /// Flagged (content-filtered) or the bot's reply to such a turn: kept
    /// only as [`WITHHELD`] in anyone's context (pollution containment).
    pub withheld: bool,
}

impl ChatTurn {
    pub fn new(role: TurnRole, content: impl Into<String>, message_id: Option<String>) -> Self {
        Self {
            role,
            content: content.into(),
            message_id,
            at: None,
            withheld: false,
        }
    }

    /// What a prompt shows for this turn.
    pub fn prompt_text(&self) -> &str {
        if self.withheld {
            WITHHELD
        } else {
            &self.content
        }
    }
}

/// The neutral placeholder for a withheld turn (user decision 2026-09-25).
pub const WITHHELD: &str = "[message withheld]";
