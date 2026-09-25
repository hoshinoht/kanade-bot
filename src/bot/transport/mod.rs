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

use twilight_model::application::command::Command;
use twilight_model::channel::message::{AllowedMentions, Embed};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, InteractionMarker, MessageMarker},
};

pub use outcome::{AmbiguousKind, Outcome, RejectionKind, classify_status, codes};
pub use twilight::{MAX_SENDS, TransportConfig, TwilightTransport};

#[cfg(any(test, feature = "test-support"))]
pub use fake::{Call, FakeDiscord, Op, Step};

/// A new message. `allowed_mentions` is required so no post can fall back to
/// Discord's parse-everything default; build it with [`crate::bot::mentions`].
#[derive(Clone, Debug, PartialEq)]
pub struct OutgoingMessage {
    pub content: Option<String>,
    pub embeds: Vec<Embed>,
    pub allowed_mentions: AllowedMentions,
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
}

impl InteractionReply {
    pub fn ephemeral(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ephemeral: true,
        }
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

    /// Replace the guild's command set (an idempotent bulk overwrite).
    fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> impl Future<Output = Outcome<()>> + Send;
}
