//! The configured guild's channels, threads and the bot's own permissions,
//! fed by `events::Router` from `GUILD_CREATE` and channel, thread, role and
//! member events. Shared (`Arc`) with the API, the chat gate and delivery,
//! which read it through their own traits (`views.rs`).

mod permissions;
mod views;

use std::collections::{BTreeSet, HashMap};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use twilight_model::channel::permission_overwrite::PermissionOverwrite;
use twilight_model::channel::{Channel, ChannelType};
use twilight_model::guild::{Guild, Permissions, Role};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker},
};

pub use views::WatchList;

/// A channel or thread as last seen on the gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedChannel {
    pub id: Id<ChannelMarker>,
    pub name: Option<String>,
    pub kind: ChannelType,
    /// A channel's category, or a thread's parent channel.
    pub parent_id: Option<Id<ChannelMarker>>,
    pub position: Option<i32>,
    overwrites: Vec<PermissionOverwrite>,
}

impl CachedChannel {
    fn from_channel(channel: &Channel) -> Self {
        Self {
            id: channel.id,
            name: channel.name.clone(),
            kind: channel.kind,
            parent_id: channel.parent_id,
            position: channel.position,
            overwrites: channel.permission_overwrites.clone().unwrap_or_default(),
        }
    }

    pub fn is_thread(&self) -> bool {
        self.kind.is_thread()
    }

    /// Text-capable: posts and messages can live here.
    pub fn is_messageable(&self) -> bool {
        self.is_thread()
            || matches!(
                self.kind,
                ChannelType::GuildText
                    | ChannelType::GuildAnnouncement
                    | ChannelType::GuildVoice
                    | ChannelType::GuildStageVoice
            )
    }
}

/// A role's display facts for the admin app's id→name lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleName {
    pub name: String,
    pub color: u32,
    pub position: i64,
    /// A bot's managed integration role: that bot's user id.
    pub bot_id: Option<Id<UserMarker>>,
}

impl RoleName {
    fn of(role: &Role) -> Self {
        Self {
            name: role.name.clone(),
            color: role.colors.primary_color,
            position: role.position,
            bot_id: role
                .managed
                .then(|| role.tags.as_ref().and_then(|tags| tags.bot_id))
                .flatten(),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    available: bool,
    owner_id: Option<Id<UserMarker>>,
    self_id: Option<Id<UserMarker>>,
    /// The bot's user name and global name (`READY`).
    self_names: Vec<String>,
    /// The bot's guild nickname (`GUILD_CREATE`).
    self_nick: Option<String>,
    /// `None` until the bot's own member is seen: permissions are unknown.
    self_roles: Option<Vec<Id<RoleMarker>>>,
    roles: HashMap<Id<RoleMarker>, Permissions>,
    role_names: HashMap<Id<RoleMarker>, RoleName>,
    channels: HashMap<Id<ChannelMarker>, CachedChannel>,
    watch: WatchList,
}

/// One guild's gateway view. Empty until the guild becomes available.
#[derive(Debug)]
pub struct GuildCache {
    guild_id: Id<GuildMarker>,
    state: RwLock<State>,
}

impl GuildCache {
    pub fn new(guild_id: Id<GuildMarker>) -> Self {
        Self {
            guild_id,
            state: RwLock::default(),
        }
    }

    pub fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// `GUILD_CREATE` arrived for this guild.
    pub fn is_available(&self) -> bool {
        self.read().available
    }

    /// The bot's own user id, from `READY`.
    pub fn set_self(&self, self_id: Id<UserMarker>) {
        self.write().self_id = Some(self_id);
    }

    /// The bot's user and global names, from `READY`.
    pub fn set_self_names(&self, names: Vec<String>) {
        self.write().self_names = names;
    }

    /// What members may call the bot (v4 `user.name`/`display_name`, plus
    /// its guild nickname): never a member in a party.
    pub fn self_names(&self) -> Vec<String> {
        let state = self.read();
        let mut names: Vec<String> = state
            .self_names
            .iter()
            .chain(&state.self_nick)
            .filter(|name| !name.trim().is_empty())
            .cloned()
            .collect();
        names.dedup();
        names
    }

    /// The bot's own managed role (Discord offers it when members type
    /// `@Kanade`), once the guild and `READY` are known.
    pub fn self_role(&self) -> Option<Id<RoleMarker>> {
        let state = self.read();
        let me = state.self_id?;
        state
            .role_names
            .iter()
            .filter(|(_, role)| role.bot_id == Some(me))
            .map(|(id, _)| *id)
            .min()
    }

    /// Replace everything from a full guild payload (`GUILD_CREATE`).
    pub fn reset(&self, guild: &Guild) {
        let mut state = self.write();
        state.available = true;
        state.owner_id = Some(guild.owner_id);
        state.roles = guild
            .roles
            .iter()
            .map(|role| (role.id, role.permissions))
            .collect();
        state.role_names = guild
            .roles
            .iter()
            .map(|role| (role.id, RoleName::of(role)))
            .collect();
        state.channels = guild
            .channels
            .iter()
            .chain(&guild.threads)
            .map(|channel| (channel.id, CachedChannel::from_channel(channel)))
            .collect();
        let self_id = state.self_id;
        if let Some(me) = guild
            .members
            .iter()
            .find(|member| Some(member.user.id) == self_id)
        {
            state.self_roles = Some(me.roles.clone());
            state.self_nick.clone_from(&me.nick);
        }
    }

    /// The guild went unavailable (outage): keep the last view, as discord.py
    /// does, but report it.
    pub fn set_unavailable(&self) {
        self.write().available = false;
    }

    /// `GUILD_UPDATE`: owner and full role list.
    pub fn update_guild(&self, owner_id: Id<UserMarker>, roles: &[Role]) {
        let mut state = self.write();
        state.owner_id = Some(owner_id);
        state.roles = roles
            .iter()
            .map(|role| (role.id, role.permissions))
            .collect();
        state.role_names = roles
            .iter()
            .map(|role| (role.id, RoleName::of(role)))
            .collect();
    }

    pub fn put_role(&self, role: &Role) {
        let mut state = self.write();
        state.roles.insert(role.id, role.permissions);
        state.role_names.insert(role.id, RoleName::of(role));
    }

    pub fn remove_role(&self, role_id: Id<RoleMarker>) {
        let mut state = self.write();
        state.roles.remove(&role_id);
        state.role_names.remove(&role_id);
        if let Some(roles) = state.self_roles.as_mut() {
            roles.retain(|role| *role != role_id);
        }
    }

    /// A member's current roles; only the bot's own are kept.
    pub fn member_roles(&self, user_id: Id<UserMarker>, roles: &[Id<RoleMarker>]) {
        let mut state = self.write();
        if state.self_id == Some(user_id) {
            state.self_roles = Some(roles.to_vec());
        }
    }

    /// Every known role's display facts.
    pub fn role_names(&self) -> Vec<(Id<RoleMarker>, RoleName)> {
        self.read()
            .role_names
            .iter()
            .map(|(id, role)| (*id, role.clone()))
            .collect()
    }

    /// The bot's own user id once `READY` arrived.
    pub fn self_id(&self) -> Option<Id<UserMarker>> {
        self.read().self_id
    }

    /// Channel or thread create/update.
    pub fn put_channel(&self, channel: &Channel) {
        self.write()
            .channels
            .insert(channel.id, CachedChannel::from_channel(channel));
    }

    /// Channel or thread delete; a deleted channel takes its threads along.
    pub fn remove_channel(&self, id: Id<ChannelMarker>) {
        let mut state = self.write();
        if state.channels.remove(&id).is_some() {
            state
                .channels
                .retain(|_, channel| !(channel.is_thread() && channel.parent_id == Some(id)));
        }
    }

    /// `THREAD_LIST_SYNC`: the active threads of `parents` (every parent when
    /// empty) are exactly `threads`.
    pub fn sync_threads(&self, parents: &[Id<ChannelMarker>], threads: &[Channel]) {
        let mut state = self.write();
        state.channels.retain(|_, channel| {
            !channel.is_thread()
                || channel
                    .parent_id
                    .is_some_and(|parent| !parents.is_empty() && !parents.contains(&parent))
        });
        for thread in threads {
            state
                .channels
                .insert(thread.id, CachedChannel::from_channel(thread));
        }
    }

    pub fn channel(&self, id: Id<ChannelMarker>) -> Option<CachedChannel> {
        self.read().channels.get(&id).cloned()
    }

    /// v4 `origin_ids`: `(channel, thread)` for a message's channel. A thread
    /// counts under its parent; an unknown channel is its own origin.
    pub fn origin(
        &self,
        channel_id: Id<ChannelMarker>,
    ) -> (Id<ChannelMarker>, Option<Id<ChannelMarker>>) {
        match self.read().channels.get(&channel_id) {
            Some(channel) if channel.is_thread() => match channel.parent_id {
                Some(parent) => (parent, Some(channel_id)),
                None => (channel_id, None),
            },
            _ => (channel_id, None),
        }
    }

    /// The raw channel name (no `#`).
    pub fn name(&self, id: Id<ChannelMarker>) -> Option<String> {
        self.read().channels.get(&id)?.name.clone()
    }

    /// Every cached channel and thread.
    pub fn channels(&self) -> Vec<CachedChannel> {
        self.read().channels.values().cloned().collect()
    }

    /// The bot's effective permissions in a channel (a thread uses its
    /// parent's). `None` when the channel, its parent, or the bot's own roles
    /// are not known yet.
    pub fn permissions(&self, id: Id<ChannelMarker>) -> Option<Permissions> {
        let state = self.read();
        let mut channel = state.channels.get(&id)?;
        if channel.is_thread() {
            channel = state.channels.get(&channel.parent_id?)?;
        }
        permissions::in_channel(self.guild_id, &state, channel)
    }

    /// v4 `can_send_in`: a known, text-capable channel where the bot may view
    /// and send. Unknown permissions (the bot's member not seen yet) count as
    /// allowed, so this only ever turns a known refusal into a skip.
    pub fn can_send(&self, id: Id<ChannelMarker>) -> bool {
        let Some(channel) = self.channel(id) else {
            return false;
        };
        if !channel.is_messageable() {
            return false;
        }
        let send = if channel.is_thread() {
            Permissions::SEND_MESSAGES_IN_THREADS
        } else {
            Permissions::SEND_MESSAGES
        };
        self.permissions(id)
            .is_none_or(|granted| granted.contains(Permissions::VIEW_CHANNEL | send))
    }

    pub fn set_watch(&self, watch: WatchList) {
        self.write().watch = watch;
    }

    /// The guild's current role ids; `None` until `GUILD_CREATE`.
    pub fn role_ids(&self) -> Option<BTreeSet<Id<RoleMarker>>> {
        let state = self.read();
        state
            .available
            .then(|| state.roles.keys().copied().collect())
    }
}
