//! ✅/❌ reactions on cards as RSVPs (v4 `_handle_reaction`).
//!
//! Deferred to later slices: proposal-card confirmation, dropping the
//! opposite reaction, and decline/retraction notices. [`ReactionRouter`]
//! returns each run's [`ReactionResult`] so those follow-ups can be driven.

use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};
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
use crate::domain::scheduler::{DeclineNoticeContext, DeclineRsvpResult};

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

/// One remotely readable message that still maps to a run. `evidence` is the
/// narrow, fresh bound-card subset; every row remains mapped for removal
/// guards and conflict detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayCard {
    pub channel_id: Option<String>,
    pub message_id: String,
    pub evidence: bool,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// All message mappings for one candidate run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayRunCards {
    pub run_id: String,
    pub cards: Vec<ReplayCard>,
}

/// The journal view needed for bounded RSVP recovery.
pub trait ReplayCards: Send + Sync {
    /// Returns only runs with at least one fresh evidence card, but each
    /// returned run carries every mapped message needed by the removal guard.
    fn replay_cards(
        &self,
        not_before: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<ReplayRunCards>, LookupError>> + Send;
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
    S: ScheduleStore + crate::domain::notify::DeclineNoticeStore + Send + Sync,
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
            .runs_for_message(reaction.message_id)
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
                Err(SchedulerError::Schedule(ScheduleError::UnknownRun(_))) => {}
                Err(error) => return Err(RouteError::Scheduler(error)),
            }
        }
        Ok(results)
    }
}
