//! The channel check (plan 4.4): the bot's grants from the guild cache, one
//! page of the channel's latest messages (its newest message, for the
//! foreign-message check, and which posted slots are still there) and a
//! presence read for each posted slot the page cannot vouch for, all within
//! [`INSPECT_BUDGET`].

use std::collections::{BTreeSet, HashSet};

use super::{CALL_DEADLINE, INSPECT_PAGE, Publisher};
use crate::app::guild::{ChannelGrants, ChannelList};
use crate::app::ids::{id_text, parse_id};
use crate::discord::guild_cache::GuildCache;
use crate::discord::transport::{
    AmbiguousKind, DiscordTransport, HistoryPage, MessageId, Outcome, Presence, RejectionKind,
};
use kanade_domain::guides::{GuideGrants, INSPECT_BUDGET, Inspection, PublishedSlot, SlotState};

/// What the gateway cache knows of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelView {
    /// The guild is not loaded yet: nothing can be said (plan "Unchecked").
    Unknown,
    /// Deleted, or never a channel of this guild.
    Missing,
    Granted(GuideGrants),
}

/// The target channel and the bot's grants there (serve: the guild cache).
pub trait GuideChannels: Send + Sync {
    fn view(&self, channel_id: &str) -> ChannelView;
}

impl GuideChannels for GuildCache {
    fn view(&self, channel_id: &str) -> ChannelView {
        if !self.is_available() {
            return ChannelView::Unknown;
        }
        let Some(id) = parse_id(channel_id) else {
            return ChannelView::Missing;
        };
        if GuildCache::channel(self, id).is_none() {
            return ChannelView::Missing;
        }
        // Unknown own permissions count as granted, as v4 read them; a
        // refusal still comes back from Discord and stops the run.
        let grants = ChannelList::grants(self, channel_id).unwrap_or(ChannelGrants::UNKNOWN);
        ChannelView::Granted(GuideGrants {
            view: grants.view,
            send: grants.send,
            history: grants.history,
            embed: grants.embed,
            attach: grants.attach,
        })
    }
}

/// One worker call bounded by [`CALL_DEADLINE`]; a cut call is ambiguous.
pub(super) async fn deadline<T>(call: impl Future<Output = Outcome<T>>) -> Outcome<T> {
    tokio::time::timeout(CALL_DEADLINE, call)
        .await
        .unwrap_or(Outcome::Ambiguous(AmbiguousKind::Timeout))
}

impl<S, T: DiscordTransport> Publisher<S, T> {
    /// The channel check within [`INSPECT_BUDGET`]; `None` when Discord or
    /// the cache could not answer in time.
    pub async fn inspect_channel(
        &self,
        channel_id: &str,
        slots: &[PublishedSlot],
    ) -> Option<Inspection> {
        tokio::time::timeout(INSPECT_BUDGET, self.read_channel(channel_id, slots))
            .await
            .ok()
            .flatten()
    }

    async fn read_channel(&self, channel_id: &str, slots: &[PublishedSlot]) -> Option<Inspection> {
        let grants = match self.channels.view(channel_id) {
            ChannelView::Unknown => return None,
            ChannelView::Missing => return Some(Inspection::ChannelMissing),
            ChannelView::Granted(grants) => grants,
        };
        let channel = match parse_id(channel_id) {
            Some(channel) if grants.view => channel,
            _ => return Some(Inspection::ChannelMissing),
        };
        let mut gone = BTreeSet::new();
        if !grants.history {
            // Nothing can be read; the plan is blocked on the grant anyway.
            return Some(Inspection::Channel {
                grants,
                gone,
                newest_message_id: None,
            });
        }
        let page = match deadline(self.transport.channel_messages(
            channel,
            HistoryPage::Latest,
            INSPECT_PAGE,
        ))
        .await
        {
            Outcome::Delivered(page) => page,
            Outcome::DefinitelyRejected(
                RejectionKind::UnknownChannel | RejectionKind::MissingAccess,
            ) => return Some(Inspection::ChannelMissing),
            _ => return None,
        };
        let seen: HashSet<MessageId> = page.iter().map(|message| message.id).collect();
        let oldest = seen.iter().min().copied();
        let newest = seen.iter().max().copied();
        let posted = slots
            .iter()
            .filter(|slot| slot.channel_id == channel_id && slot.state == SlotState::Posted);
        for slot in posted {
            let Some(id) = slot.message_id.as_deref().and_then(parse_id) else {
                continue;
            };
            if seen.contains(&id) {
                continue;
            }
            // The page is contiguous: a message inside its span would be in it.
            if oldest
                .zip(newest)
                .is_some_and(|(old, new)| old < id && id < new)
            {
                gone.insert(slot.position);
                continue;
            }
            match deadline(self.transport.message_presence(channel, id)).await {
                Outcome::Delivered(Presence::Absent) => {
                    gone.insert(slot.position);
                }
                Outcome::Delivered(Presence::Present) => {}
                Outcome::DefinitelyRejected(
                    RejectionKind::UnknownChannel | RejectionKind::MissingAccess,
                ) => return Some(Inspection::ChannelMissing),
                _ => return None,
            }
        }
        Some(Inspection::Channel {
            grants,
            gone,
            newest_message_id: newest.map(id_text),
        })
    }
}
