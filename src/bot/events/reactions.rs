//! ✅/❌ reactions on cards as RSVPs (v4 `_handle_reaction`).
//!
//! Deferred to later slices: proposal-card confirmation, dropping the
//! opposite reaction, and decline/retraction notices. [`ReactionRouter`]
//! returns each run's [`ReactionResult`] so those follow-ups can be driven.

use std::fmt;
use std::future::Future;

use twilight_model::channel::message::EmojiReactionType;
use twilight_model::gateway::GatewayReaction;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker, UserMarker},
};

use crate::bot::ids::id_text;
use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::members::Directory;
use crate::domain::schedule::{
    EMOJI_NO, EMOJI_YES, ReactionResult, RsvpState, ScheduleError, state_for_emoji,
};
use crate::domain::scheduler::{
    Clock, IdSource, ScheduleStore, SchedulerError, SchedulerResult, SchedulerService,
};

/// The two answers a card reaction can give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RsvpAnswer {
    Yes,
    No,
}

impl RsvpAnswer {
    pub fn emoji(self) -> &'static str {
        match self {
            Self::Yes => EMOJI_YES,
            Self::No => EMOJI_NO,
        }
    }
}

/// One RSVP reaction by a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RsvpReaction {
    pub channel_id: Id<ChannelMarker>,
    pub message_id: Id<MessageMarker>,
    pub user_id: Id<UserMarker>,
    pub answer: RsvpAnswer,
    pub added: bool,
}

/// Keep reactions that answer an RSVP. The bot itself, any bot account
/// (from the payload member, else the roster) and other emoji are ignored.
pub fn rsvp_reaction(
    reaction: &GatewayReaction,
    added: bool,
    self_id: Option<Id<UserMarker>>,
    directory: &(impl Directory + ?Sized),
) -> Option<RsvpReaction> {
    if Some(reaction.user_id) == self_id {
        return None;
    }
    if reaction
        .member
        .as_ref()
        .is_some_and(|member| member.user.bot)
    {
        return None;
    }
    if directory
        .member(&id_text(reaction.user_id))
        .is_some_and(|member| member.is_bot)
    {
        return None;
    }
    let EmojiReactionType::Unicode { name } = &reaction.emoji else {
        return None;
    };
    let answer = match state_for_emoji(name)? {
        RsvpState::Yes => RsvpAnswer::Yes,
        RsvpState::No => RsvpAnswer::No,
        RsvpState::Maybe => return None,
    };
    Some(RsvpReaction {
        channel_id: reaction.channel_id,
        message_id: reaction.message_id,
        user_id: reaction.user_id,
        answer,
        added,
    })
}

/// A card-index lookup failure (storage).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupError(pub String);

/// Which runs a posted message is a card for; implemented by storage.
pub trait CardIndex: Send + Sync {
    /// Run ids bound to `message`, empty when it is not a run card.
    fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> impl Future<Output = Result<Vec<String>, LookupError>> + Send;
}

/// Where reaction RSVPs are applied.
pub trait ReactionSink: Send {
    fn apply_reaction(
        &mut self,
        run_id: &str,
        user_id: &str,
        emoji: &str,
        added: bool,
    ) -> impl Future<Output = SchedulerResult<ReactionResult>> + Send;
}

impl<S, I, C> ReactionSink for SchedulerService<S, I, C>
where
    S: ScheduleStore + Send + Sync,
    I: IdSource + Send,
    C: Clock + Send + Sync,
{
    fn apply_reaction(
        &mut self,
        run_id: &str,
        user_id: &str,
        emoji: &str,
        added: bool,
    ) -> impl Future<Output = SchedulerResult<ReactionResult>> + Send {
        // Each reaction is its member's own change, made in Discord.
        self.as_origin(Origin::new(Actor::member(user_id), Surface::Discord))
            .apply_reaction(run_id, user_id, emoji, added)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteError {
    Lookup(LookupError),
    Scheduler(SchedulerError),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lookup(LookupError(detail)) => write!(f, "card lookup failed: {detail}"),
            Self::Scheduler(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RouteError {}

/// Routes RSVP reactions through the card index to the scheduler.
pub struct ReactionRouter<I, S> {
    pub index: I,
    pub sink: S,
}

impl<I: CardIndex, S: ReactionSink> ReactionRouter<I, S> {
    pub fn new(index: I, sink: S) -> Self {
        Self { index, sink }
    }

    /// Apply the reaction to every run on the card, each in its own
    /// transaction as v4 did; runs deleted since posting are skipped.
    ///
    /// # Errors
    /// The lookup failed, or a store failure stopped the remaining runs.
    pub async fn route(
        &mut self,
        reaction: &RsvpReaction,
    ) -> Result<Vec<ReactionResult>, RouteError> {
        let runs = self
            .index
            .runs_for_message(reaction.message_id)
            .await
            .map_err(RouteError::Lookup)?;
        let user_id = id_text(reaction.user_id);
        let mut results = Vec::new();
        for run_id in runs {
            match self
                .sink
                .apply_reaction(&run_id, &user_id, reaction.answer.emoji(), reaction.added)
                .await
            {
                Ok(result) => results.push(result),
                Err(SchedulerError::Schedule(ScheduleError::UnknownRun(_))) => {}
                Err(error) => return Err(RouteError::Scheduler(error)),
            }
        }
        Ok(results)
    }
}
