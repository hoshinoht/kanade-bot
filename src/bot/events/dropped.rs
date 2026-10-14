//! Counts of events refused for belonging to another guild or to no guild,
//! for health reporting. The production token may sit in other guilds.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use twilight_gateway::Event;

/// Shared counters; clone the handle to read them elsewhere.
#[derive(Clone, Debug, Default)]
pub struct DroppedEvents {
    other_guild: Arc<AtomicU64>,
    no_guild: Arc<AtomicU64>,
}

/// A point-in-time reading of [`DroppedEvents`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DroppedCounts {
    pub other_guild: u64,
    /// DMs and other guild-less messages, reactions, interactions, channels.
    pub no_guild: u64,
}

impl DroppedEvents {
    pub fn counts(&self) -> DroppedCounts {
        DroppedCounts {
            other_guild: self.other_guild.load(Ordering::Relaxed),
            no_guild: self.no_guild.load(Ordering::Relaxed),
        }
    }

    pub(super) fn other_guild(&self) {
        self.other_guild.fetch_add(1, Ordering::Relaxed);
    }

    /// Only kinds that normally carry a guild count; gateway control events
    /// (close frames, heartbeats) are guild-less by nature.
    pub(super) fn no_guild(&self, event: &Event) {
        if matches!(
            event,
            Event::MessageCreate(_)
                | Event::MessageUpdate(_)
                | Event::MessageDelete(_)
                | Event::MessageDeleteBulk(_)
                | Event::ReactionAdd(_)
                | Event::ReactionRemove(_)
                | Event::InteractionCreate(_)
                | Event::ChannelCreate(_)
                | Event::ChannelUpdate(_)
                | Event::ChannelDelete(_)
                | Event::ThreadCreate(_)
                | Event::ThreadUpdate(_)
        ) {
            self.no_guild.fetch_add(1, Ordering::Relaxed);
        }
    }
}
