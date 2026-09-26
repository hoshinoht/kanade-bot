//! The one sequential reaction worker: a ✅/❌ on a proposal card goes to the
//! [`CardDesk`]; on any other message it is an RSVP through the card index.
//! Sequential so a member's add and remove are applied in order. After an
//! RSVP the runs' posted reminder cards are re-rendered (v4
//! `card_needs_refresh`).

use std::sync::Arc;

use serde_json::json;
use tokio::sync::mpsc;

use crate::bot::cards::{CardDesk, CardReaction};
use crate::bot::delivery::cards::ReminderCardStore;
use crate::bot::delivery::{AlertSink, CardRefresh};
use crate::bot::events::{CardIndex, ReactionRouter, ReactionSink, RsvpReaction};
use crate::bot::ids::id_text;
use crate::bot::transport::DiscordTransport;
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
    /// Reminder card edits after an applied RSVP; `None` edits nothing.
    pub refresh: Option<Arc<CardRefresh<S, T>>>,
}

impl<S, T, I, A, X, K> Reactions<S, T, I, A, X, K>
where
    S: ScheduleStore
        + ProposalStore
        + ProposalCardStore
        + DeliveryJournal
        + ReminderCardStore
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
            return Reacted::Card(card);
        }
        match self.rsvp.route(reaction).await {
            Ok(results) => {
                let changed: Vec<String> = results
                    .iter()
                    .filter(|result| result.applied)
                    .map(|result| result.run_id.clone())
                    .collect();
                if let Some(refresh) = &self.refresh
                    && !changed.is_empty()
                {
                    refresh.refresh(&changed).await;
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
