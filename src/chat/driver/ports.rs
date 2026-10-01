//! What the driver needs from the model side ([`Answerer`]) and from Discord
//! ([`Surface`]), and the plain values that cross them.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

use super::ChatEvent;
use crate::chat::answer::{Generation, Question};
use crate::chat::context::QuestionMessage;
use crate::chat::gate::{ChannelDirectory, IncomingMessage, PilotSettings};
use crate::chat::persona::CompiledPersona;
use crate::chat::pilot::StormAlert;
use crate::domain::catalog::BossTable;
use crate::domain::members::{Directory, MemberProfile};
use crate::domain::model_log::ChatInteraction;
use crate::infrastructure::llm::{Effort, governor::RoleRoute};

/// One member message as the adapter hands it over.
#[derive(Clone)]
pub struct Asked {
    pub message: QuestionMessage,
    /// Where it was posted (a thread's own id): the reply goes here.
    pub channel_id: String,
    /// A thread's parent, else `channel_id`: the queue, history and cards key.
    pub origin_id: String,
    /// The gate's view: resolved author roles, mentions and channel.
    pub gate: IncomingMessage,
    pub replied_author_id: Option<String>,
    pub bot_user_id: Option<String>,
    /// The bot's managed role: `@Kanade` often arrives as this role mention.
    pub self_role_id: Option<String>,
    /// Staff: exempt from the role gate and both allowances.
    pub is_admin: bool,
}

/// Live settings read for every message.
#[derive(Clone, Debug)]
pub struct Setup {
    /// `chatbot.enabled`.
    pub enabled: bool,
    /// A chat model route and an active persona exist.
    pub ready: bool,
    pub pilot: PilotSettings,
    pub member_rate: (usize, f64),
    pub pool_rate: (usize, f64),
    /// The chat alias for rate-limited rows; `""` when unrouted.
    pub model: String,
    pub now: DateTime<Utc>,
}

/// Everything one question needs besides the pilot, read when it starts.
pub struct Prepared {
    pub persona: CompiledPersona,
    /// The boss catalog the staging line resolves names through.
    pub catalog: Arc<BossTable>,
    /// Changes when the persona's identity does (history is then forgotten).
    pub persona_key: String,
    pub directory: Arc<dyn Directory + Send + Sync>,
    /// The member rows the directory was built from (tools, identity).
    pub members: Vec<MemberProfile>,
    pub pilot: PilotSettings,
    /// `route`'s alias and effort: the system prompt, logs and requests
    /// all use them.
    pub model: String,
    pub reasoning: Option<Effort>,
    /// Context and completion reserve resolved with this question's route;
    /// a later Config save cannot alter an in-flight question.
    pub context_window: usize,
    pub max_output_tokens: u32,
    pub context_source: &'static str,
    /// The chat route read once for this question; the answer opens its
    /// session on it (`None`: the answerer reads its own).
    pub route: Option<RoleRoute>,
    /// The question's single wall-clock reading.
    pub now: DateTime<Utc>,
    pub zone: Tz,
    pub reset: (Weekday, NaiveTime),
    pub bot_names: Vec<String>,
}

/// One question for the model side.
pub struct Job<'a> {
    pub prepared: &'a Prepared,
    pub asked: &'a Asked,
    pub question: Question<'a>,
    /// Set when the question was deleted: post nothing more for it.
    pub cancelled: &'a Arc<AtomicBool>,
}

/// The model, persona, store and card side of a question.
pub trait Answerer: Send + Sync + 'static {
    fn setup(&self) -> Setup;

    /// Channels and threads for the gate's category check.
    fn channels(&self) -> &(dyn ChannelDirectory + Send + Sync);

    /// `None` when chat cannot answer now (no persona or model route).
    fn prepare(&self, asked: &Asked) -> impl Future<Output = Option<Prepared>> + Send;

    /// Never fails: failures come back in the generation.
    fn answer(&self, job: Job<'_>) -> impl Future<Output = Generation> + Send;

    /// Persist one chat-log row.
    fn record(&self, row: ChatInteraction) -> impl Future<Output = ()> + Send;

    fn storm(&self, alert: &StormAlert);

    /// Lifecycle facts for the operator log.
    fn observe(&self, _event: &ChatEvent<'_>) {}
}

/// What one message effect did: the transport's outcome classes, without
/// Discord types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect<T> {
    Done(T),
    /// Certainly never processed (not sent, or rate limited out): one retry
    /// is safe.
    NotSent,
    /// The target message does not exist.
    UnknownMessage,
    /// Refused; nothing happened. A content-free label.
    Rejected(String),
    /// May or may not have happened. A content-free label.
    Ambiguous(String),
}

/// A new message. It always mentions nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Post<'a> {
    pub text: &'a str,
    /// Reply to this message (a deleted target still posts).
    pub reply_to: Option<&'a str>,
    /// Discord's `@silent`: notifies nobody.
    pub silent: bool,
}

/// Reactions on the asking message and the message effects that deliver an
/// answer. Delivery is outside the delivery journal: the driver applies the
/// retry rules (`delivery.rs`); an implementation never retries.
pub trait Surface: Send + Sync + 'static {
    fn react(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> impl Future<Output = ()> + Send;

    fn unreact(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> impl Future<Output = ()> + Send;

    /// Post a message; the posted message's id.
    fn post(&self, channel_id: &str, post: Post<'_>)
    -> impl Future<Output = Effect<String>> + Send;

    /// Replace a message's text, mentioning nobody.
    fn edit(
        &self,
        channel_id: &str,
        message_id: &str,
        text: &str,
    ) -> impl Future<Output = Effect<()>> + Send;

    fn delete(&self, channel_id: &str, message_id: &str)
    -> impl Future<Output = Effect<()>> + Send;

    /// Show the bot typing; fire and forget.
    fn typing(&self, channel_id: &str) -> impl Future<Output = ()> + Send;
}
