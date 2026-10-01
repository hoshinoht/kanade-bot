//! The one sequential reaction worker: a ✅/❌ on a proposal card goes to the
//! [`CardDesk`]; on any other message it is an RSVP through the card index.
//! Sequential so a member's add and remove are applied in order.

use std::sync::Arc;

use serde_json::json;
use tokio::sync::mpsc;

use crate::api::auth::Clock;
use crate::api::state::DeclineRetraction;
use crate::bot::cards::{CardDesk, CardReaction};
use crate::bot::delivery::AlertSink;
use crate::bot::events::{CardIndex, ReactionRouter, ReactionSink, RsvpReaction};
use crate::bot::ids::id_text;
use crate::bot::transport::DiscordTransport;
use crate::chat::driver::{FollowUpCard, FollowUpRequest, RejectionFollowUp};
use crate::domain::drafts::ProposalStore;
use crate::domain::notify::DeliveryJournal;
use crate::domain::proposals::ProposalCardStore;
use crate::domain::scheduler::{IdSource, ScheduleStore};
use crate::runtime::logging;

/// What one reaction did, for tests and logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reacted {
    Card(CardReaction),
    /// RSVP results, one per run on the card.
    Rsvp(usize),
    Failed,
}

pub struct Reactions<S, T, I, A, X, K> {
    /// Shared with extraction's card outbox.
    pub desk: Arc<CardDesk<S, T, I, A>>,
    pub rsvp: ReactionRouter<X, K>,
    /// Optional while chat is unavailable; rejection follow-ups never delay
    /// the sequential reaction worker beyond their scope checks.
    pub follow_up: Option<Arc<dyn RejectionFollowUp>>,
    /// Best-effort S2 deletion after a committed RSVP answer replaces a no.
    pub decline_retraction: Option<DeclineRetraction>,
    pub clock: Clock,
}

impl<S, T, I, A, X, K> Reactions<S, T, I, A, X, K>
where
    S: ScheduleStore
        + crate::domain::notify::DeclineNoticeStore
        + ProposalStore
        + ProposalCardStore
        + DeliveryJournal
        + Send
        + Sync,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
    X: CardIndex,
    K: ReactionSink,
{
    pub async fn apply(&mut self, reaction: &RsvpReaction) -> Reacted {
        let card = self
            .desk
            .on_reaction(
                &id_text(reaction.message_id),
                &id_text(reaction.user_id),
                reaction.answer,
                reaction.added,
            )
            .await;
        if card != CardReaction::NotACard {
            if let CardReaction::Rejected { proposal_ids } = &card
                && let (Some(follow_up), Some(facts)) = (
                    &self.follow_up,
                    self.desk.rejection_follow_up(proposal_ids).await,
                )
            {
                follow_up
                    .rejected(FollowUpRequest {
                        card_message_id: id_text(reaction.message_id),
                        channel_id: facts.channel_id,
                        reactor_id: id_text(reaction.user_id),
                        source_ids: facts.source_ids,
                        cards: facts
                            .cards
                            .into_iter()
                            .map(|card| FollowUpCard {
                                summary: card.summary,
                                bosses: card.bosses,
                                participants: card.participants,
                            })
                            .collect(),
                    })
                    .await;
            }
            return Reacted::Card(card);
        }
        match self.rsvp.route_declines(reaction).await {
            Ok(results) => {
                for routed in &results {
                    if routed.retract
                        && let Some(retract) = &self.decline_retraction
                    {
                        retract(
                            routed.result.run_id.clone(),
                            id_text(reaction.user_id),
                            (self.clock)(),
                        )
                        .await;
                    }
                }
                Reacted::Rsvp(results.len())
            }
            Err(error) => {
                // Scheduler/lookup text can quote store errors; log the kind only.
                let kind = match error {
                    crate::bot::events::RouteError::Lookup(_) => "lookup",
                    crate::bot::events::RouteError::Scheduler(_) => "scheduler",
                };
                logging::event("WARN", "rsvp_failed", json!({"kind": kind}));
                Reacted::Failed
            }
        }
    }

    /// Apply reactions until every sender is dropped.
    pub async fn run(mut self, mut reactions: mpsc::UnboundedReceiver<RsvpReaction>) {
        while let Some(reaction) = reactions.recv().await {
            self.apply(&reaction).await;
        }
    }
}
