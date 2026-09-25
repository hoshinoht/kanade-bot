//! A `FakeDiscord` wrapper whose create and delete calls pass through a hook
//! that may act (e.g. change the store) and replace the outcome.

use std::future::Future;
use std::pin::Pin;

use kanade::bot::transport::{
    ChannelId, DiscordTransport, FakeDiscord, InteractionRef, InteractionReply, MessageEdit,
    MessageId, Outcome, OutgoingMessage, Presence,
};
use twilight_model::application::command::Command;
use twilight_model::id::{Id, marker::GuildMarker};

pub type Hooked<'a, T> = Pin<Box<dyn Future<Output = Outcome<T>> + Send + 'a>>;
type Hook<'a, T> = Box<dyn Fn(Outcome<T>) -> Hooked<'a, T> + Send + Sync + 'a>;

pub struct Intercept<'a> {
    pub fake: &'a FakeDiscord,
    pub on_create: Option<Hook<'a, MessageId>>,
    pub on_delete: Option<Hook<'a, ()>>,
}

impl<'a> Intercept<'a> {
    pub fn new(fake: &'a FakeDiscord) -> Self {
        Self {
            fake,
            on_create: None,
            on_delete: None,
        }
    }

    pub fn on_create(
        mut self,
        hook: impl Fn(Outcome<MessageId>) -> Hooked<'a, MessageId> + Send + Sync + 'a,
    ) -> Self {
        self.on_create = Some(Box::new(hook));
        self
    }

    pub fn on_delete(
        mut self,
        hook: impl Fn(Outcome<()>) -> Hooked<'a, ()> + Send + Sync + 'a,
    ) -> Self {
        self.on_delete = Some(Box::new(hook));
        self
    }
}

impl DiscordTransport for Intercept<'_> {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        let outcome = self.fake.create_message(channel, message).await;
        match &self.on_create {
            Some(hook) => hook(outcome).await,
            None => outcome,
        }
    }

    fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.edit_message(channel, message, edit)
    }

    async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        let outcome = self.fake.delete_message(channel, message).await;
        match &self.on_delete {
            Some(hook) => hook(outcome).await,
            None => outcome,
        }
    }

    fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.add_own_reaction(channel, message, emoji)
    }

    fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.remove_own_reaction(channel, message, emoji)
    }

    fn message_presence(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl Future<Output = Outcome<Presence>> + Send {
        self.fake.message_presence(channel, message)
    }

    fn respond(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.respond(interaction, reply)
    }

    fn defer(
        &self,
        interaction: &InteractionRef,
        ephemeral: bool,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.defer(interaction, ephemeral)
    }

    fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.complete_deferred(interaction, reply)
    }

    fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> impl Future<Output = Outcome<()>> + Send {
        self.fake.register_guild_commands(guild, commands)
    }
}
