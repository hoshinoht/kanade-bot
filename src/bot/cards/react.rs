//! ✅/❌ on a proposal card (v4 `_handle_proposal_reaction`): approve or
//! reject every proposal on it the member may answer; anyone else is ignored
//! in silence. Approval rules, refusal wording and follow-ups are the
//! scheduler's (`approve_proposal`); this re-renders the cards and posts one
//! "⚠️" notice for changes that no longer apply. A V2 card's Apply/Reject
//! button ([`CardPress`]) takes exactly the same path; only its presser is
//! told (ephemerally) when they may not answer or the button is stale.

use crate::bot::delivery::{AdminAlert, AlertSink};
use crate::bot::events::RsvpAnswer;
use crate::bot::transport::DiscordTransport;
use crate::domain::drafts::{ProposalSource, ProposalStore};
use crate::domain::notify::DeliveryJournal;
use crate::domain::proposals::{ProposalCardStore, StoredCard};
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

/// Server-side facts for a card whose successful ❌ may ask the original chat
/// author what they want instead. No Discord message text is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardFollowUp {
    pub channel_id: String,
    pub source_ids: Vec<String>,
    pub cards: Vec<FollowUpCard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUpCard {
    pub summary: Option<String>,
    pub bosses: Vec<String>,
    pub participants: Vec<String>,
}

impl CardReaction {
    /// The merges' schedule notices, already written to the notice outbox
    /// by the store with each merge (for reports; never enqueue them again).
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

/// An Apply (`Yes`) or Reject (`No`) press on a V2 proposal card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardPress {
    pub message_id: String,
    /// The proposal the button names (the first on its card).
    pub proposal_id: String,
    pub user_id: String,
    pub answer: RsvpAnswer,
}

/// What a press did, for the presser's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pressed {
    /// Answered as the matching reaction would have been (`Ignored` when
    /// every proposal on it was already closed).
    Answered(CardReaction),
    /// The presser may not answer any proposal on the card: nothing changed.
    NotYours,
    /// The button names no proposal on this message (stale or forged).
    Inactive,
}

/// An answer and how many of its proposals refused the member outright.
struct Answer {
    reaction: CardReaction,
    unauthorised: usize,
}

/// User decision 2026-09-25: a ✅ refused because the run was edited after
/// the card went up (a draft conflict).
pub const CHANGED_SINCE_CARD: &str = "That run was changed after this card went up, so I didn't \
     apply it. Check the run and ask again if it still needs changing.";

/// The line such a card gets, beside "✅ applied by X" and the rest.
pub const OUT_OF_DATE_NOTICE: &str = "⚠️ out of date";

/// What an answer's failure means in the channel.
enum Failure {
    /// Answered, closed or not theirs: say nothing.
    Silent,
    /// A refusal members may read (v4 wording or the stale sentence).
    Public { text: String, stale: bool },
    /// Anything else (store, retries, history): admins only.
    Private,
}

fn failure(error: &ProposalError) -> Failure {
    match error {
        ProposalError::Unauthorised
        | ProposalError::NotAProposal
        | ProposalError::Draft(
            DraftError::Stale { .. }
            | DraftError::AlreadyApplied { .. }
            | DraftError::AlreadyMerged { .. }
            | DraftError::UnknownDraft(_),
        ) => Failure::Silent,
        ProposalError::Refused(_) | ProposalError::NoEffect | ProposalError::Expired => {
            Failure::Public {
                text: error.to_string(),
                stale: false,
            }
        }
        ProposalError::Draft(DraftError::Conflicts(_)) => Failure::Public {
            text: CHANGED_SINCE_CARD.to_owned(),
            stale: true,
        },
        // Reactions never edit; anything else is a fault.
        ProposalError::Draft(_) | ProposalError::EditNotApplicable | ProposalError::EditInPast => {
            Failure::Private
        }
    }
}

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
    /// The source and stored facts for a just-rejected all-chat card. A mixed
    /// source card is never eligible for a chat clarification.
    pub async fn rejection_follow_up(&self, proposal_ids: &[String]) -> Option<CardFollowUp> {
        let cards = self.store.load_cards(proposal_ids).await.ok()?;
        if cards.len() != proposal_ids.len() {
            return None;
        }
        let channel_id = cards.first()?.channel_id.clone();
        if cards.iter().any(|card| card.channel_id != channel_id) {
            return None;
        }
        let mut source_ids = Vec::with_capacity(cards.len());
        let mut facts = Vec::with_capacity(cards.len());
        for card in cards {
            let (_, source) = self.store.load_proposal(&card.proposal_id).await.ok()??;
            if source.source != ProposalSource::Chat {
                return None;
            }
            source_ids.push(source.source_id);
            facts.push(FollowUpCard {
                summary: card.details.summary,
                bosses: card.details.bosses,
                participants: card.details.participants,
            });
        }
        Some(CardFollowUp {
            channel_id,
            source_ids,
            cards: facts,
        })
    }

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
        self.answer_cards(message_id, user_id, answer, &cards).await
    }

    /// A V2 card's Apply/Reject press: the same scheduler calls, refusals,
    /// alerts and refresh as a ✅/❌ by the same member.
    pub async fn on_press(&self, press: &CardPress) -> Pressed {
        let Ok(cards) = self.store.cards_on_message(&press.message_id).await else {
            return Pressed::Inactive;
        };
        if !cards
            .iter()
            .any(|card| card.proposal_id == press.proposal_id)
        {
            return Pressed::Inactive;
        }
        // Only a V2 card has buttons: refresh it as one.
        self.cards.v2.formats.record(&press.message_id, true);
        let answer = self
            .answer(&press.message_id, &press.user_id, press.answer, &cards)
            .await;
        match answer.reaction {
            CardReaction::Ignored if answer.unauthorised > 0 => Pressed::NotYours,
            reaction => Pressed::Answered(reaction),
        }
    }

    /// Live and replayed answers share the same scheduler calls and refreshes;
    /// replay can select one proposal on a grouped card without deciding its siblings.
    pub(super) async fn answer_cards(
        &self,
        message_id: &str,
        user_id: &str,
        answer: RsvpAnswer,
        cards: &[StoredCard],
    ) -> CardReaction {
        self.answer(message_id, user_id, answer, cards)
            .await
            .reaction
    }

    async fn answer(
        &self,
        message_id: &str,
        user_id: &str,
        answer: RsvpAnswer,
        cards: &[StoredCard],
    ) -> Answer {
        let approver = self.authority.approver(user_id);
        let now = self.now();
        let mut service = self.service(now);
        let mut unauthorised = 0;
        let reaction = match answer {
            RsvpAnswer::No => {
                let mut rejected = Vec::new();
                for card in cards {
                    match service.reject_proposal(&card.proposal_id, &approver).await {
                        Ok(_) => rejected.push(card.proposal_id.clone()),
                        // Member-facing refusals (e.g. an expired card) are
                        // routine on ❌; only faults reach the admins.
                        Err(error) => {
                            unauthorised +=
                                usize::from(matches!(error, ProposalError::Unauthorised));
                            if let Failure::Private = failure(&error) {
                                self.alert_failure(&card.proposal_id, &error, now);
                            }
                        }
                    }
                }
                drop(service);
                if rejected.is_empty() {
                    CardReaction::Ignored
                } else {
                    self.refresh(message_id).await;
                    CardReaction::Rejected {
                        proposal_ids: rejected,
                    }
                }
            }
            RsvpAnswer::Yes => {
                let mut approved: Vec<ProposalApproved> = Vec::new();
                let mut problems: Vec<String> = Vec::new();
                let mut stale = false;
                for card in cards {
                    let error = match service
                        .approve_proposal(
                            &card.proposal_id,
                            &approver,
                            &self.settings.policy,
                            &*self.directory,
                        )
                        .await
                    {
                        Ok(done) => {
                            approved.push(done);
                            continue;
                        }
                        Err(error) => error,
                    };
                    match failure(&error) {
                        Failure::Silent => {
                            unauthorised +=
                                usize::from(matches!(error, ProposalError::Unauthorised));
                        }
                        Failure::Public { text, stale: out } => {
                            stale |= out;
                            if !problems.contains(&text) {
                                problems.push(text);
                            }
                        }
                        Failure::Private => self.alert_failure(&card.proposal_id, &error, now),
                    }
                }
                drop(service);
                if approved.is_empty() && problems.is_empty() {
                    return Answer {
                        reaction: CardReaction::Ignored,
                        unauthorised,
                    };
                }
                let extra: &[&str] = if stale { &[OUT_OF_DATE_NOTICE] } else { &[] };
                if !approved.is_empty() || stale {
                    self.refresh_with(message_id, extra).await;
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
        };
        Answer {
            reaction,
            unauthorised,
        }
    }

    fn alert_failure(
        &self,
        proposal_id: &str,
        error: &ProposalError,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        self.raise(
            AdminAlert::CardAnswerFailed {
                proposal_id: proposal_id.to_owned(),
                detail: error.to_string(),
            },
            now,
        );
    }
}
