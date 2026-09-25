//! ✅/❌ on a proposal card (v4 `_handle_proposal_reaction`): approve or
//! reject every proposal on it the member may answer; anyone else is ignored
//! in silence. Approval rules, refusal wording and follow-ups are the
//! scheduler's (`approve_proposal`); this re-renders the cards and posts one
//! "⚠️" notice for changes that no longer apply.

use crate::bot::delivery::AlertSink;
use crate::bot::events::RsvpAnswer;
use crate::bot::transport::DiscordTransport;
use crate::domain::drafts::ProposalStore;
use crate::domain::notify::DeliveryJournal;
use crate::domain::proposals::ProposalCardStore;
use crate::domain::schedule::Notice;
use crate::domain::scheduler::{
    DraftError, IdSource, ProposalApproved, ProposalError, ScheduleStore,
};

use super::desk::CardDesk;

/// What a reaction on a message did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardReaction {
    /// The message is not a proposal card; route it as an RSVP.
    NotACard,
    /// Removed reactions, closed cards or members who may not answer.
    Ignored,
    Rejected {
        proposal_ids: Vec<String>,
    },
    Approved {
        approved: Vec<ProposalApproved>,
        /// v4's ✅-time refusal texts, posted once as a notice.
        problems: Vec<String>,
    },
}

impl CardReaction {
    /// The merges' schedule notices; serve wiring enqueues them (draft-merge
    /// outbox path), as for any merge.
    pub fn notices(&self) -> Vec<Notice> {
        match self {
            Self::Approved { approved, .. } => approved
                .iter()
                .flat_map(|approved| approved.merge.notices.clone())
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Nothing to say: the proposal was answered, closed or is not theirs.
fn silent(error: &ProposalError) -> bool {
    matches!(
        error,
        ProposalError::Unauthorised
            | ProposalError::NotAProposal
            | ProposalError::Draft(
                DraftError::Stale { .. }
                    | DraftError::AlreadyApplied { .. }
                    | DraftError::AlreadyMerged { .. }
                    | DraftError::UnknownDraft(_)
            )
    )
}

impl<S, T, I, A> CardDesk<S, T, I, A>
where
    S: ScheduleStore + ProposalStore + ProposalCardStore + DeliveryJournal + Send + Sync,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
{
    /// Handle a reaction by `user_id` on `message_id`.
    pub async fn on_reaction(
        &self,
        message_id: &str,
        user_id: &str,
        answer: RsvpAnswer,
        added: bool,
    ) -> CardReaction {
        let Ok(cards) = self.store.cards_on_message(message_id).await else {
            return CardReaction::Ignored;
        };
        if cards.is_empty() {
            return CardReaction::NotACard;
        }
        if !added {
            return CardReaction::Ignored;
        }
        let approver = self.authority.approver(user_id);
        let now = self.now();
        let mut service = self.service(now);
        match answer {
            RsvpAnswer::No => {
                let mut rejected = Vec::new();
                for card in &cards {
                    if service
                        .reject_proposal(&card.proposal_id, &approver)
                        .await
                        .is_ok()
                    {
                        rejected.push(card.proposal_id.clone());
                    }
                }
                drop(service);
                if rejected.is_empty() {
                    return CardReaction::Ignored;
                }
                self.refresh(message_id).await;
                CardReaction::Rejected {
                    proposal_ids: rejected,
                }
            }
            RsvpAnswer::Yes => {
                let mut approved: Vec<ProposalApproved> = Vec::new();
                let mut problems: Vec<String> = Vec::new();
                for card in &cards {
                    match service
                        .approve_proposal(
                            &card.proposal_id,
                            &approver,
                            &self.settings.policy,
                            &*self.directory,
                        )
                        .await
                    {
                        Ok(done) => approved.push(done),
                        Err(error) if silent(&error) => {}
                        Err(error) => problems.push(error.to_string()),
                    }
                }
                drop(service);
                if approved.is_empty() && problems.is_empty() {
                    return CardReaction::Ignored;
                }
                if !approved.is_empty() {
                    self.refresh(message_id).await;
                    let superseded: Vec<String> = approved
                        .iter()
                        .flat_map(|done| done.superseded.clone())
                        .collect();
                    self.refresh_proposals(&superseded).await;
                }
                if !problems.is_empty() {
                    let channel = &cards[0].channel_id;
                    self.post_plain(
                        channel,
                        "notice.card.apply.problem",
                        format!("⚠️ {}", problems.join("; ")),
                    )
                    .await;
                }
                CardReaction::Approved { approved, problems }
            }
        }
    }
}
