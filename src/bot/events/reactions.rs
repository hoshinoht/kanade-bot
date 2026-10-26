//! ✅/❌ reactions on cards as RSVPs (v4 `_handle_reaction`).
//!
//! Deferred to later slices: proposal-card confirmation, dropping the
//! opposite reaction, and decline/retraction notices. [`ReactionRouter`]
//! returns each run's [`ReactionResult`] so those follow-ups can be driven.

use std::future::Future;

use twilight_model::channel::message::EmojiReactionType;
use twilight_model::gateway::GatewayReaction;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker, UserMarker},
};

use crate::app::ids::id_text;
use kanade_domain::history::{Actor, Origin, Surface};
use kanade_domain::members::Directory;
use kanade_domain::notify::{CardIndex, LookupError};
use kanade_domain::schedule::{
    EMOJI_NO, EMOJI_YES, ReactionResult, RsvpState, ScheduleError, state_for_emoji,
};
use kanade_domain::scheduler::{
    Clock, IdSource, ScheduleStore, SchedulerError, SchedulerResult, SchedulerService,
};
use kanade_domain::scheduler::{DeclineNoticeContext, DeclineRsvpResult};

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
    /// Captured from the gateway event while it is available; removals fall
    /// back to the synced roster for the mandatory candidate display name.
    pub display_name: String,
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
    let user_id = id_text(reaction.user_id);
    let display_name = reaction
        .member
        .as_ref()
        .and_then(|member| {
            member
                .nick
                .clone()
                .or_else(|| member.user.global_name.clone())
                .or_else(|| Some(member.user.name.clone()))
        })
        .or_else(|| directory.display_name(&user_id))
        .unwrap_or_else(|| user_id.clone());
    Some(RsvpReaction {
        channel_id: reaction.channel_id,
        message_id: reaction.message_id,
        user_id: reaction.user_id,
        answer,
        added,
        display_name,
    })
}

/// Where reaction RSVPs are applied.
pub trait ReactionSink: Send {
    fn apply_reaction(
        &mut self,
        run_id: &str,
        user_id: &str,
        emoji: &str,
        added: bool,
        decline: DeclineNoticeContext,
    ) -> impl Future<Output = SchedulerResult<DeclineRsvpResult<ReactionResult>>> + Send;
}

impl<S, I, C> ReactionSink for SchedulerService<S, I, C>
where
    S: ScheduleStore + kanade_domain::notify::DeclineNoticeStore + Send + Sync,
    I: IdSource + Send,
    C: Clock + Send + Sync,
{
    fn apply_reaction(
        &mut self,
        run_id: &str,
        user_id: &str,
        emoji: &str,
        added: bool,
        decline: DeclineNoticeContext,
    ) -> impl Future<Output = SchedulerResult<DeclineRsvpResult<ReactionResult>>> + Send {
        // Each reaction is its member's own change, made in Discord.
        self.as_origin(Origin::new(Actor::member(user_id), Surface::Discord))
            .apply_reaction_with_decline(run_id, user_id, emoji, added, decline)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("card lookup failed: {}", .0.0)]
    Lookup(LookupError),
    #[error("{0}")]
    Scheduler(SchedulerError),
}

/// Routes RSVP reactions through the card index to the scheduler.
pub struct ReactionRouter<I, S> {
    pub index: I,
    pub sink: S,
}

/// One committed card RSVP plus whether the caller should invoke S2 now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedReaction {
    pub result: ReactionResult,
    pub retract: bool,
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
        Ok(self
            .route_declines(reaction)
            .await?
            .into_iter()
            .map(|routed| routed.result)
            .collect())
    }

    /// Route the reaction and retain its committed retraction decision for
    /// the post-commit delivery adapter.
    pub async fn route_declines(
        &mut self,
        reaction: &RsvpReaction,
    ) -> Result<Vec<RoutedReaction>, RouteError> {
        let runs = self
            .index
            .runs_for_message(&id_text(reaction.message_id))
            .await
            .map_err(RouteError::Lookup)?;
        let user_id = id_text(reaction.user_id);
        let mut results = Vec::new();
        for run_id in runs {
            match self
                .sink
                .apply_reaction(
                    &run_id,
                    &user_id,
                    reaction.answer.emoji(),
                    reaction.added,
                    DeclineNoticeContext {
                        channel_id: Some(id_text(reaction.channel_id)),
                        reference_id: Some(id_text(reaction.message_id)),
                        display_name: reaction.display_name.clone(),
                    },
                )
                .await
            {
                Ok(result) => results.push(RoutedReaction {
                    retract: result.retract,
                    result: result.value,
                }),
                // A deleted run, or one past its end (frozen until settled):
                // the answer is not recorded and the card's other runs still are.
                Err(SchedulerError::Schedule(
                    ScheduleError::UnknownRun(_) | ScheduleError::RunEnded { .. },
                )) => {}
                Err(error) => return Err(RouteError::Scheduler(error)),
            }
        }
        Ok(results)
    }
}
