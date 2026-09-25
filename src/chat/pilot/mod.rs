//! The chat pilot's traffic and safety around one question (v4
//! `ChatPilot.offer`/`_answer`): allowance with overrides and refunds, the
//! per-channel answer queue, the clean-retry guard, code-only tool routing,
//! the post-answer glue (reply, history, anchors, focus, pollution
//! containment) and the chat-log rows for answered and rate-limited
//! questions. No Discord types: replies go out through [`ReplyPort`].

mod allowance;
mod guard;
mod reply;
mod traffic;

use std::future::Future;

use chrono::{DateTime, Utc};

pub use allowance::{
    Allowance, AllowanceSnapshot, DEFAULT_MEMBER_ALLOWANCE, DEFAULT_POOL_ALLOWANCE, MemberUsage,
    PoolUsage,
};
pub use guard::{CleanRetryGuard, GuardLimits, GuardView, StormAlert};
pub use reply::{CONTENT_BLOCKED_REPLY, failure_reply};
pub use traffic::{Admission, QueueView, Traffic, TrafficLimits, Waiting};

use crate::chat::answer::{AnswerFailure, Generation, interaction};
use crate::chat::context::{ChatTurn, Conversations, QuestionMessage, TurnRole, question_turn};
use crate::chat::gate::ChatDecision;
use crate::chat::persona::CompiledPersona;
use crate::chat::tools::ToolContext;
use crate::chat::tools::bundles::{CardContext, Signals, ToolOffer, select};
use crate::domain::members::Directory;
use crate::domain::model_log::{ChatInteraction, ChatOutcome};
use crate::infrastructure::llm::Effort;
use crate::infrastructure::llm::governor::Charge;

/// Where the answer is posted (the Discord adapter implements it later).
pub trait ReplyPort {
    /// Reply to `reply_to` in `channel_id`; the posted message's id.
    fn post_reply(
        &self,
        channel_id: &str,
        reply_to: &str,
        text: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

/// The log row's fixed facts.
#[derive(Clone, Debug)]
pub struct LogFacts<'a> {
    pub id: String,
    pub at: DateTime<Utc>,
    pub model: &'a str,
    pub reasoning: Option<Effort>,
    pub latency_ms: u64,
}

/// A finished question to conclude.
pub struct Finished<'a> {
    pub message: &'a QuestionMessage,
    pub channel_id: &'a str,
    pub ctx: &'a ToolContext,
    pub generation: &'a Generation,
    pub persona: &'a CompiledPersona,
    pub directory: &'a (dyn Directory + Sync),
    pub log: LogFacts<'a>,
    /// When the gate spent this question's allowance (`None` for admins).
    pub spent_at: Option<f64>,
    /// Monotonic now.
    pub now: f64,
}

/// What concluding a question did.
#[derive(Clone, Debug, PartialEq)]
pub struct Concluded {
    /// The text posted (the answer, or a fixed failure line).
    pub reply: String,
    /// The posted reply's id, when posting worked.
    pub posted_id: Option<String>,
    /// The exchange was withheld from every later context.
    pub withheld: bool,
    pub interaction: ChatInteraction,
    pub alert: Option<StormAlert>,
}

/// What the Limits page reads (no HTTP here).
#[derive(Clone, Debug, PartialEq)]
pub struct LimitsView {
    pub allowance: AllowanceSnapshot,
    pub queue: QueueView,
    pub clean_retry: GuardView,
}

/// The pilot's in-memory state (a restart forgets it, as v4 did).
#[derive(Clone, Debug)]
pub struct ChatPilot {
    pub conversations: Conversations,
    pub allowance: Allowance,
    pub traffic: Traffic,
    pub guard: CleanRetryGuard,
}

impl ChatPilot {
    pub fn new(history_ttl_s: f64, traffic: TrafficLimits, guard: GuardLimits) -> Self {
        Self {
            conversations: Conversations::new(history_ttl_s),
            allowance: Allowance::default(),
            traffic: Traffic::new(traffic),
            guard: CleanRetryGuard::new(guard),
        }
    }

    /// The tools a question starts with, chosen by code: v4 had no model
    /// pre-screen, so there is no intent label, only the message's own
    /// words and the card it is about.
    pub fn route(text: &str, card: Option<CardContext>, read_only: bool) -> ToolOffer {
        ToolOffer::dynamic(
            select(Signals {
                text,
                intent: None,
                card,
            }),
            read_only,
        )
    }

    /// Whether this member's question may spend a clean retry.
    pub fn clean_retry_allowed(&mut self, member: &str, now: f64) -> bool {
        self.guard.allows(member, now)
    }

    /// A rate-limited question: the once-per-episode reply (if due) and its
    /// log row (`rate_limited`); nothing reaches the model.
    pub fn limited(
        &mut self,
        ctx: &ToolContext,
        question: &str,
        decision: &ChatDecision,
        log: LogFacts<'_>,
        now: f64,
    ) -> (Option<String>, ChatInteraction) {
        let reply = self.allowance.limited_reply(&ctx.author_id, decision, now);
        let mut row = interaction(
            log.id,
            log.at,
            ctx,
            question,
            &Generation::default(),
            log.model,
            log.reasoning,
            log.latency_ms,
        );
        row.outcome = ChatOutcome::RateLimited;
        row.reply = reply.clone().unwrap_or_default();
        row.error = Some(decision.reason.to_owned());
        (reply, row)
    }

    /// Post the reply and fold the exchange into the channel's context.
    ///
    /// A content-filtered question and the reply to it are withheld from
    /// every later context (the `[message withheld]` placeholder), never
    /// anchored, and never pulled back by a reply chain. A question that
    /// failed before any model work is refunded; a sent clean retry is
    /// counted by the storm guard.
    pub async fn conclude<R: ReplyPort>(&mut self, done: Finished<'_>, replies: &R) -> Concluded {
        let generation = done.generation;
        let failure = generation.failure.as_ref();
        let withheld = failure == Some(&AnswerFailure::ContentBlocked);
        let reply = if generation.reply.is_empty() {
            failure_reply(failure, done.persona).to_owned()
        } else {
            generation.reply.clone()
        };
        let posted_id = replies
            .post_reply(done.channel_id, &done.message.id, &reply)
            .await
            .ok()
            .filter(|id| !id.is_empty());

        let mut asked = question_turn(done.message, done.directory);
        let mut answered = ChatTurn::new(TurnRole::Assistant, reply.clone(), posted_id.clone());
        if withheld {
            for id in [Some(&done.message.id), posted_id.as_ref()]
                .into_iter()
                .flatten()
            {
                self.conversations.withhold(id);
            }
            asked.withheld = true;
            answered.withheld = true;
        }
        let conversations = &mut self.conversations;
        conversations.remember(done.channel_id, asked.clone(), done.now);
        conversations.remember(done.channel_id, answered.clone(), done.now);
        conversations.anchor(posted_id.as_deref(), done.channel_id, asked, answered);
        if let Some(focus) = &generation.focus {
            conversations.note_card(done.channel_id, focus, done.now);
        }

        let refunded = failure.is_some_and(|failure| failure.charge() == Charge::Refunded);
        if let (true, Some(stamp)) = (refunded, done.spent_at) {
            self.allowance.refund(&done.ctx.author_id, stamp);
        }
        let alert = generation
            .clean_retry
            .then(|| self.guard.record(&done.ctx.author_id, done.now))
            .flatten();

        let mut row = interaction(
            done.log.id,
            done.log.at,
            done.ctx,
            &done.message.content,
            generation,
            done.log.model,
            done.log.reasoning,
            done.log.latency_ms,
        );
        row.reply = reply.clone();
        row.withheld = withheld;
        Concluded {
            reply,
            posted_id,
            withheld,
            interaction: row,
            alert,
        }
    }

    pub fn limits(&mut self, now: f64) -> LimitsView {
        LimitsView {
            allowance: self.allowance.snapshot(now),
            queue: self.traffic.view(),
            clean_retry: self.guard.view(now),
        }
    }
}
