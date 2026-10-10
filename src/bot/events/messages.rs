//! Guild messages with the channel they group under (v4 `origin_ids`).

use std::fmt;

use twilight_model::channel::Message;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker},
};

use crate::bot::guild_cache::GuildCache;

/// A created or edited message. `Debug` omits the content.
#[derive(Clone, PartialEq)]
pub struct GuildMessage {
    pub message: Message,
    /// A thread's parent channel, else the message's own channel.
    pub origin_channel_id: Id<ChannelMarker>,
    /// The thread it was posted in, if any.
    pub thread_id: Option<Id<ChannelMarker>>,
}

impl GuildMessage {
    pub(super) fn new(message: Message, cache: &GuildCache) -> Self {
        let (origin_channel_id, thread_id) = cache.origin(message.channel_id);
        Self {
            message,
            origin_channel_id,
            thread_id,
        }
    }
}

impl fmt::Debug for GuildMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuildMessage")
            .field("id", &self.message.id)
            .field("channel_id", &self.message.channel_id)
            .field("origin_channel_id", &self.origin_channel_id)
            .field("thread_id", &self.thread_id)
            .field("author_id", &self.message.author.id)
            .field("content_len", &self.message.content.len())
            .finish_non_exhaustive()
    }
}

/// One `MESSAGE_DELETE` (a single id) or `MESSAGE_DELETE_BULK`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedMessages {
    pub channel_id: Id<ChannelMarker>,
    pub origin_channel_id: Id<ChannelMarker>,
    pub thread_id: Option<Id<ChannelMarker>>,
    pub message_ids: Vec<Id<MessageMarker>>,
}

impl DeletedMessages {
    pub(super) fn new(
        channel_id: Id<ChannelMarker>,
        message_ids: Vec<Id<MessageMarker>>,
        cache: &GuildCache,
    ) -> Self {
        let (origin_channel_id, thread_id) = cache.origin(channel_id);
        Self {
            channel_id,
            origin_channel_id,
            thread_id,
            message_ids,
        }
    }
}
