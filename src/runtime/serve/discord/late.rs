//! The production transport, built once `READY` names the application.
//! Nothing calls Discord before the gateway is ready (registration,
//! interactions, roster paging and the tick all wait for it); a call that
//! did is refused unsent. The one exception is the startup list of the
//! application's emojis, which reads the application id over HTTP itself.
//! Every trait method is delegated, including the ones with a default body
//! (a missed one would be refused `Invalid`).

use std::sync::OnceLock;

use twilight_model::application::command::{Command, CommandOptionChoice};
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::{Channel, Message};
use twilight_model::guild::Member;
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker, UserMarker},
};
use twilight_model::user::CurrentUser;

use crate::bot::transport::{
    ApplicationEmoji, ChannelId, DiscordTransport, HistoryPage, InteractionRef, InteractionReply,
    MessageEdit, MessageId, Outcome, OutgoingMessage, Presence, RejectionKind, TransportConfig,
    TwilightTransport,
};
use crate::runtime::secrets::Redacted;

/// A transport the gateway hands the application id on every `READY`.
pub trait GatewayTransport: DiscordTransport + 'static {
    fn application_ready(&self, _application: Id<ApplicationMarker>) {}
}

#[cfg(any(test, feature = "test-support"))]
impl GatewayTransport for crate::bot::transport::FakeDiscord {}

pub struct LateTransport {
    token: Redacted,
    inner: OnceLock<TwilightTransport>,
}

impl LateTransport {
    pub fn new(token: Redacted) -> Self {
        Self {
            token,
            inner: OnceLock::new(),
        }
    }
}

impl GatewayTransport for LateTransport {
    fn application_ready(&self, application: Id<ApplicationMarker>) {
        self.inner.get_or_init(|| {
            TwilightTransport::new(
                self.token.expose().to_owned(),
                application,
                TransportConfig::default(),
            )
        });
    }
}

macro_rules! delegate {
    ($self:ident, $call:ident($($arg:expr),*)) => {
        match $self.inner.get() {
            Some(inner) => inner.$call($($arg),*).await,
            None => Outcome::DefinitelyRejected(RejectionKind::NotSent),
        }
    };
}

impl DiscordTransport for LateTransport {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        delegate!(self, create_message(channel, message))
    }

    async fn create_flagged_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> Outcome<MessageId> {
        delegate!(self, create_flagged_message(channel, message, flags))
    }

    async fn trigger_typing(&self, channel: ChannelId) -> Outcome<()> {
        delegate!(self, trigger_typing(channel))
    }

    async fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> Outcome<()> {
        delegate!(self, edit_message(channel, message, edit))
    }

    async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        delegate!(self, delete_message(channel, message))
    }

    async fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        delegate!(self, add_own_reaction(channel, message, emoji))
    }

    async fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        delegate!(self, remove_own_reaction(channel, message, emoji))
    }

    async fn message_presence(&self, channel: ChannelId, message: MessageId) -> Outcome<Presence> {
        delegate!(self, message_presence(channel, message))
    }

    async fn message_flags(&self, channel: ChannelId, message: MessageId) -> Outcome<MessageFlags> {
        delegate!(self, message_flags(channel, message))
    }

    async fn respond(&self, interaction: &InteractionRef, reply: &InteractionReply) -> Outcome<()> {
        delegate!(self, respond(interaction, reply))
    }

    async fn autocomplete(
        &self,
        interaction: &InteractionRef,
        choices: &[CommandOptionChoice],
    ) -> Outcome<()> {
        delegate!(self, autocomplete(interaction, choices))
    }

    async fn defer(&self, interaction: &InteractionRef, ephemeral: bool) -> Outcome<()> {
        delegate!(self, defer(interaction, ephemeral))
    }

    async fn defer_update(&self, interaction: &InteractionRef) -> Outcome<()> {
        delegate!(self, defer_update(interaction))
    }

    async fn followup(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        delegate!(self, followup(interaction, reply))
    }

    async fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        delegate!(self, complete_deferred(interaction, reply))
    }

    async fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> Outcome<()> {
        delegate!(self, register_guild_commands(guild, commands))
    }

    async fn reaction_users(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
        kind: twilight_model::channel::message::ReactionType,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Id<UserMarker>>> {
        delegate!(
            self,
            reaction_users(channel, message, emoji, kind, after, limit)
        )
    }

    async fn list_members(
        &self,
        guild: Id<GuildMarker>,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Member>> {
        delegate!(self, list_members(guild, after, limit))
    }

    async fn channel_messages(
        &self,
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
    ) -> Outcome<Vec<Message>> {
        delegate!(self, channel_messages(channel, page, limit))
    }

    async fn guild_channels(&self, guild: Id<GuildMarker>) -> Outcome<Vec<Channel>> {
        delegate!(self, guild_channels(guild))
    }

    async fn current_user(&self) -> Outcome<CurrentUser> {
        delegate!(self, current_user())
    }

    async fn application_emojis(&self) -> Outcome<Vec<ApplicationEmoji>> {
        if let Some(inner) = self.inner.get() {
            return inner.application_emojis().await;
        }
        // Before READY: a throwaway transport for the token's application,
        // so the refusal of every other early call stays in place.
        match TwilightTransport::for_current_application(
            self.token.expose().to_owned(),
            TransportConfig::default(),
        )
        .await
        {
            Outcome::Delivered(early) => early.application_emojis().await,
            Outcome::DefinitelyRejected(kind) => Outcome::DefinitelyRejected(kind),
            Outcome::Ambiguous(kind) => Outcome::Ambiguous(kind),
        }
    }

    async fn create_application_emoji(&self, name: &str, png: &[u8]) -> Outcome<ApplicationEmoji> {
        delegate!(self, create_application_emoji(name, png))
    }
}

#[cfg(test)]
#[path = "late_tests.rs"]
mod tests;
