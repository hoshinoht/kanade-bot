//! Least-privilege gateway intents, matching what v4 subscribed to and used.

use twilight_gateway::{EventTypeFlags, Intents};

/// * `GUILDS`: guild availability and the owner id for staff checks.
/// * `GUILD_MEMBERS` (privileged): roster sync from the bossing role.
/// * `GUILD_MESSAGES`: watched-channel messages for chat and extraction.
/// * `MESSAGE_CONTENT` (privileged): their text, which extraction reads.
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
/// Message events are subscribed through [`INTENTS`] but have no handler yet.
pub const WANTED_EVENTS: EventTypeFlags = EventTypeFlags::READY
    .union(EventTypeFlags::GUILD_CREATE)
    .union(EventTypeFlags::MEMBER_ADD)
    .union(EventTypeFlags::MEMBER_UPDATE)
    .union(EventTypeFlags::MEMBER_REMOVE)
    .union(EventTypeFlags::REACTION_ADD)
    .union(EventTypeFlags::REACTION_REMOVE)
    .union(EventTypeFlags::INTERACTION_CREATE);
