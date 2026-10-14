//! The gateway's guild cache, in place of the one `serve` fills from
//! Discord: connected, with the seeded guild's channels and roles
//! ([`seed::CHANNELS`], [`seed::ROLES`]) and the bot's own identity.

use kanade::api::state::{ChannelEntry, ChannelGrants, ChannelList, RoleEntry};

use crate::seed;

/// The bot's Discord user id and name, as the mock's gateway reports them.
const BOT_USER_ID: &str = seed::ids::BOT;
const BOT_NAME: &str = "YuukiSakuna";

pub struct Guild;

impl ChannelList for Guild {
    fn channels(&self) -> Vec<ChannelEntry> {
        seed::CHANNELS
            .iter()
            .map(|channel| ChannelEntry {
                id: channel.id.into(),
                name: format!("#{}", channel.id),
                watched: channel.watched,
            })
            .collect()
    }

    fn roles(&self) -> Vec<RoleEntry> {
        seed::ROLES
            .iter()
            .map(|&(id, name, color)| RoleEntry {
                id: id.into(),
                name: name.into(),
                color,
            })
            .collect()
    }

    fn bot_user_id(&self) -> Option<String> {
        Some(BOT_USER_ID.into())
    }

    fn bot_name(&self) -> Option<String> {
        Some(BOT_NAME.into())
    }

    fn connected(&self) -> bool {
        true
    }

    fn grants(&self, id: &str) -> Option<ChannelGrants> {
        let channel = seed::CHANNELS.iter().find(|channel| channel.id == id)?;
        Some(ChannelGrants {
            send: channel.send,
            manage_messages: channel.manage,
            ..ChannelGrants::UNKNOWN
        })
    }
}
