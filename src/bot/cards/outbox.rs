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
use crate::domain::scheduler::{DeclineNoticeContext, IdSource, ScheduleStore};
use crate::extract::pipeline::{BacklogDrop, Card, ChatAnswer, Outbox, PostResult, Redirected};

use super::desk::CardDesk;

impl<S, T, I, A> CardDesk<S, T, I, A>
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
{
    /// v4 extraction `_apply_rsvp`: this is fed only by
    /// [`crate::extract::pipeline::Outbox::answers`], never proposal-card
    /// approval. The member's ✅/❌ is applied first, then recorded as chat;
    /// an extraction decline enters the durable delivery drain.
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
                    .as_origin(
                        origin
                            .clone()
                            .with_request_id(format!("extract-answer:{}", uuid::Uuid::new_v4())),
                    )
                    .apply_reaction_with_decline(
                        &answer.run_id,
                        user_id,
                        emoji,
                        true,
                        DeclineNoticeContext {
                            channel_id: Some(answer.channel_id.clone()),
                            reference_id: None,
                            display_name: self
                                .directory
                                .display_name(user_id)
                                .unwrap_or_else(|| user_id.clone()),
                        },
                    )
                    .await
                else {
                    continue;
                };
                if !result.value.applied {
                    continue;
                }
                applied += 1;
                if result.retract
                    && let Some(retract) = &self.decline_retraction
                {
                    retract(answer.run_id.clone(), user_id.clone(), now).await;
                }
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
