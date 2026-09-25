//! Stored rows. Instants keep microsecond precision; JSON fields hold what
//! the pipeline recorded and are opaque to the store beyond their shape.

use std::fmt;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::outcome::{ChatOutcome, ExtractionOutcome, RescanStatus};
use crate::domain::scheduler::StoreError;

/// One watched Discord message (the extraction window cache, v4
/// `messages`). An edit with different content clears `processed_at`, so
/// the message is read again.
#[derive(Clone, PartialEq, Eq)]
pub struct WatchedMessage {
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub content: String,
    pub processed_at: Option<DateTime<Utc>>,
}

/// `found` in the order of `ids`, each id once (the by-ids read's order).
pub fn in_order(ids: &[String], mut found: Vec<WatchedMessage>) -> Vec<WatchedMessage> {
    let mut ordered = Vec::with_capacity(found.len());
    for id in ids {
        if let Some(index) = found.iter().position(|message| &message.id == id) {
            ordered.push(found.swap_remove(index));
        }
    }
    ordered
}

/// One extraction pass (v4 `extractions`, plus the v5 filter fields).
#[derive(Clone, PartialEq, Eq)]
pub struct ExtractionLog {
    pub id: String,
    pub at: DateTime<Utc>,
    pub channel_id: Option<String>,
    /// Authors of the messages read (the `member` filter).
    pub member_ids: Vec<String>,
    /// Model alias.
    pub model: String,
    pub reasoning: Option<String>,
    pub prompt: String,
    pub raw_response: String,
    pub latency_ms: Option<u64>,
    pub request_count: u32,
    pub outcome: ExtractionOutcome,
    pub error: Option<String>,
    /// Guardrail signals; always a JSON object (`{}` when none).
    pub guardrail: Value,
    pub message_ids: Vec<String>,
    /// Proposal draft ids (v4 `amendment_ids`).
    pub proposal_ids: Vec<String>,
    /// Changes refused up front (`D-PROPOSE-REFUSES`); `error` is only for
    /// failures.
    pub refusals: Vec<ExtractionRefusal>,
}

/// One change the scheduler refused to stage: the change kind, a stable
/// reason code and the (v4-worded) message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractionRefusal {
    pub change: String,
    pub code: String,
    pub message: String,
}

impl ExtractionRefusal {
    pub fn to_json(&self) -> Value {
        serde_json::json!({ "change": self.change, "code": self.code, "message": self.message })
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        let field = |name: &str| value.get(name)?.as_str().map(str::to_owned);
        Some(Self {
            change: field("change")?,
            code: field("code")?,
            message: field("message")?,
        })
    }
}

/// One model request within a chat question.
#[derive(Clone, PartialEq, Eq)]
pub struct ChatRound {
    /// Model alias.
    pub model: String,
    pub reasoning: Option<String>,
    pub finish_reason: Option<String>,
    pub latency_ms: Option<u64>,
    /// Tool bundles offered this round.
    pub tool_bundles: Vec<String>,
    /// Tools called this round, in call order (the `tool` filter).
    pub tools: Vec<String>,
    /// Tool-call diagnostics; always a JSON array.
    pub tool_calls: Value,
    /// The provider's response for the round (prompts are never stored).
    pub response: Option<String>,
}

/// One chat question (v4 `chat_interactions`, plus the v5 filter fields).
#[derive(Clone, PartialEq, Eq)]
pub struct ChatInteraction {
    pub id: String,
    pub at: DateTime<Utc>,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub member_id: Option<String>,
    pub question: String,
    pub reply: String,
    pub outcome: ChatOutcome,
    pub error: Option<String>,
    /// A clean-context retry was used.
    pub clean_retry: bool,
    /// The turn was withheld from shared context.
    pub withheld: bool,
    /// Guardrail signals; always a JSON object (`{}` when none).
    pub guardrail: Value,
    pub request_count: u32,
    pub latency_ms: Option<u64>,
    pub model_ms: Option<u64>,
    pub tools_ms: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Model rounds in order.
    pub rounds: Vec<ChatRound>,
}

/// One rescan job (v4 `rescan_jobs`). `window` is kept as given (v4 and v5
/// spell windows differently); the API validates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RescanJob {
    pub id: String,
    pub channels: Vec<String>,
    pub window: String,
    pub source: String,
    pub automated: bool,
    pub requested_by: Option<String>,
    pub status: RescanStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Per-channel results; always a JSON array.
    pub results: Value,
    pub error: Option<String>,
}

/// A member's chat allowance override (v4 `chat_rate_limits`; the window
/// is whole milliseconds instead of v4's float seconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowanceOverride {
    pub member_id: String,
    pub count: u32,
    pub window_ms: u64,
    pub updated_at: DateTime<Utc>,
}

fn shape(ok: bool, what: &str) -> Result<(), StoreError> {
    if ok {
        Ok(())
    } else {
        Err(StoreError::Constraint(format!(
            "{what} has the wrong shape"
        )))
    }
}

impl ExtractionLog {
    /// The shape every store refuses to write otherwise.
    pub fn check_shape(&self) -> Result<(), StoreError> {
        shape(self.guardrail.is_object(), "extraction guardrail")
    }
}

impl ChatInteraction {
    /// The shape every store refuses to write otherwise.
    pub fn check_shape(&self) -> Result<(), StoreError> {
        shape(self.guardrail.is_object(), "chat guardrail")?;
        for round in &self.rounds {
            shape(round.tool_calls.is_array(), "chat round tool_calls")?;
        }
        Ok(())
    }
}

impl RescanJob {
    /// The shape every store refuses to write otherwise.
    pub fn check_shape(&self) -> Result<(), StoreError> {
        shape(self.results.is_array(), "rescan results")?;
        shape(!self.window.is_empty(), "rescan window")
    }
}

impl AllowanceOverride {
    /// The shape every store refuses to write otherwise.
    pub fn check_shape(&self) -> Result<(), StoreError> {
        shape(self.window_ms > 0, "allowance window")
    }
}

// Debug shows ids, instants and sizes only: member text, prompts and model
// output must not reach logs or panic messages.

impl fmt::Debug for WatchedMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchedMessage")
            .field("id", &self.id)
            .field("channel_id", &self.channel_id)
            .field("created_at", &self.created_at)
            .field("edited_at", &self.edited_at)
            .field("content_len", &self.content.len())
            .field("processed_at", &self.processed_at)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ExtractionLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractionLog")
            .field("id", &self.id)
            .field("at", &self.at)
            .field("channel_id", &self.channel_id)
            .field("model", &self.model)
            .field("outcome", &self.outcome)
            .field("request_count", &self.request_count)
            .field("latency_ms", &self.latency_ms)
            .field("prompt_len", &self.prompt.len())
            .field("raw_response_len", &self.raw_response.len())
            .field("messages", &self.message_ids.len())
            .field("proposal_ids", &self.proposal_ids)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ChatRound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatRound")
            .field("model", &self.model)
            .field("finish_reason", &self.finish_reason)
            .field("latency_ms", &self.latency_ms)
            .field("tools", &self.tools)
            .field("response_len", &self.response.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ChatInteraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatInteraction")
            .field("id", &self.id)
            .field("at", &self.at)
            .field("channel_id", &self.channel_id)
            .field("outcome", &self.outcome)
            .field("clean_retry", &self.clean_retry)
            .field("withheld", &self.withheld)
            .field("request_count", &self.request_count)
            .field("latency_ms", &self.latency_ms)
            .field("question_len", &self.question.len())
            .field("reply_len", &self.reply.len())
            .field("rounds", &self.rounds)
            .finish_non_exhaustive()
    }
}
