//! The chatbot's closed tool surface (v4 `bot/chat/tools`): schemas, the
//! read tools, the proposal tools, v5's dynamic tool bundles and the guarded
//! dispatch boundary. Tool logic is pure over an injected [`read::ToolWorld`]
//! and clock; proposals go only through the scheduler's propose API.

pub mod bundles;
pub mod dispatch;
pub mod propose;
pub mod read;
pub mod schemas;

use std::fmt;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

pub use propose::ProposalCard;
pub use schemas::ToolName;

/// Runs a schedule answer lists before it is truncated.
pub const MAX_RUNS: usize = 20;
/// Member replies stay comfortably below Discord's message ceiling.
pub const MAX_MEMBER_REPLY: usize = 1200;

/// A call for something that is not a tool; `{name}` and `{known}` are
/// filled in. Phrased as an instruction so a small model picks a real tool.
pub const UNKNOWN_TOOL: &str = "There is no tool called {name}. The tools you have are: {known}. Use one of those when it can answer the request.";

/// A card-posting tool on a read-only turn.
pub const READ_ONLY_TURN: &str = "You cannot post a card in this message. Ask them in words what it should be instead, and stop there -- their answer comes back to you as a normal message and you can post the card then.";

/// The log's words for what went wrong with a call.
pub const REFUSED: &str = "refused";
pub const UNKNOWN: &str = "unknown tool";
pub const FAILED: &str = "failed";

/// What a tool says when something unexpected broke underneath it.
pub const LOOKUP_FAILED: &str =
    "That lookup failed. Say you could not complete that request just now.";

/// A refusal the model may read and should say something about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ToolError {}

pub type ToolResult<T> = Result<T, ToolError>;

/// Why a call produced no answer: a refusal the model reads, or a failure
/// underneath it (logged; the model reads [`LOOKUP_FAILED`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    Refused(ToolError),
    Failed(String),
}

impl From<ToolError> for CallError {
    fn from(error: ToolError) -> Self {
        Self::Refused(error)
    }
}

/// Who is asking, from where, and when: trusted runtime state, never
/// anything the model said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContext {
    /// The Discord id of the person whose message is being answered.
    pub author_id: String,
    pub channel_id: String,
    pub message_id: String,
    /// The chat interaction a proposal is recorded against.
    pub source_id: String,
    /// The one exemption from the authority checks; least privilege by default.
    pub is_admin: bool,
    /// No card may be posted this turn, whatever the model asks for.
    pub read_only: bool,
    pub bot_user_id: Option<String>,
    /// The bot's user and display names, stripped from party fields.
    pub bot_names: Vec<String>,
    pub self_role_id: Option<String>,
    pub force_all_channels: bool,
    pub force_channel_scope: bool,
    pub force_group_schedule: bool,
    pub self_schedule_requested: bool,
    pub upcoming_only: bool,
    /// The answer's single clock reading.
    pub now: DateTime<Utc>,
}

impl ToolContext {
    pub fn new(
        author_id: impl Into<String>,
        channel_id: impl Into<String>,
        message_id: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let message_id = message_id.into();
        Self {
            author_id: author_id.into(),
            channel_id: channel_id.into(),
            source_id: message_id.clone(),
            message_id,
            is_admin: false,
            read_only: false,
            bot_user_id: None,
            bot_names: Vec::new(),
            self_role_id: None,
            force_all_channels: false,
            force_channel_scope: false,
            force_group_schedule: false,
            self_schedule_requested: false,
            upcoming_only: false,
            now,
        }
    }
}

/// One tool call as the log describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    pub name: String,
    /// What the model reads (before identity encoding).
    pub output: String,
    /// The decoded arguments the tool ran with (`{}` for malformed ones).
    pub arguments: Map<String, Value>,
    pub ok: bool,
    /// [`REFUSED`], [`UNKNOWN`] or [`FAILED`] when not ok.
    pub error: Option<&'static str>,
    /// Proposal ids this call created.
    pub created: Vec<String>,
    /// Cards for the caller to post, one per created proposal.
    pub cards: Vec<ProposalCard>,
    /// A [`FAILED`] call's cause, for the chat log only.
    pub detail: Option<String>,
}

impl ToolOutcome {
    pub fn outcome(&self) -> &str {
        if self.ok {
            "ok"
        } else {
            self.error.unwrap_or(FAILED)
        }
    }
}
