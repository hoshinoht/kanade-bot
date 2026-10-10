//! Chat input from the gateway: a created guild message becomes the chat
//! driver's [`Asked`] (resolved lists only; nothing is fetched), a deletion
//! cancels its question. `surface.rs` is the driver's Discord side.

mod surface;
#[cfg(test)]
mod surface_tests;

use std::sync::Arc;

use twilight_model::channel::Message;
use twilight_model::id::{
    Id,
    marker::{RoleMarker, UserMarker},
};

pub use surface::DiscordSurface;

use crate::bot::events::{DeletedMessages, GuildMessage};
use crate::bot::guild_cache::GuildCache;
use crate::bot::ids::id_text;
use crate::chat::context::{Parent, QuestionMessage, Reference};
use crate::chat::driver::{Answerer, Asked, ChatDriver, Surface};
use crate::chat::gate::{Author, ChannelDirectory, ChannelInfo, IncomingMessage};

/// Where offered messages go (the chat driver).
pub trait ChatSink: Send + Sync {
    /// `true` when chat took the message (it is then not extraction input).
    fn offer(&self, asked: Asked) -> bool;
    fn deleted(&self, message_ids: &[String]);
}

impl<A: Answerer, S: Surface> ChatSink for ChatDriver<A, S> {
    fn offer(&self, asked: Asked) -> bool {
        ChatDriver::offer(self, asked)
    }

    fn deleted(&self, message_ids: &[String]) {
        ChatDriver::deleted(self, message_ids);
    }
}

/// Is this member staff (admin role, Administrator or the owner)?
pub type StaffFn = Arc<dyn Fn(Id<UserMarker>, &[Id<RoleMarker>]) -> bool + Send + Sync>;

pub struct ChatFeed {
    sink: Arc<dyn ChatSink>,
    cache: Arc<GuildCache>,
    staff: StaffFn,
}

impl std::fmt::Debug for ChatFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatFeed").finish_non_exhaustive()
    }
}

fn parent(message: &Message) -> Parent {
    Parent {
        id: id_text(message.id),
        author_id: Some(id_text(message.author.id)),
        content: Some(message.content.clone()),
        reference: message
            .reference
            .as_ref()
            .and_then(|reference| reference.message_id)
            .map(|id| {
                Box::new(Reference {
                    message_id: Some(id_text(id)),
                    resolved: None,
                })
            }),
    }
}

impl ChatFeed {
    pub fn new(sink: Arc<dyn ChatSink>, cache: Arc<GuildCache>, staff: StaffFn) -> Self {
        Self { sink, cache, staff }
    }

    /// Offer a created message; `true` when chat took it.
    pub fn message(&self, message: &GuildMessage, self_id: Option<Id<UserMarker>>) -> bool {
        self.sink.offer(self.asked(message, self_id))
    }

    pub fn deleted(&self, deleted: &DeletedMessages) {
        let ids: Vec<String> = deleted.message_ids.iter().copied().map(id_text).collect();
        self.sink.deleted(&ids);
    }

    fn asked(&self, guild_message: &GuildMessage, self_id: Option<Id<UserMarker>>) -> Asked {
        let message = &guild_message.message;
        let roles: Vec<Id<RoleMarker>> = message
            .member
            .as_ref()
            .map(|member| member.roles.clone())
            .unwrap_or_default();
        let channel_id = id_text(message.channel_id);
        let channel = ChannelDirectory::channel(&*self.cache, &channel_id)
            .unwrap_or_else(|| ChannelInfo::bare(&channel_id));
        let replied = message.referenced_message.as_deref();
        let reference = message.reference.as_ref().map(|reference| Reference {
            message_id: reference.message_id.map(id_text),
            resolved: replied.map(|parent_message| Box::new(parent(parent_message))),
        });
        Asked {
            message: QuestionMessage {
                id: id_text(message.id),
                author_id: id_text(message.author.id),
                content: message.content.clone(),
                reference,
            },
            origin_id: id_text(guild_message.origin_channel_id),
            channel_id,
            gate: IncomingMessage {
                // Only a guild member's roles count; a webhook or uncached
                // author has none, so the role gate fails closed.
                author: Some(Author {
                    id: id_text(message.author.id),
                    bot: message.author.bot,
                    roles: roles.iter().copied().map(id_text).collect(),
                }),
                guild_id: message.guild_id.map(id_text),
                channel: Some(channel),
                mentions: message
                    .mentions
                    .iter()
                    .map(|mention| id_text(mention.id))
                    .collect(),
                role_mentions: message.mention_roles.iter().copied().map(id_text).collect(),
            },
            replied_author_id: replied.map(|parent| id_text(parent.author.id)),
            bot_user_id: self_id.map(id_text),
            self_role_id: self.cache.self_role().map(id_text),
            is_admin: message.member.is_some() && (self.staff)(message.author.id, &roles),
        }
    }
}
