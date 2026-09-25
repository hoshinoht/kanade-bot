//! Twilight model builders (from JSON, as Discord sends them) and shared ids.
#![allow(dead_code)]

use serde_json::{Value, json};
use twilight_gateway::Event;
use twilight_model::application::interaction::Interaction;
use twilight_model::channel::{Channel, Message};
use twilight_model::gateway::GatewayReaction;
use twilight_model::gateway::payload::incoming::{
    ChannelCreate, ChannelDelete, ChannelUpdate, GuildCreate, GuildUpdate, MemberAdd, MemberRemove,
    MemberUpdate, MessageCreate, MessageDelete, MessageDeleteBulk, MessageUpdate, ReactionAdd,
    ReactionRemove, Ready, RoleCreate, RoleDelete, RoleUpdate, ThreadCreate, ThreadDelete,
    ThreadListSync, ThreadUpdate,
};
use twilight_model::id::{
    Id,
    marker::{GuildMarker, RoleMarker, UserMarker},
};

use kanade::bot::events::{BotEvent, GuildScope, Router};

pub const GUILD: u64 = 100;
pub const OTHER_GUILD: u64 = 200;
pub const CHANNEL: u64 = 300;
pub const MESSAGE: u64 = 400;
pub const BOSSING_ROLE: u64 = 500;
pub const ADMIN_ROLE: u64 = 501;
pub const SELF_ID: u64 = 900;
pub const ALICE: u64 = 1001;
pub const BOB: u64 = 1002;
pub const OWNER: u64 = 1003;

pub fn guild() -> Id<GuildMarker> {
    Id::new(GUILD)
}

pub fn role(id: u64) -> Id<RoleMarker> {
    Id::new(id)
}

pub fn user(id: u64) -> Id<UserMarker> {
    Id::new(id)
}

pub fn scope() -> GuildScope {
    GuildScope {
        guild_id: guild(),
        bossing_role_id: role(BOSSING_ROLE),
    }
}

/// One event through a fresh router (no guild state).
pub fn route(scope: GuildScope, event: Event) -> Option<BotEvent> {
    Router::new(scope).route(event)
}

pub const ADMINISTRATOR: u64 = 1 << 3;

pub fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("twilight model fixture")
}

pub fn user_json(id: u64, name: &str, global_name: Option<&str>, bot: bool) -> Value {
    json!({
        "id": id.to_string(),
        "username": name,
        "global_name": global_name,
        "discriminator": "0",
        "avatar": null,
        "bot": bot,
    })
}

pub fn member_json(user: Value, nick: Option<&str>, roles: &[u64]) -> Value {
    json!({
        "user": user,
        "nick": nick,
        "roles": roles.iter().map(u64::to_string).collect::<Vec<_>>(),
        "joined_at": null,
        "deaf": false,
        "mute": false,
        "flags": 0,
        "communication_disabled_until": null,
    })
}

pub fn unicode(name: &str) -> Value {
    json!({ "id": null, "name": name })
}

pub fn custom_emoji(id: u64, name: &str) -> Value {
    json!({ "id": id.to_string(), "name": name, "animated": false })
}

/// A reaction by `user_id` in `guild_id`; `member` is sent on adds only.
pub fn reaction(
    guild_id: Option<u64>,
    user_id: u64,
    emoji: Value,
    member: Option<Value>,
) -> GatewayReaction {
    parse(json!({
        "burst": false,
        "channel_id": CHANNEL.to_string(),
        "emoji": emoji,
        "guild_id": guild_id.map(|id| id.to_string()),
        "member": member,
        "message_id": MESSAGE.to_string(),
        "user_id": user_id.to_string(),
    }))
}

pub fn reaction_add(reaction: GatewayReaction) -> Event {
    Event::ReactionAdd(Box::new(ReactionAdd(reaction)))
}

pub fn reaction_remove(reaction: GatewayReaction) -> Event {
    Event::ReactionRemove(Box::new(ReactionRemove(reaction)))
}

pub fn ready(self_id: u64) -> Event {
    let ready: Ready = parse(json!({
        "application": { "id": "9", "flags": 0 },
        "guilds": [],
        "resume_gateway_url": "wss://gateway.invalid",
        "session_id": "session",
        "user": {
            "id": self_id.to_string(),
            "username": "kanade",
            "discriminator": "0",
            "avatar": null,
            "bot": true,
            "mfa_enabled": false,
        },
        "v": 10,
    }));
    Event::Ready(ready)
}

pub fn member_add(guild_id: u64, member: Value) -> Event {
    let mut value = member;
    value["guild_id"] = json!(guild_id.to_string());
    Event::MemberAdd(Box::new(parse::<MemberAdd>(value)))
}

pub fn member_update(guild_id: u64, user: Value, nick: Option<&str>, roles: &[u64]) -> Event {
    let update: MemberUpdate = parse(json!({
        "guild_id": guild_id.to_string(),
        "user": user,
        "nick": nick,
        "roles": roles.iter().map(u64::to_string).collect::<Vec<_>>(),
        "joined_at": null,
        "premium_since": null,
        "avatar": null,
        "communication_disabled_until": null,
    }));
    Event::MemberUpdate(Box::new(update))
}

pub fn member_remove(guild_id: u64, user: Value) -> Event {
    Event::MemberRemove(parse::<MemberRemove>(json!({
        "guild_id": guild_id.to_string(),
        "user": user,
    })))
}

/// A chat-input interaction; `options` is the raw Discord option tree.
pub fn command_interaction(
    guild_id: Option<u64>,
    invoker: u64,
    roles: &[u64],
    permissions: u64,
    name: &str,
    options: Value,
) -> Interaction {
    let mut member = member_json(user_json(invoker, "someone", None, false), None, roles);
    member["permissions"] = json!(permissions.to_string());
    parse(json!({
        "application_id": "9",
        "authorizing_integration_owners": {},
        "entitlements": [],
        "id": "7700",
        "type": 2,
        "token": "interaction-secret-token",
        "guild_id": guild_id.map(|id| id.to_string()),
        "member": member,
        "data": { "id": "5500", "name": name, "type": 1, "options": options },
    }))
}

pub fn debug_status(
    guild_id: Option<u64>,
    invoker: u64,
    roles: &[u64],
    permissions: u64,
) -> Interaction {
    command_interaction(
        guild_id,
        invoker,
        roles,
        permissions,
        "debug",
        json!([{ "name": "status", "type": 1, "options": [] }]),
    )
}

pub fn role_json(id: u64, permissions: u64) -> Value {
    json!({
        "color": 0,
        "colors": { "primary_color": 0, "secondary_color": null, "tertiary_color": null },
        "hoist": false,
        "id": id.to_string(),
        "managed": false,
        "mentionable": false,
        "name": format!("role-{id}"),
        "permissions": permissions.to_string(),
        "position": 1,
        "flags": 0,
    })
}

/// The guild fields `GUILD_CREATE` and `GUILD_UPDATE` both require.
fn guild_json(guild_id: u64, owner: u64, roles: &[Value]) -> Value {
    json!({
        "afk_channel_id": null,
        "afk_timeout": 300,
        "application_id": null,
        "banner": null,
        "default_message_notifications": 0,
        "description": null,
        "discovery_splash": null,
        "emojis": [],
        "explicit_content_filter": 0,
        "features": [],
        "icon": null,
        "id": guild_id.to_string(),
        "mfa_level": 0,
        "name": "guild",
        "nsfw_level": 0,
        "owner_id": owner.to_string(),
        "preferred_locale": "en-US",
        "premium_progress_bar_enabled": false,
        "premium_tier": 0,
        "public_updates_channel_id": null,
        "roles": roles,
        "rules_channel_id": null,
        "splash": null,
        "system_channel_flags": 0,
        "system_channel_id": null,
        "verification_level": 0,
        "vanity_url_code": null,
    })
}

pub fn guild_create(guild_id: u64, owner: u64, roles: &[Value]) -> Event {
    guild_create_with(guild_id, owner, roles, &[], &[], &[])
}

/// `GUILD_CREATE` with channels, active threads and members.
pub fn guild_create_with(
    guild_id: u64,
    owner: u64,
    roles: &[Value],
    channels: &[Value],
    threads: &[Value],
    members: &[Value],
) -> Event {
    let mut guild = guild_json(guild_id, owner, roles);
    for list in ["presences", "stickers", "voice_states"] {
        guild[list] = json!([]);
    }
    guild["channels"] = json!(channels);
    guild["threads"] = json!(threads);
    guild["members"] = json!(members);
    guild["guild_scheduled_events"] = json!([]);
    guild["stage_instances"] = json!([]);
    Event::GuildCreate(Box::new(parse::<GuildCreate>(guild)))
}

pub const TEXT: u8 = 0;
pub const CATEGORY: u8 = 4;
pub const PUBLIC_THREAD: u8 = 11;
pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const SEND_MESSAGES: u64 = 1 << 11;
pub const SEND_IN_THREADS: u64 = 1 << 38;

/// A permission overwrite; `kind` 0 = role, 1 = member.
pub fn overwrite(id: u64, kind: u8, allow: u64, deny: u64) -> Value {
    json!({
        "id": id.to_string(),
        "type": kind,
        "allow": allow.to_string(),
        "deny": deny.to_string(),
    })
}

/// A guild channel as `GUILD_CREATE` lists it (no `guild_id`).
pub fn channel_json(
    id: u64,
    kind: u8,
    name: &str,
    parent: Option<u64>,
    overwrites: &[Value],
) -> Value {
    json!({
        "id": id.to_string(),
        "type": kind,
        "name": name,
        "parent_id": parent.map(|id| id.to_string()),
        "position": 0,
        "permission_overwrites": overwrites,
    })
}

pub fn thread_json(id: u64, parent: u64, name: &str) -> Value {
    json!({
        "id": id.to_string(),
        "type": PUBLIC_THREAD,
        "name": name,
        "parent_id": parent.to_string(),
        "owner_id": ALICE.to_string(),
        "thread_metadata": {
            "archived": false,
            "auto_archive_duration": 1440,
            "archive_timestamp": "2026-09-25T12:00:00.000000+00:00",
            "locked": false,
        },
    })
}

/// `channel` as a gateway event payload for `guild_id` (`None` = a DM).
fn in_guild(mut channel: Value, guild_id: Option<u64>) -> Value {
    channel["guild_id"] = json!(guild_id.map(|id| id.to_string()));
    channel
}

pub fn channel_create(guild_id: u64, channel: Value) -> Event {
    Event::ChannelCreate(Box::new(parse::<ChannelCreate>(in_guild(
        channel,
        Some(guild_id),
    ))))
}

pub fn channel_update(guild_id: u64, channel: Value) -> Event {
    Event::ChannelUpdate(Box::new(parse::<ChannelUpdate>(in_guild(
        channel,
        Some(guild_id),
    ))))
}

pub fn channel_delete(guild_id: u64, channel: Value) -> Event {
    Event::ChannelDelete(Box::new(parse::<ChannelDelete>(in_guild(
        channel,
        Some(guild_id),
    ))))
}

pub fn thread_create(guild_id: u64, thread: Value) -> Event {
    Event::ThreadCreate(Box::new(parse::<ThreadCreate>(in_guild(
        thread,
        Some(guild_id),
    ))))
}

pub fn thread_update(guild_id: u64, thread: Value) -> Event {
    Event::ThreadUpdate(Box::new(parse::<ThreadUpdate>(in_guild(
        thread,
        Some(guild_id),
    ))))
}

pub fn thread_delete(guild_id: u64, id: u64, parent: u64) -> Event {
    Event::ThreadDelete(parse::<ThreadDelete>(json!({
        "guild_id": guild_id.to_string(),
        "id": id.to_string(),
        "type": PUBLIC_THREAD,
        "parent_id": parent.to_string(),
    })))
}

pub fn thread_list_sync(guild_id: u64, parents: &[u64], threads: &[Value]) -> Event {
    Event::ThreadListSync(parse::<ThreadListSync>(json!({
        "guild_id": guild_id.to_string(),
        "channel_ids": parents.iter().map(u64::to_string).collect::<Vec<_>>(),
        "threads": threads.iter().map(|thread| in_guild(thread.clone(), Some(guild_id))).collect::<Vec<_>>(),
        "members": [],
    })))
}

pub fn parse_channel(channel: Value) -> Channel {
    parse(channel)
}

/// A message by Alice in `channel_id`; `guild_id` `None` is a DM.
pub fn message_json(id: u64, channel_id: u64, guild_id: Option<u64>, content: &str) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": channel_id.to_string(),
        "guild_id": guild_id.map(|id| id.to_string()),
        "author": user_json(ALICE, "alice", None, false),
        "content": content,
        "timestamp": "2026-09-25T12:00:00.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
    })
}

pub fn parse_message(message: Value) -> Message {
    parse(message)
}

pub fn message_create(message: Value) -> Event {
    Event::MessageCreate(Box::new(parse::<MessageCreate>(message)))
}

pub fn message_update(message: Value) -> Event {
    Event::MessageUpdate(Box::new(parse::<MessageUpdate>(message)))
}

pub fn message_delete(guild_id: Option<u64>, channel_id: u64, id: u64) -> Event {
    Event::MessageDelete(parse::<MessageDelete>(json!({
        "id": id.to_string(),
        "channel_id": channel_id.to_string(),
        "guild_id": guild_id.map(|id| id.to_string()),
    })))
}

pub fn message_delete_bulk(guild_id: Option<u64>, channel_id: u64, ids: &[u64]) -> Event {
    Event::MessageDeleteBulk(parse::<MessageDeleteBulk>(json!({
        "ids": ids.iter().map(u64::to_string).collect::<Vec<_>>(),
        "channel_id": channel_id.to_string(),
        "guild_id": guild_id.map(|id| id.to_string()),
    })))
}

pub fn guild_update(guild_id: u64, owner: u64, roles: &[Value]) -> Event {
    Event::GuildUpdate(Box::new(parse::<GuildUpdate>(guild_json(
        guild_id, owner, roles,
    ))))
}

pub fn role_create(guild_id: u64, role: Value) -> Event {
    Event::RoleCreate(parse::<RoleCreate>(
        json!({ "guild_id": guild_id.to_string(), "role": role }),
    ))
}

pub fn role_update(guild_id: u64, role: Value) -> Event {
    Event::RoleUpdate(parse::<RoleUpdate>(
        json!({ "guild_id": guild_id.to_string(), "role": role }),
    ))
}

pub fn role_delete(guild_id: u64, role_id: u64) -> Event {
    Event::RoleDelete(parse::<RoleDelete>(
        json!({ "guild_id": guild_id.to_string(), "role_id": role_id.to_string() }),
    ))
}
