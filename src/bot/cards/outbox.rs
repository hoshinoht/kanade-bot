//! The extract pipeline's `Outbox` on Discord: cards and self-service links
//! through the journal, chat answers through the reaction path, backlog
//! drops as an admin alert.

use std::sync::Arc;

use crate::bot::delivery::{AdminAlert, AlertSink};
use crate::bot::transport::DiscordTransport;
use crate::domain::drafts::ProposalStore;
use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::notify::DeliveryJournal;
use crate::domain::proposals::ProposalCardStore;
use crate::domain::schedule::{EMOJI_NO, EMOJI_YES, RsvpSource, RsvpState};
use crate::domain::scheduler::{IdSource, ScheduleStore};
use crate::extract::pipeline::{BacklogDrop, Card, ChatAnswer, Outbox, PostResult, Redirected};

use super::desk::CardDesk;

impl<S, T, I, A> CardDesk<S, T, I, A>
where
    S: ScheduleStore + ProposalStore + ProposalCardStore + DeliveryJournal + Send + Sync,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
{
    /// v4 `_apply_rsvp`: the member's ✅/❌ as if reacted (participants
    /// only; status re-derived), then the answer recorded as from chat.
    /// Returns how many answers applied. Decline notices are not sent yet.
    pub async fn apply_answers(&self, answers: &[ChatAnswer]) -> usize {
        let now = self.now();
        let mut applied = 0;
        for answer in answers {
            let emoji = match answer.state {
                RsvpState::Yes => EMOJI_YES,
                RsvpState::No => EMOJI_NO,
                RsvpState::Maybe => continue,
            };
            for user_id in &answer.user_ids {
                let origin = Origin::new(Actor::member(user_id.clone()), Surface::Discord);
                let mut service = self.service(now);
                let Ok(result) = service
                    .as_origin(origin.clone())
                    .apply_reaction(&answer.run_id, user_id, emoji, true)
                    .await
                else {
                    continue;
                };
                if !result.applied {
                    continue;
                }
                applied += 1;
                let _ = service
                    .as_origin(origin)
                    .set_rsvp(&answer.run_id, user_id, answer.state, RsvpSource::Chat)
                    .await;
            }
        }
        applied
    }
}

/// [`CardDesk`] as the pipeline's outbox.
pub struct CardOutbox<S, T, I, A>(pub Arc<CardDesk<S, T, I, A>>);

impl<S, T, I, A> Outbox for CardOutbox<S, T, I, A>
where
    S: ScheduleStore + ProposalStore + ProposalCardStore + DeliveryJournal + Send + Sync,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
{
    async fn redirect(&self, redirected: Redirected) -> PostResult {
        self.0.redirect(&redirected).await
    }

    async fn card(&self, card: Card) -> PostResult {
        self.0.post_card(&card).await
    }

    async fn answers(&self, answers: Vec<ChatAnswer>) {
        self.0.apply_answers(&answers).await;
    }

    async fn backlog_dropped(&self, drop: BacklogDrop) {
        let now = self.0.now();
        self.0.raise(
            AdminAlert::BacklogDropped {
                messages: drop.message_ids.len(),
                capacity: drop.capacity,
            },
            now,
        );
    }
}
