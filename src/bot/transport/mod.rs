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
use std::sync::Arc;

use twilight_model::application::command::{Command, CommandOptionChoice};
use twilight_model::channel::message::ReactionType;
use twilight_model::channel::message::{AllowedMentions, Component, Embed, MessageFlags};
use twilight_model::channel::{Channel, Message};
use twilight_model::guild::Member;
use twilight_model::id::{
    Id,
    marker::{
        ChannelMarker, EmojiMarker, GuildMarker, InteractionMarker, MessageMarker, UserMarker,
    },
};
use twilight_model::user::CurrentUser;

pub use outcome::{AmbiguousKind, Outcome, RejectionKind, classify_status, codes};
pub use twilight::{MAX_SENDS, TransportConfig, TwilightTransport};

#[cfg(any(test, feature = "test-support"))]
pub use fake::{Call, FakeDiscord, Hold, Op, Step};

/// Discord's page-size bounds; out-of-range limits are refused unsent
/// (`RejectionKind::Invalid`).
pub const MAX_MEMBERS_PAGE: u16 = 1000;
pub const MAX_MESSAGES_PAGE: u16 = 100;
pub const MAX_REACTIONS_PAGE: u16 = 100;

/// The only flags Discord accepts on a created message; any other bit is
/// refused unsent (`RejectionKind::Invalid`).
pub const CREATE_FLAGS: MessageFlags = MessageFlags::SUPPRESS_EMBEDS
    .union(MessageFlags::SUPPRESS_NOTIFICATIONS)
    .union(MessageFlags::IS_COMPONENTS_V2);

/// A post that notifies nobody (Discord's `@silent`).
pub const SILENT: MessageFlags = MessageFlags::SUPPRESS_NOTIFICATIONS;

/// A Components V2 message (`1 << 15`). Transports set it for every message
/// that carries components (only V2 layouts are sent; legacy component rows
/// never are) and refuse it unsent without them. Discord can add it on an
/// edit but never remove it.
pub const COMPONENTS_V2: MessageFlags = MessageFlags::IS_COMPONENTS_V2;

/// Whether a body is sendable as far as Components V2 goes: one with
/// components has no content and no embeds (Discord answers 400 otherwise),
/// and an explicit V2 flag needs components.
pub fn v2_body_valid(
    content: Option<&str>,
    embeds: &[Embed],
    components: &[Component],
    flags: MessageFlags,
) -> bool {
    if components.is_empty() {
        return !flags.contains(COMPONENTS_V2);
    }
    content.is_none_or(str::is_empty) && embeds.is_empty()
}

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
    /// Files posted with the message (multipart); an embed refers to one as
    /// `attachment://<filename>`.
    pub attachments: Vec<Upload>,
    /// A Components V2 layout; non-empty makes the post V2 (see
    /// [`COMPONENTS_V2`]), so `content` and `embeds` must then be empty.
    pub components: Vec<Component>,
}

impl OutgoingMessage {
    /// A Components V2 post: the layout alone, no content or embeds.
    pub fn v2(components: Vec<Component>, allowed_mentions: AllowedMentions) -> Self {
        Self {
            content: None,
            embeds: Vec::new(),
            allowed_mentions,
            reply_to: None,
            attachments: Vec::new(),
            components,
        }
    }
}

/// One uploaded file. Filenames are ASCII alphanumerics, `.`, `-` and `_`
/// (Discord's rule; Twilight refuses others unsent as `Invalid`).
#[derive(Clone, PartialEq, Eq)]
pub struct Upload {
    pub filename: String,
    pub bytes: Arc<[u8]>,
}

impl fmt::Debug for Upload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upload")
            .field("filename", &self.filename)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Which page of a channel's history to read. Discord returns every page
/// newest first; `After` holds the oldest messages after the id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryPage {
    Latest,
    Before(MessageId),
    After(MessageId),
}

/// An edit; `None` fields are left unchanged. Attachments are never sent
/// with an edit, so the message keeps the files it was posted with and a
/// re-rendered embed's `attachment://` references keep resolving.
///
/// `components: Some(non-empty)` makes it a Components V2 edit: the V2 flag
/// is set and the content and embeds are cleared (`null` and `[]`), which
/// also converts a legacy message. `content` and `embeds` must then be
/// `None` or empty. A legacy edit of a V2 message is refused by Discord.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageEdit {
    pub content: Option<String>,
    pub embeds: Option<Vec<Embed>>,
    pub allowed_mentions: AllowedMentions,
    pub components: Option<Vec<Component>>,
}

impl MessageEdit {
    /// A Components V2 edit (see the type's docs).
    pub fn v2(components: Vec<Component>, allowed_mentions: AllowedMentions) -> Self {
        Self {
            content: None,
            embeds: None,
            allowed_mentions,
            components: Some(components),
        }
    }

    /// The V2 layout this edit sends, if it is a V2 edit.
    pub fn v2_components(&self) -> Option<&[Component]> {
        self.components
            .as_deref()
            .filter(|components| !components.is_empty())
    }

    /// See [`v2_body_valid`]; `components: Some([])` is refused too.
    pub fn valid(&self) -> bool {
        match &self.components {
            None => true,
            Some(components) => v2_body_valid(
                self.content.as_deref(),
                self.embeds.as_deref().unwrap_or_default(),
                components,
                COMPONENTS_V2,
            ),
        }
    }
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
    /// A Components V2 layout; non-empty makes the reply V2, so `content`
    /// and `embeds` must then be empty.
    pub components: Vec<Component>,
}

impl InteractionReply {
    pub fn ephemeral(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ephemeral: true,
            embeds: Vec::new(),
            components: Vec::new(),
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

    /// A Components V2 reply with this visibility.
    pub fn v2(components: Vec<Component>, ephemeral: bool) -> Self {
        Self {
            components,
            ephemeral,
            ..Self::ephemeral("")
        }
    }

    /// The message flags this reply is sent with.
    pub fn flags(&self) -> MessageFlags {
        let mut flags = MessageFlags::empty();
        if self.ephemeral {
            flags |= MessageFlags::EPHEMERAL;
        }
        if !self.components.is_empty() {
            flags |= COMPONENTS_V2;
        }
        flags
    }

    /// See [`v2_body_valid`].
    pub fn valid(&self) -> bool {
        v2_body_valid(
            Some(&self.content),
            &self.embeds,
            &self.components,
            MessageFlags::empty(),
        )
    }
}

pub type ChannelId = Id<ChannelMarker>;
pub type MessageId = Id<MessageMarker>;

/// One of the application's own emojis (usable in any guild the bot is in).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationEmoji {
    pub id: Id<EmojiMarker>,
    pub name: String,
    pub animated: bool,
}

impl ApplicationEmoji {
    /// The message markup that shows it: `<:name:id>` (`<a:name:id>` animated).
    pub fn markup(&self) -> String {
        let animated = if self.animated { "a" } else { "" };
        format!("<{animated}:{}:{}>", self.name, self.id)
    }
}

/// Discord operations with classified outcomes.
///
/// Reactions are the bot's own and unicode-only.
pub trait DiscordTransport: Send + Sync {
    fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> impl Future<Output = Outcome<MessageId>> + Send;

    /// [`Self::create_message`] with message flags within [`CREATE_FLAGS`]
    /// (e.g. [`SILENT`]). Transports that cannot carry flags keep the
    /// default and refuse unsent, so a silent post never goes out pinging.
    fn create_flagged_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> impl Future<Output = Outcome<MessageId>> + Send {
        let _ = (channel, message, flags);
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

    /// Show the bot as typing in `channel` (Discord clears it after ~10 s or
    /// at the bot's next message). Callers fire and forget: a failure is
    /// never retried. Test doubles that never type may keep the default.
    fn trigger_typing(&self, channel: ChannelId) -> impl Future<Output = Outcome<()>> + Send {
        let _ = channel;
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

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

    /// One page of reactors, after a user id. No new gateway intent is needed.
    fn reaction_users(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
        kind: ReactionType,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> impl Future<Output = Outcome<Vec<Id<UserMarker>>>> + Send {
        let _ = (channel, message, emoji, kind, after, limit);
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

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

    /// A posted message's flags (`GET` the message), to tell a Components V2
    /// post from a legacy one. Test doubles that never ask may keep the
    /// default.
    fn message_flags(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl Future<Output = Outcome<MessageFlags>> + Send {
        let _ = (channel, message);
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

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

    /// Acknowledge a button press without a new message (response type 6,
    /// deferred update); answer later with [`Self::followup`]. Test doubles
    /// that never see a button may keep the default.
    fn defer_update(
        &self,
        interaction: &InteractionRef,
    ) -> impl Future<Output = Outcome<()>> + Send {
        let _ = interaction;
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

    /// Fill in a deferred response (edits the original response).
    fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send;

    /// A further message for an answered interaction (webhook follow-up),
    /// with the reply's visibility; it mentions nobody. Test doubles that
    /// never see one may keep the default.
    fn followup(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl Future<Output = Outcome<()>> + Send {
        let _ = (interaction, reply);
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

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

    /// The bot's own user (`GET /users/@me`: the banner `READY` may omit).
    /// Test doubles that never refresh identity art may keep the default.
    fn current_user(&self) -> impl Future<Output = Outcome<CurrentUser>> + Send {
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

    /// The application's own emojis. Test doubles that never list them may
    /// keep the default.
    fn application_emojis(&self) -> impl Future<Output = Outcome<Vec<ApplicationEmoji>>> + Send {
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }

    /// Upload one application emoji from a PNG (Discord: at most 128×128
    /// and 256 KiB). An ambiguous upload may have landed: list before
    /// trying again.
    fn create_application_emoji(
        &self,
        name: &str,
        png: &[u8],
    ) -> impl Future<Output = Outcome<ApplicationEmoji>> + Send {
        let _ = (name, png);
        async { Outcome::DefinitelyRejected(RejectionKind::Invalid) }
    }
}
