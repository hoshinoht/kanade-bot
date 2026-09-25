//! The Discord operations the domain needs, behind one seam.
//!
//! Implementations never retry a request whose effect may have happened; they
//! classify it (see [`Outcome`]) and leave any retry decision to the journal.

mod outcome;
mod twilight;

#[cfg(any(test, feature = "test-support"))]
mod fake;

use std::fmt;
use std::future::Future;

use twilight_model::application::command::{Command, CommandOptionChoice};
use twilight_model::channel::message::{AllowedMentions, Embed};
use twilight_model::channel::{Channel, Message};
use twilight_model::guild::Member;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, InteractionMarker, MessageMarker, UserMarker},
};

pub use outcome::{AmbiguousKind, Outcome, RejectionKind, classify_status, codes};
pub use twilight::{MAX_SENDS, TransportConfig, TwilightTransport};

#[cfg(any(test, feature = "test-support"))]
pub use fake::{Call, FakeDiscord, Op, Step};

/// Discord's page-size bounds; out-of-range limits are refused unsent
/// (`RejectionKind::Invalid`).
pub const MAX_MEMBERS_PAGE: u16 = 1000;
pub const MAX_MESSAGES_PAGE: u16 = 100;

/// A new message. `allowed_mentions` is required so no post can fall back to
/// Discord's parse-everything default; build it with [`crate::bot::mentions`].
#[derive(Clone, Debug, PartialEq)]
pub struct OutgoingMessage {
    pub content: Option<String>,
    pub embeds: Vec<Embed>,
    pub allowed_mentions: AllowedMentions,
    /// Reply to this message in the same channel. Sent with
    /// `fail_if_not_exists = false` (a deleted target still posts); whether
    /// the author is pinged stays with `allowed_mentions.replied_user`.
    pub reply_to: Option<MessageId>,
}

/// Which page of a channel's history to read. Discord returns every page
/// newest first; `After` holds the oldest messages after the id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryPage {
    Latest,
    Before(MessageId),
    After(MessageId),
}

/// An edit; `None` fields are left unchanged.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageEdit {
    pub content: Option<String>,
    pub embeds: Option<Vec<Embed>>,
    pub allowed_mentions: AllowedMentions,
}

/// Whether a message still exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Present,
    /// Discord answered Unknown Message: deletion is confirmed.
    Absent,
}

/// The id and short-lived token that answer one interaction.
#[derive(Clone, PartialEq, Eq)]
pub struct InteractionRef {
    pub id: Id<InteractionMarker>,
    token: String,
}

impl InteractionRef {
    pub fn new(id: Id<InteractionMarker>, token: String) -> Self {
        Self { id, token }
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for InteractionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InteractionRef")
            .field("id", &self.id)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// An immediate interaction reply. It always mentions nobody.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteractionReply {
    pub content: String,
    pub ephemeral: bool,
    /// Mentions inside embeds never notify anyone.
    pub embeds: Vec<Embed>,
}

impl InteractionReply {
    pub fn ephemeral(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ephemeral: true,
            embeds: Vec::new(),
        }
    }

    /// A reply everyone in the channel sees (still mentioning nobody).
    pub fn public(content: impl Into<String>) -> Self {
        Self {
            ephemeral: false,
            ..Self::ephemeral(content)
        }
    }

    #[must_use]
    pub fn with_embed(mut self, embed: Embed) -> Self {
        self.embeds.push(embed);
        self
    }
}

pub type ChannelId = Id<ChannelMarker>;
pub type MessageId = Id<MessageMarker>;

/// Discord operations with classified outcomes.
///
/// Reactions are the bot's own and unicode-only.
pub trait DiscordTransport: Send + Sync {
    fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> impl Future<Output = Outcome<MessageId>> + Send;

    fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> impl Future<Output = Outcome<()>> + Send;

    fn delete_message(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl Future<Output = Outcome<()>> + Send;

    fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl Future<Output = Outcome<()>> + Send;

    fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl Future<Output = Outcome<()>> + Send;

    /// Fetch a message to confirm it exists or is gone.
    fn message_presence(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl Future<Output = Outcome<Presence>> + Send;

    /// The initial interaction response; must land within Discord's 3 s.
    fn respond(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send;

    /// Acknowledge now and answer later (response type 5). Visibility is
    /// fixed here; [`Self::complete_deferred`] cannot change it.
    fn defer(
        &self,
        interaction: &InteractionRef,
        ephemeral: bool,
    ) -> impl Future<Output = Outcome<()>> + Send;

    /// Fill in a deferred response (edits the original response).
    fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send;

    /// Answer an autocomplete interaction (response type 8) with at most 25
    /// choices. Test doubles that never see autocomplete may keep the default.
    fn autocomplete(
        &self,
        interaction: &InteractionRef,
        choices: &[CommandOptionChoice],
    ) -> impl Future<Output = Outcome<()>> + Send {
        let _ = (interaction, choices);
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

    /// Replace the guild's command set (an idempotent bulk overwrite). There
    /// is deliberately no global-command operation.
    fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> impl Future<Output = Outcome<()>> + Send;

    /// One page of guild members, ordered by user id, after `after`
    /// (1..=[`MAX_MEMBERS_PAGE`]). Needs `GUILD_MEMBERS`.
    fn list_members(
        &self,
        guild: Id<GuildMarker>,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> impl Future<Output = Outcome<Vec<Member>>> + Send;

    /// One page of a channel's (or thread's) history, newest first
    /// (1..=[`MAX_MESSAGES_PAGE`]). Needs View Channel + Read Message History.
    fn channel_messages(
        &self,
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
    ) -> impl Future<Output = Outcome<Vec<Message>>> + Send;

    /// The guild's channels (threads excluded, as Discord lists them).
    fn guild_channels(
        &self,
        guild: Id<GuildMarker>,
    ) -> impl Future<Output = Outcome<Vec<Channel>>> + Send;
}
