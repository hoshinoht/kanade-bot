//! What the pipeline needs from outside, kept free of Discord types: message
//! events in; guild facts; the scheduler's proposal service; and an outbox for
//! cards, chat answers, self-service redirects and backlog audits (E5/N1 and
//! serve wiring implement it).

use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::domain::catalog::BossTable;
use crate::domain::proposals::ProposedChange;
use crate::domain::schedule::RsvpState;
use crate::domain::scheduler::{ProposalRequest, ProposalResult, Proposed, SupersedeScope};
use crate::extract::AmendmentKind;
use crate::infrastructure::llm::identity::Member;

/// Who wrote a message. Only members' messages are ever stored or read; the
/// rest are the loop guard (bot, webhook and the bot's own posts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorKind {
    Member,
    Bot,
    Webhook,
    Myself,
}

/// Live gateway traffic, or history arriving late (a RESUME replay, a
/// backfill on start): late messages go to the backlog, not the debounce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageOrigin {
    Live,
    Replay,
}

/// One message as the Discord adapter hands it over. `channel_id` is the
/// parent channel for thread messages (v4 `origin_ids`).
#[derive(Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub author: AuthorKind,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub content: String,
    pub origin: MessageOrigin,
    /// The chatbot answered it: stored, never offered to the extractor (v4).
    pub handled_by_chat: bool,
}

impl std::fmt::Debug for IncomingMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingMessage")
            .field("id", &self.id)
            .field("channel_id", &self.channel_id)
            .field("author", &self.author)
            .field("origin", &self.origin)
            .field("content_len", &self.content.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageEvent {
    Posted(IncomingMessage),
    /// Re-debounces the channel; never an urgent flush.
    Edited(IncomingMessage),
    Deleted {
        id: String,
    },
}

/// Guild facts owned elsewhere (member table, gateway, runtime config).
pub trait Guild: Send + Sync {
    /// The extract switch is on and the bot is not paused.
    fn extraction_enabled(&self) -> bool;
    fn is_watched(&self, channel_id: &str) -> bool;
    /// Every known member (the prompt roster and the gate's mention list).
    fn members(&self) -> Vec<Member>;
    /// Holds the bossing role: only their messages are read (v4 DESIGN §1).
    fn has_role(&self, user_id: &str) -> bool;
    fn bosses(&self) -> Arc<BossTable>;
    fn channel_name(&self, channel_id: &str) -> String;
}

/// The scheduler's proposal service (`SchedulerService::propose` and
/// `supersede_proposals`); the only way the pipeline changes anything.
pub trait Proposer: Send + Sync {
    fn supersede(
        &self,
        scope: SupersedeScope<'_>,
    ) -> impl Future<Output = ProposalResult<Vec<String>>> + Send;

    fn propose(
        &self,
        request: ProposalRequest,
    ) -> impl Future<Output = ProposalResult<Proposed>> + Send;
}

/// A kept change before it is proposed, as the self-service redirect sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectOffer {
    pub channel_id: String,
    pub change: ProposedChange,
    /// Authors of the evidence messages.
    pub authors: Vec<String>,
}

/// One proposed change on a card, with what the card shows beside it.
#[derive(Clone, Debug, PartialEq)]
pub struct CardEntry {
    pub proposal_id: String,
    pub kind: AmendmentKind,
    pub run_id: Option<String>,
    pub summary: String,
    pub is_question: bool,
    /// No time was pinned down, or it was asked: word it as a suggestion.
    pub needs_answer: bool,
    pub confidence: f64,
    pub also_mentioned: Vec<AmendmentKind>,
    pub day_ref: Option<String>,
    pub time_ref: Option<String>,
    pub evidence_message_ids: Vec<String>,
}

/// One card per burst (or per rescan channel pass).
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub channel_id: String,
    pub entries: Vec<CardEntry>,
    /// Live proposals this pass retired (their cards say so).
    pub superseded: Vec<String>,
}

/// An RSVP read from chat, applied through the reaction path (v4
/// `_apply_rsvp`: ✅/❌ as the member, decline notices) by the outbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatAnswer {
    pub channel_id: String,
    pub run_id: String,
    pub user_ids: Vec<String>,
    pub state: RsvpState,
}

/// Message ids the backlog dropped (oldest first) because it was full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacklogDrop {
    pub message_ids: Vec<String>,
    pub capacity: usize,
}

pub trait Outbox: Send + Sync {
    /// Self-service redirect seam (slice N1): `true` when a link was sent
    /// instead of a card, so no proposal is created.
    fn redirect(&self, offer: &RedirectOffer) -> impl Future<Output = bool> + Send;

    fn card(&self, card: Card) -> impl Future<Output = ()> + Send;

    fn answers(&self, answers: Vec<ChatAnswer>) -> impl Future<Output = ()> + Send;

    /// Audit: the backlog was full and dropped its oldest entries.
    fn backlog_dropped(&self, drop: BacklogDrop) -> impl Future<Output = ()> + Send;
}
