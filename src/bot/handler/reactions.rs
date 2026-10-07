//! The one sequential reaction worker: a ✅/❌ on a proposal card goes to the
//! [`CardDesk`]; on any other message it is an RSVP through the card index.
//! Sequential so a member's add and remove are applied in order. Apply /
//! Reject presses on V2 proposal cards ([`PressJob`]) are answered here
//! too, in the same order as reactions and with the same follow-ups.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};

use crate::api::auth::Clock;
use crate::api::state::DeclineRetraction;
use crate::bot::cards::{CardDesk, CardPress, CardReaction, Pressed, ReplayLive, ReplayReport};
use crate::bot::commands::CardPresses;
use crate::bot::delivery::AlertSink;
use crate::bot::events::{CardIndex, ReactionRouter, ReactionSink, RsvpReaction};
use crate::bot::gateway::ConnectionStatus;
use crate::bot::ids::id_text;
use crate::bot::rsvp_replay::RsvpReplay;
use crate::bot::transport::DiscordTransport;
use crate::chat::driver::{FollowUpCard, FollowUpRequest, RejectionFollowUp};
use crate::domain::drafts::ProposalStore;
use crate::domain::history::{BlameIndex, ChangeHistory};
use crate::domain::notify::DeliveryJournal;
use crate::domain::proposals::ProposalCardStore;
use crate::domain::scheduler::{IdSource, ScheduleStore};
use crate::runtime::logging;
use twilight_model::id::{Id, marker::UserMarker};

/// What one reaction did, for tests and logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reacted {
    Card(CardReaction),
    /// RSVP results, one per run on the card.
    Rsvp(usize),
    Failed,
}

/// One Apply/Reject press for the worker, and where its result goes.
#[derive(Debug)]
pub struct PressJob {
    pub press: CardPress,
    pub done: oneshot::Sender<Pressed>,
}

/// The dispatcher's way into the worker's press queue: `None` once the
/// worker is gone.
pub fn press_port(jobs: mpsc::UnboundedSender<PressJob>) -> CardPresses {
    Arc::new(move |press| {
        let jobs = jobs.clone();
        Box::pin(async move {
            let (done, result) = oneshot::channel();
            jobs.send(PressJob { press, done }).ok()?;
            result.await.ok()
        })
    })
}

pub struct Reactions<S, T, I, A, X, K> {
    /// Shared with extraction's card outbox.
    pub desk: Arc<CardDesk<S, T, I, A>>,
    pub rsvp: ReactionRouter<X, K>,
    pub rsvp_replay: Option<RsvpReplay<S, T, A>>,
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
        + ChangeHistory
        + BlameIndex
        + crate::bot::events::ReplayCards
        + crate::domain::notify::DeclineNoticeStore
        + ProposalStore
        + ProposalCardStore
        + DeliveryJournal
        + Send
        + Sync
        + 'static,
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
            if let CardReaction::Rejected { proposal_ids } = &card {
                self.rejected(
                    &id_text(reaction.message_id),
                    &id_text(reaction.user_id),
                    proposal_ids,
                )
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

    /// An Apply/Reject press, answered as ✅/❌ by the presser would be.
    pub async fn press(&mut self, press: &CardPress) -> Pressed {
        let pressed = self.desk.on_press(press).await;
        if let Pressed::Answered(CardReaction::Rejected { proposal_ids }) = &pressed {
            self.rejected(&press.message_id, &press.user_id, proposal_ids)
                .await;
        }
        pressed
    }

    /// A successful ❌ may ask an all-chat card's author what they want
    /// instead (the chat driver's scope checks decide).
    async fn rejected(&mut self, message_id: &str, user_id: &str, proposal_ids: &[String]) {
        if let (Some(follow_up), Some(facts)) = (
            &self.follow_up,
            self.desk.rejection_follow_up(proposal_ids).await,
        ) {
            follow_up
                .rejected(FollowUpRequest {
                    card_message_id: message_id.to_owned(),
                    channel_id: facts.channel_id,
                    reactor_id: user_id.to_owned(),
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
    }

    /// Apply reactions until every sender is dropped.
    pub async fn run(mut self, mut reactions: mpsc::UnboundedReceiver<RsvpReaction>) {
        while let Some(reaction) = reactions.recv().await {
            self.apply(&reaction).await;
        }
    }

    /// Queued live reactions retain their ordinary follow-ups, even when HTTP
    /// observes them during replay. Touched cards get a bounded fresh HTTP read.
    pub async fn replay(
        &mut self,
        self_id: Id<UserMarker>,
        current: impl Fn() -> bool + Send + Sync,
        reactions: &mut mpsc::UnboundedReceiver<RsvpReaction>,
    ) -> ReplayReport {
        let desk = Arc::clone(&self.desk);
        let proposal = desk
            .replay_reactions_with(self_id, &current, &mut (&mut *self, &mut *reactions))
            .await;
        let Some(mut rsvp) = self.rsvp_replay.take() else {
            return proposal;
        };
        let _ = rsvp
            .replay(self_id, &current, &mut (&mut *self, &mut *reactions))
            .await;
        self.rsvp_replay = Some(rsvp);
        proposal
    }

    /// Replay and live events share one worker, so neither two passes nor a
    /// queued gateway decision can race. Watch keeps only the latest READY.
    /// Presses wait for a replay pass like reactions do; once stopping,
    /// queued presses are dropped (their interaction was acknowledged).
    pub(crate) async fn run_with_replay(
        mut self,
        mut reactions: mpsc::UnboundedReceiver<RsvpReaction>,
        mut presses: mpsc::UnboundedReceiver<PressJob>,
        connection: ConnectionStatus,
        mut stop: watch::Receiver<bool>,
    ) {
        let mut replays = connection.reaction_replays();
        loop {
            let request = *replays.borrow_and_update();
            if let Some(request) = request
                && !*stop.borrow()
                && connection.reaction_replay_allowed(request.generation)
            {
                self.replay(
                    request.self_id,
                    || !*stop.borrow() && connection.reaction_replay_allowed(request.generation),
                    &mut reactions,
                )
                .await;
            }
            loop {
                tokio::select! {
                    biased;
                    _ = stop.changed() => {
                        self.run(reactions).await;
                        return;
                    }
                    _ = replays.changed() => break,
                    reaction = reactions.recv() => {
                        let Some(reaction) = reaction else { return; };
                        self.apply(&reaction).await;
                    }
                    Some(job) = presses.recv() => {
                        let pressed = self.press(&job.press).await;
                        let _ = job.done.send(pressed);
                    }
                }
            }
        }
    }
}

impl<S, T, I, A, X, K> ReplayLive
    for (
        &mut Reactions<S, T, I, A, X, K>,
        &mut mpsc::UnboundedReceiver<RsvpReaction>,
    )
where
    S: ScheduleStore
        + ChangeHistory
        + BlameIndex
        + crate::bot::events::ReplayCards
        + crate::domain::notify::DeclineNoticeStore
        + ProposalStore
        + ProposalCardStore
        + DeliveryJournal
        + Send
        + Sync
        + 'static,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
    X: CardIndex,
    K: ReactionSink,
{
    async fn drain(&mut self) -> BTreeSet<String> {
        let mut touched = BTreeSet::new();
        // Drain a snapshot, not an unbounded stream that could starve replay.
        for _ in 0..self.1.len() {
            let Ok(reaction) = self.1.try_recv() else {
                break;
            };
            touched.insert(id_text(reaction.message_id));
            self.0.apply(&reaction).await;
        }
        touched
    }
}
