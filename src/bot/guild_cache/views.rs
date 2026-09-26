//! The cache behind the traits its readers already define.

use std::collections::BTreeSet;

use twilight_model::channel::ChannelType;
use twilight_model::guild::Permissions;
use twilight_model::id::{Id, marker::ChannelMarker};

use super::{CachedChannel, GuildCache};
use crate::api::state::{ChannelEntry, ChannelGrants, ChannelList, RoleEntry};
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

    /// Highest first, as Discord lists them; `@everyone` is left out.
    fn roles(&self) -> Vec<RoleEntry> {
        let guild = self.guild_id().cast();
        let mut roles: Vec<_> = self
            .role_names()
            .into_iter()
            .filter(|(id, _)| *id != guild)
            .collect();
        roles.sort_by_key(|(id, role)| (std::cmp::Reverse(role.position), *id));
        roles
            .into_iter()
            .map(|(id, role)| RoleEntry {
                id: id_text(id),
                name: role.name,
                color: (role.color != 0).then_some(role.color),
            })
            .collect()
    }

    fn bot_user_id(&self) -> Option<String> {
        self.self_id().map(id_text)
    }

    fn bot_name(&self) -> Option<String> {
        self.display_name()
    }

    fn connected(&self) -> bool {
        self.is_available()
    }

    fn grants(&self, id: &str) -> Option<ChannelGrants> {
        let granted = self.permissions(parse_id(id)?)?;
        Some(ChannelGrants {
            view: granted.contains(Permissions::VIEW_CHANNEL),
            send: granted.contains(Permissions::SEND_MESSAGES),
            history: granted.contains(Permissions::READ_MESSAGE_HISTORY),
            embed: granted.contains(Permissions::EMBED_LINKS),
            react: granted.contains(Permissions::ADD_REACTIONS),
            manage_messages: granted.contains(Permissions::MANAGE_MESSAGES),
        })
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

#[cfg(test)]
mod tests {
    use twilight_model::guild::Role;
    use twilight_model::id::Id;

    use super::*;

    fn role(id: u64, name: &str, color: u32, position: i64) -> Role {
        serde_json::from_value(serde_json::json!({
            "id": id.to_string(), "name": name, "color": color,
            "colors": {"primary_color": color, "secondary_color": null, "tertiary_color": null},
            "hoist": false, "managed": false, "mentionable": false,
            "permissions": "0", "position": position, "flags": 0,
        }))
        .unwrap()
    }

    #[test]
    fn roles_list_highest_first_without_everyone() {
        let cache = GuildCache::new(Id::new(1));
        assert_eq!(ChannelList::bot_user_id(&cache), None);
        cache.update_guild(
            Id::new(9),
            &[role(1, "@everyone", 0, 0), role(5, "Bossing", 0, 1)],
        );
        cache.put_role(&role(6, "Officer", 0x0a0bff, 2));
        cache.set_self(Id::new(42));
        assert_eq!(
            ChannelList::roles(&cache),
            vec![
                RoleEntry {
                    id: "6".into(),
                    name: "Officer".into(),
                    color: Some(0x0a0bff),
                },
                RoleEntry {
                    id: "5".into(),
                    name: "Bossing".into(),
                    color: None,
                },
            ]
        );
        cache.remove_role(Id::new(6));
        assert_eq!(ChannelList::roles(&cache).len(), 1);
        assert_eq!(ChannelList::bot_user_id(&cache), Some("42".into()));
    }
}
