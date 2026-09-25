//! One chat question end to end (v4 `ChatPilot.generate`/`_loop`): tool
//! rounds over one governed question session, one identity session for the
//! whole conversation, cards handed to a caller port, the reserved clean
//! retry, reply finishing, and the chat-log row. No Discord types: the
//! caller supplies the conversation, the channel's pending cards and card
//! posting through [`ChatPorts`].

mod finish;
mod pilot;
mod record;
mod rounds;

use std::fmt;
use std::future::Future;
use std::time::Duration;

use chrono::{NaiveTime, Weekday};
use chrono_tz::Tz;

pub use pilot::{AnswerDeps, answer};
pub use record::{chat_outcome, interaction};
pub use rounds::run_question;

use crate::chat::context::ContextBudgetError;
use crate::chat::gate::{ChannelDirectory, PilotSettings};
use crate::chat::tools::bundles::ToolOffer;
use crate::chat::tools::read::{PendingCard, StrategyGuides};
use crate::chat::tools::{ProposalCard, ToolContext, ToolOutcome};
use crate::domain::catalog::BossTable;
use crate::domain::members::{Directory, Member};
use crate::infrastructure::llm::governor::{Charge, SessionError};
use crate::infrastructure::llm::{Effort, Message};

/// v4's reply when a posted card could not be delivered.
pub const CARD_NOT_POSTED: &str = "The change was recorded but the card could not be posted to the channel. Tell them to check with an admin.";

/// What the caller owns: the channel's pending cards and card posting.
pub trait ChatPorts {
    /// Cards still waiting for a ✅, read before each round.
    fn pending(&self) -> impl Future<Output = Vec<PendingCard>> + Send;

    /// Post one card in the asking channel; `Err` means nobody can see it.
    fn post_card(&self, card: &ProposalCard) -> impl Future<Output = Result<(), String>> + Send;
}

/// The guild facts every round's tools read besides the schedule.
#[derive(Clone, Copy)]
pub struct GuildView<'a> {
    pub members: &'a [Member],
    pub directory: &'a (dyn Directory + Sync),
    pub catalog: &'a BossTable,
    pub channels: &'a (dyn ChannelDirectory + Sync),
    pub pilot: &'a PilotSettings,
    pub zone: Tz,
    pub reset_weekday: Weekday,
    pub reset_time: NaiveTime,
    pub guides: Option<&'a (dyn StrategyGuides + Sync)>,
}

/// Per-question model settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnswerSettings {
    /// Model rounds (D-TOOL-ROUNDS: 8 by default, 1..=12); the last withholds tools.
    pub tool_rounds: u8,
    pub timeout: Duration,
    pub reasoning: Option<Effort>,
    pub temperature: Option<f64>,
    pub max_output_tokens: u32,
    /// `MODEL_CONTEXT_TOKENS`.
    pub model_context_tokens: usize,
    /// The clean retry may be sent (the caller's per-member and storm
    /// guards); when `false` the question fails with the original reason.
    pub clean_retry: bool,
}

/// One question as the loop receives it.
pub struct Question<'a> {
    pub ctx: &'a ToolContext,
    /// System prompt first, the asker's message last (plain text: the loop
    /// encodes it through the identity session).
    pub conversation: Vec<Message>,
    /// The persona's final voice reminder.
    pub reminder: String,
    pub offer: ToolOffer,
    pub settings: AnswerSettings,
}

/// One tool call and the round that asked for it.
#[derive(Clone, Debug, PartialEq)]
pub struct RoundOutcome {
    pub round: u32,
    pub outcome: ToolOutcome,
    /// Cards from this call that reached the channel.
    pub posted: Vec<String>,
}

/// Diagnostics for one model request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRound {
    pub round: u32,
    /// The reply text as the model sent it (identity-decoded).
    pub content: Option<String>,
    pub requested_tools: Vec<String>,
    pub finish_reason: Option<String>,
    /// Bundles offered (`full` for v4's surface, empty when tools were withheld).
    pub bundles: Vec<String>,
    pub latency_ms: u64,
    /// The reserved clean-context retry.
    pub clean: bool,
}

/// Why a question produced no answer. C3 turns these into member-facing
/// lines; the loop only reports them.
#[derive(Clone, Debug, PartialEq)]
pub enum AnswerFailure {
    /// Every round asked for tools.
    KeptCallingTools,
    ContextBudget(ContextBudgetError),
    /// The question's deadline passed (v4 `no answer within Ns`).
    Timeout {
        seconds: u64,
    },
    /// The provider's content filter blocked the answer, clean retry included.
    ContentBlocked,
    /// No usable answer (malformed, empty or naming an unknown identity),
    /// clean retry included.
    Malformed,
    /// The identity route refused (an external route without pseudonymization).
    Route(String),
    /// The governed session failed or turned the question away.
    Session(SessionError),
}

impl AnswerFailure {
    /// Whether the asker's allowance is spent.
    pub fn charge(&self) -> Charge {
        match self {
            Self::Session(error) => error.charge,
            Self::ContextBudget(_) | Self::Route(_) => Charge::Refunded,
            _ => Charge::Charged,
        }
    }
}

impl fmt::Display for AnswerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeptCallingTools => f.write_str("the model kept calling tools"),
            Self::ContextBudget(error) => write!(f, "ContextBudgetError: {error}"),
            Self::Timeout { seconds } => write!(f, "no answer within {seconds}s"),
            Self::ContentBlocked => f.write_str("the provider's content filter blocked the answer"),
            Self::Malformed => f.write_str("the model gave no usable answer"),
            Self::Route(reason) => f.write_str(reason),
            Self::Session(error) => error.fmt(f),
        }
    }
}

/// One question's result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Generation {
    /// Member-facing, finished reply; empty when there is none.
    pub reply: String,
    /// Model rounds of the tool loop (the clean retry is not one).
    pub rounds: u32,
    pub tool_calls: Vec<String>,
    pub outcomes: Vec<RoundOutcome>,
    pub model_rounds: Vec<ModelRound>,
    pub created: Vec<String>,
    pub posted: Vec<String>,
    /// The focus line of the last posted card, for [`Conversations::note_card`].
    ///
    /// [`Conversations::note_card`]: crate::chat::context::Conversations::note_card
    pub focus: Option<String>,
    pub failure: Option<AnswerFailure>,
    pub clean_retry: bool,
    /// Some attempt was content-filtered and no reply came of it, whatever
    /// the final failure (a clean retry may then time out or be malformed).
    pub blocked: bool,
    /// Summed; `None` when no round reported usage.
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Provider requests sent (retries and requeues included).
    pub requests: u32,
    pub model_ms: u64,
    pub tools_ms: u64,
    /// Sent to an external route without pseudonymization (operator override).
    pub external_unmasked: bool,
}

impl Generation {
    pub fn failed(failure: AnswerFailure) -> Self {
        Self {
            failure: Some(failure),
            ..Self::default()
        }
    }

    /// No reply because content was filtered at some attempt.
    pub fn is_blocked(&self) -> bool {
        self.reply.is_empty()
            && (self.blocked || self.failure == Some(AnswerFailure::ContentBlocked))
    }

    fn add_usage(&mut self, prompt: u32, completion: u32) {
        *self.prompt_tokens.get_or_insert(0) += u64::from(prompt);
        *self.completion_tokens.get_or_insert(0) += u64::from(completion);
    }

    /// Tool outcomes without their rounds.
    pub fn tool_outcomes(&self) -> Vec<ToolOutcome> {
        self.outcomes.iter().map(|o| o.outcome.clone()).collect()
    }
}
