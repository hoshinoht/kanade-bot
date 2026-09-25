//! Least-privilege gateway intents, matching what v4 subscribed to and used.

use twilight_gateway::{EventTypeFlags, Intents};

/// * `GUILDS`: guild availability, owner and role permissions for staff
///   checks, and channel/thread events for the guild cache.
/// * `GUILD_MEMBERS` (privileged): roster sync from the bossing role.
/// * `GUILD_MESSAGES`: watched-channel messages for chat and extraction.
/// * `MESSAGE_CONTENT` (privileged): their text, which extraction reads.
///   Without it (Developer Portal → Bot → Message Content Intent) the
///   gateway closes with 4014.
/// * `GUILD_MESSAGE_REACTIONS`: ✅/❌ RSVPs on cards.
///
/// v4's `Intents.default()` also carried DM, typing, voice, invite and other
/// guild intents the bot never handled; they are deliberately omitted.
pub const INTENTS: Intents = Intents::GUILDS
    .union(Intents::GUILD_MEMBERS)
    .union(Intents::GUILD_MESSAGES)
    .union(Intents::MESSAGE_CONTENT)
    .union(Intents::GUILD_MESSAGE_REACTIONS);

/// Events deserialized for the adapter; everything else is skipped unparsed.
pub const WANTED_EVENTS: EventTypeFlags = EventTypeFlags::READY
    .union(EventTypeFlags::GUILD_CREATE)
    .union(EventTypeFlags::GUILD_UPDATE)
    .union(EventTypeFlags::ROLE_CREATE)
    .union(EventTypeFlags::ROLE_UPDATE)
    .union(EventTypeFlags::ROLE_DELETE)
    .union(EventTypeFlags::MEMBER_ADD)
    .union(EventTypeFlags::MEMBER_UPDATE)
    .union(EventTypeFlags::MEMBER_REMOVE)
    .union(EventTypeFlags::REACTION_ADD)
    .union(EventTypeFlags::REACTION_REMOVE)
    .union(EventTypeFlags::INTERACTION_CREATE)
    .union(EventTypeFlags::MESSAGE_CREATE)
    .union(EventTypeFlags::MESSAGE_UPDATE)
    .union(EventTypeFlags::MESSAGE_DELETE)
    .union(EventTypeFlags::MESSAGE_DELETE_BULK)
    .union(EventTypeFlags::CHANNEL_CREATE)
    .union(EventTypeFlags::CHANNEL_UPDATE)
    .union(EventTypeFlags::CHANNEL_DELETE)
    .union(EventTypeFlags::THREAD_CREATE)
    .union(EventTypeFlags::THREAD_UPDATE)
    .union(EventTypeFlags::THREAD_DELETE)
    .union(EventTypeFlags::THREAD_LIST_SYNC);
