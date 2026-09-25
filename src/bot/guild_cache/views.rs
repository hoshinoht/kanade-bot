//! The cache behind the traits its readers already define.

use std::collections::BTreeSet;

use twilight_model::channel::ChannelType;
use twilight_model::id::{Id, marker::ChannelMarker};

use super::{CachedChannel, GuildCache};
use crate::api::state::{ChannelEntry, ChannelList};
use crate::bot::ids::{id_text, parse_id};
use crate::chat::gate::{ChannelDirectory, ChannelInfo};
use crate::domain::notify::ChannelDirectory as PostDirectory;

/// The extractor's watched channels and categories (v4 `is_watched`): a
/// channel is watched when listed, under a listed category, or a thread of
/// either.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchList {
    pub channel_ids: BTreeSet<Id<ChannelMarker>>,
    pub category_ids: BTreeSet<Id<ChannelMarker>>,
}

impl WatchList {
    fn matches(&self, channel: &CachedChannel) -> bool {
        self.channel_ids.contains(&channel.id)
            || channel
                .parent_id
                .is_some_and(|parent| !channel.is_thread() && self.category_ids.contains(&parent))
    }
}

impl GuildCache {
    /// v4 `is_watched` over the live cache; unknown channels are not watched.
    pub fn is_watched(&self, id: Id<ChannelMarker>) -> bool {
        let state = self.read();
        let Some(channel) = state.channels.get(&id) else {
            return false;
        };
        if state.watch.matches(channel) {
            return true;
        }
        channel
            .is_thread()
            .then_some(channel.parent_id)
            .flatten()
            .and_then(|parent| state.channels.get(&parent))
            .is_some_and(|parent| state.watch.matches(parent))
    }

    /// A channel's name for prompts and logs, by id text; `None` if unknown.
    pub fn channel_name(&self, id: &str) -> Option<String> {
        self.name(parse_id(id)?)
    }
}

/// Text and announcement channels (discord.py `text_channels`), by position.
impl ChannelList for GuildCache {
    fn channels(&self) -> Vec<ChannelEntry> {
        let mut text: Vec<CachedChannel> = GuildCache::channels(self)
            .into_iter()
            .filter(|channel| {
                matches!(
                    channel.kind,
                    ChannelType::GuildText | ChannelType::GuildAnnouncement
                )
            })
            .collect();
        text.sort_by_key(|channel| (channel.position.unwrap_or(i32::MAX), channel.id));
        text.into_iter()
            .map(|channel| ChannelEntry {
                id: id_text(channel.id),
                name: format!("#{}", channel.name.as_deref().unwrap_or_default()),
                watched: self.is_watched(channel.id),
            })
            .collect()
    }
}

impl ChannelDirectory for GuildCache {
    fn channel(&self, id: &str) -> Option<ChannelInfo> {
        let channel = GuildCache::channel(self, parse_id(id)?)?;
        let (category_id, parent_id) = if channel.is_thread() {
            (None, channel.parent_id)
        } else {
            (channel.parent_id, None)
        };
        Some(ChannelInfo {
            id: id_text(channel.id),
            name: channel.name,
            category_id: category_id.map(id_text),
            parent_id: parent_id.map(id_text),
        })
    }
}

impl PostDirectory for GuildCache {
    fn is_reachable(&self, channel_id: &str) -> bool {
        parse_id(channel_id).is_some_and(|id| self.can_send(id))
    }
}
