//! One chat question end to end (v4 `ChatPilot.generate`/`_loop`): tool
//! rounds over one governed question session, cards handed to a caller port,
//! the reserved clean retry, reply finishing, and the chat-log row. No Discord types: the
//! caller supplies the conversation, the channel's pending cards and card
//! posting through [`ChatPorts`].

mod cover;
mod finish;
mod pilot;
mod profanity;
mod record;
mod rounds;

use std::fmt;
use std::future::Future;
use std::time::Duration;

use chrono::{NaiveTime, Weekday};
use chrono_tz::Tz;

pub use pilot::{AnswerDeps, answer};
pub use profanity::{ProfanityGuard, ProfanityHit, ProfanitySide};
pub use record::{chat_outcome, interaction, with_persona};
pub use rounds::run_question;

use crate::chat::context::ContextBudgetError;
use crate::chat::gate::{ChannelDirectory, PilotSettings};
use crate::chat::tools::bundles::ToolOffer;
use crate::chat::tools::read::{PendingCard, StrategyGuides};
use crate::chat::tools::{ProposalCard, ToolContext, ToolOutcome};
use crate::domain::catalog::BossTable;
use crate::domain::members::{Directory, Member};
use crate::infrastructure::llm::governor::{Charge, SentRequest, SessionError};
use crate::infrastructure::llm::{Effort, Message};

/// v4's reply when a posted card could not be delivered.
pub const CARD_NOT_POSTED: &str = "The change was recorded but the card could not be posted to the channel. Tell them to check with an admin.";

/// The member-facing line when protected prompt material exceeds a route's
/// context window before any model request can be sent.
pub const CONTEXT_BUDGET_REPLY: &str =
    "Sorry — that question is too long for this model's context. Please shorten it and try again.";

/// What the caller owns: the channel's pending cards and card posting.
pub trait ChatPorts {
    /// Cards still waiting for a ✅, read before each round.
    fn pending(&self) -> impl Future<Output = Vec<PendingCard>> + Send;

    /// Post one card in the asking channel; `Err` means nobody can see it.
    fn post_card(&self, card: &ProposalCard) -> impl Future<Output = Result<(), String>> + Send;

    /// Refresh retired cards when reusing a proposal instead of posting one.
    fn refresh_proposals(&self, proposal_ids: &[String]) -> impl Future<Output = ()> + Send;
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
    /// System prompt first, the asker's message last; all content is sent as supplied.
    pub conversation: Vec<Message>,
    /// The persona's final voice reminder.
    pub reminder: String,
    pub offer: ToolOffer,
    pub settings: AnswerSettings,
    /// The live profanity guardrail; `None` checks nothing.
    pub profanity: Option<&'a ProfanityGuard>,
}

/// One tool call and the round that asked for it.
#[derive(Clone, Debug, PartialEq)]
pub struct RoundOutcome {
    pub round: u32,
    pub outcome: ToolOutcome,
    /// Cards from this call that reached the channel.
    pub posted: Vec<String>,
    /// Wall time of the call: store load, dispatch and card posting.
    pub took_ms: u64,
}

/// Diagnostics for one model request.
#[derive(Clone, PartialEq, Eq)]
pub struct ModelRound {
    pub round: u32,
    pub reasoning_content: Option<String>,
    pub reasoning_tokens: Option<u64>,
    /// The reply text as the model sent it.
    pub content: Option<String>,
    pub requested_tools: Vec<String>,
    pub finish_reason: Option<String>,
    /// Bundles offered (`full` for v4's surface, empty when tools were withheld).
    pub bundles: Vec<String>,
    pub latency_ms: u64,
    /// The reserved clean-context retry.
    pub clean: bool,
    /// What the governed session actually sent (alias, reasoning effort).
    pub sent: Option<SentRequest>,
    /// Provider-reported usage for this request; both or neither.
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// The budget's prompt estimate for this request, completion reserve
    /// excluded.
    pub prompt_estimate: Option<u64>,
    /// The `x-request-id`s this round sent (retries included).
    pub request_ids: Vec<String>,
}

impl fmt::Debug for ModelRound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelRound")
            .field("round", &self.round)
            .field("content_bytes", &self.content.as_ref().map(String::len))
            .field(
                "reasoning_bytes",
                &self.reasoning_content.as_ref().map(String::len),
            )
            .field("reasoning_tokens", &self.reasoning_tokens)
            .field("latency_ms", &self.latency_ms)
            .field("finish_reason", &self.finish_reason)
            .finish_non_exhaustive()
    }
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
    /// No usable answer (malformed or empty), clean retry included.
    Malformed,
    /// The governed session failed or turned the question away.
    Session(SessionError),
}

impl AnswerFailure {
    /// A stable code for the log and the admin transcript.
    pub fn code(&self) -> &'static str {
        match self {
            Self::KeptCallingTools => "kept_calling_tools",
            Self::ContextBudget(_) => "context_budget",
            Self::Timeout { .. } => "timeout",
            Self::ContentBlocked => "content_blocked",
            Self::Malformed => "malformed",
            Self::Session(error) => error.code(),
        }
    }

    /// Whether the asker's allowance is spent.
    pub fn charge(&self) -> Charge {
        match self {
            Self::Session(error) => error.charge,
            Self::ContextBudget(_) => Charge::Refunded,
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
    /// The question session's correlation stem, once it sent a tagged request.
    pub session_id: Option<String>,
    /// Every `x-request-id` the session sent, failed requests included (a
    /// failed request answers no round, so rounds alone miss it).
    pub request_ids: Vec<String>,
    pub model_ms: u64,
    pub tools_ms: u64,
    /// The chat route sends raw member data outside the homelab.
    pub external_unmasked: bool,
    /// The chat route leaves the homelab.
    pub external: bool,
    /// A profanity guardrail hit (question deflected, or reply retried or replaced).
    pub profanity: Option<ProfanityHit>,
}

impl Generation {
    pub fn failed(failure: AnswerFailure) -> Self {
        Self {
            failure: Some(failure),
            ..Self::default()
        }
    }

    /// A profanity hit whose question or reply was replaced by the
    /// deflection line: the exchange stays out of every later context.
    pub fn kept_out_of_context(&self) -> bool {
        self.profanity
            .as_ref()
            .is_some_and(|hit| hit.sent.is_some())
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

    /// `homelab` or `external_unmasked`.
    pub fn route(&self) -> &'static str {
        if self.external {
            "external_unmasked"
        } else {
            "homelab"
        }
    }

    /// Tool outcomes without their rounds.
    pub fn tool_outcomes(&self) -> Vec<ToolOutcome> {
        self.outcomes.iter().map(|o| o.outcome.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::llm::governor::{Refused, SessionFailure};
    use crate::infrastructure::llm::{ErrorCode, LlmError};

    /// The chat log's codes, byte-identical to the table they had before
    /// `SessionError::code` became their single source.
    #[test]
    fn session_failure_codes_are_unchanged() {
        let failed = |failure| {
            AnswerFailure::Session(SessionError {
                failure,
                charge: Charge::Refunded,
            })
            .code()
        };
        let refused = [
            (Refused::UnknownRole, "unknown_role"),
            (Refused::Ungrouped, "ungrouped"),
            (Refused::ExternalForbidden, "external_forbidden"),
            (Refused::MustNotWait, "must_not_wait"),
            (Refused::Busy, "busy"),
            (Refused::Timeout, "queue_timeout"),
            (
                Refused::Unavailable { retry_at: None },
                "backend_unavailable",
            ),
            (
                Refused::RateLimited {
                    wait: Duration::ZERO,
                },
                "rate_ceiling",
            ),
            (Refused::RetryBudgetExhausted, "retry_budget_exhausted"),
        ];
        for (refused, code) in refused {
            assert_eq!(failed(SessionFailure::Refused(refused)), code);
        }
        for (failure, code) in [
            (SessionFailure::RequestsExhausted, "requests_exhausted"),
            (SessionFailure::Ended, "session_ended"),
            (
                SessionFailure::CleanRetryUnavailable,
                "clean_retry_unavailable",
            ),
            (
                SessionFailure::AnswerRetryUnavailable,
                "answer_retry_unavailable",
            ),
        ] {
            assert_eq!(failed(failure), code);
        }
        for (error, code) in [
            (ErrorCode::RequestInvalid, "request_invalid"),
            (ErrorCode::BudgetExceeded, "budget_exceeded"),
            (ErrorCode::DeadlineExceeded, "deadline_exceeded"),
            (ErrorCode::ProviderPermanent, "provider_permanent"),
            (ErrorCode::ProviderAuthentication, "provider_authentication"),
            (ErrorCode::InvalidOutput, "invalid_output"),
            (ErrorCode::ModelMismatch, "model_mismatch"),
            (ErrorCode::Incomplete, "incomplete"),
            (ErrorCode::ContentFiltered, "content_filtered"),
            (ErrorCode::UnsupportedCapability, "unsupported_capability"),
            (ErrorCode::AdmissionRefused, "admission_refused"),
            (ErrorCode::BackendUnavailable, "backend_unavailable"),
            (ErrorCode::UpstreamTimeout, "upstream_timeout"),
            (ErrorCode::KeyExpired, "key_expired"),
        ] {
            assert_eq!(
                failed(SessionFailure::Model(LlmError::new(error, "test"))),
                code
            );
        }
    }
}
