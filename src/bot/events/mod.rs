//! Gateway events narrowed to the configured guild and mapped to typed
//! adapter events.

mod dropped;
mod guild;
mod members;
mod messages;
mod reactions;

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use twilight_gateway::Event;
use twilight_model::application::interaction::Interaction;
use twilight_model::gateway::GatewayReaction;
use twilight_model::gateway::payload::incoming::GuildCreate;
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker, RoleMarker, UserMarker},
};

use super::guild_cache::GuildCache;
use super::ids::id_text;

pub use dropped::{DroppedCounts, DroppedEvents};
pub use guild::{AdminRoles, GuildRoles};
pub use members::{RosterUpdate, member_update, roster_update};
pub use messages::{DeletedMessages, GuildMessage};
pub use reactions::{
    CardIndex, LookupError, ReactionRouter, ReactionSink, RouteError, RsvpAnswer, RsvpReaction,
    rsvp_reaction,
};

/// Which guild the adapter serves, and its bossing role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuildScope {
    pub guild_id: Id<GuildMarker>,
    pub bossing_role_id: Id<RoleMarker>,
}

/// An event for the configured guild. `Debug` omits the interaction token.
#[derive(Clone, PartialEq)]
pub enum BotEvent {
    /// The session is ready: the bot's own user id and its application
    /// (interaction responses and command registration need it).
    Ready {
        self_id: Id<UserMarker>,
        application_id: Id<ApplicationMarker>,
        /// The bot's user name (command replies about its access).
        name: String,
    },
    /// The guild became available, its owner or Administrator roles
    /// changed, or a role was deleted (stored role lists may name it). The
    /// owner counts as staff.
    GuildAvailable {
        owner_id: Id<UserMarker>,
        admin_roles: AdminRoles,
    },
    Reaction {
        reaction: Box<GatewayReaction>,
        added: bool,
    },
    Roster(RosterUpdate),
    Interaction(Box<Interaction>),
    MessageCreated(Box<GuildMessage>),
    /// Discord sends the full message on edits.
    MessageUpdated(Box<GuildMessage>),
    /// `MESSAGE_DELETE` or `MESSAGE_DELETE_BULK`.
    MessagesDeleted(DeletedMessages),
}

impl fmt::Debug for BotEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready {
                self_id,
                application_id,
                name,
            } => f
                .debug_struct("Ready")
                .field("self_id", self_id)
                .field("application_id", application_id)
                .field("name", name)
                .finish(),
            Self::GuildAvailable {
                owner_id,
                admin_roles,
            } => f
                .debug_struct("GuildAvailable")
                .field("owner_id", owner_id)
                .field("admin_roles", admin_roles)
                .finish(),
            Self::Reaction { reaction, added } => f
                .debug_struct("Reaction")
                .field("reaction", reaction)
                .field("added", added)
                .finish(),
            Self::Roster(update) => f.debug_tuple("Roster").field(update).finish(),
            Self::Interaction(interaction) => f
                .debug_struct("Interaction")
                .field("id", &interaction.id)
                .field("kind", &interaction.kind)
                .field("guild_id", &interaction.guild_id)
                .field("token", &"<redacted>")
                .finish_non_exhaustive(),
            Self::MessageCreated(message) => {
                f.debug_tuple("MessageCreated").field(message).finish()
            }
            Self::MessageUpdated(message) => {
                f.debug_tuple("MessageUpdated").field(message).finish()
            }
            Self::MessagesDeleted(deleted) => {
                f.debug_tuple("MessagesDeleted").field(deleted).finish()
            }
        }
    }
}

/// Maps gateway events for one guild, keeping the role permissions and owner
/// that member updates need to compute Administrator, and feeding the shared
/// [`GuildCache`] (channels, threads, the bot's own roles).
#[derive(Debug)]
pub struct Router {
    scope: GuildScope,
    guild: GuildRoles,
    cache: Arc<GuildCache>,
    dropped: DroppedEvents,
}

impl Router {
    pub fn new(scope: GuildScope) -> Self {
        Self::with_cache(scope, Arc::new(GuildCache::new(scope.guild_id)))
    }

    /// Feed a cache shared with its readers; it must be for `scope`'s guild.
    pub fn with_cache(scope: GuildScope, cache: Arc<GuildCache>) -> Self {
        debug_assert_eq!(cache.guild_id(), scope.guild_id);
        Self {
            scope,
            guild: GuildRoles::new(scope.guild_id),
            cache,
            dropped: DroppedEvents::default(),
        }
    }

    pub fn cache(&self) -> &Arc<GuildCache> {
        &self.cache
    }

    /// Counters of refused other-guild and guild-less events.
    pub fn dropped(&self) -> DroppedEvents {
        self.dropped.clone()
    }

    /// Map one gateway event, or `None` for other guilds, DMs, bot members,
    /// unchanged guild access, channel/thread bookkeeping and kinds the
    /// adapter does not handle.
    pub fn route(&mut self, event: Event) -> Option<BotEvent> {
        if let Event::Ready(ready) = &event {
            self.cache.set_self_user(&ready.user);
            return Some(BotEvent::Ready {
                self_id: ready.user.id,
                application_id: ready.application.id,
                name: ready.user.name.clone(),
            });
        }
        // Guild-less, and always the bot itself.
        if let Event::UserUpdate(update) = &event {
            self.cache.set_self_user(update);
            return None;
        }
        match event.guild_id() {
            Some(guild) if guild == self.scope.guild_id => {}
            Some(_) => {
                self.dropped.other_guild();
                return None;
            }
            None => {
                self.dropped.no_guild(&event);
                return None;
            }
        }
        let role = self.scope.bossing_role_id;
        let cache = &self.cache;
        match event {
            Event::GuildCreate(create) => match *create {
                GuildCreate::Available(guild) => {
                    cache.reset(&guild);
                    self.guild.reset(guild.owner_id, &guild.roles);
                    self.guild_access()
                }
                GuildCreate::Unavailable(_) => {
                    cache.set_unavailable();
                    None
                }
            },
            Event::GuildUpdate(update) => {
                cache.update_guild(update.owner_id, &update.roles);
                self.changed(|guild| guild.reset(update.owner_id, &update.roles))
            }
            Event::RoleCreate(create) => {
                cache.put_role(&create.role);
                self.changed(|guild| guild.put_role(&create.role))
            }
            Event::RoleUpdate(update) => {
                cache.put_role(&update.role);
                self.changed(|guild| guild.put_role(&update.role))
            }
            // Always reported: Discord sends no member updates for a deleted
            // role, so stored role lists (the admin role) must be pruned.
            Event::RoleDelete(delete) => {
                cache.remove_role(delete.role_id);
                self.guild.remove_role(delete.role_id);
                self.guild_access()
            }
            Event::ChannelCreate(channel) => {
                cache.put_channel(&channel);
                None
            }
            Event::ChannelUpdate(channel) => {
                cache.put_channel(&channel);
                None
            }
            Event::ThreadCreate(thread) => {
                cache.put_channel(&thread);
                None
            }
            Event::ThreadUpdate(thread) => {
                cache.put_channel(&thread);
                None
            }
            Event::ChannelDelete(channel) => {
                cache.remove_channel(channel.id);
                None
            }
            Event::ThreadDelete(thread) => {
                cache.remove_channel(thread.id);
                None
            }
            Event::ThreadListSync(sync) => {
                cache.sync_threads(&sync.channel_ids, &sync.threads);
                None
            }
            Event::MessageCreate(create) => Some(BotEvent::MessageCreated(Box::new(
                GuildMessage::new(create.0, cache),
            ))),
            Event::MessageUpdate(update) => Some(BotEvent::MessageUpdated(Box::new(
                GuildMessage::new(update.0, cache),
            ))),
            Event::MessageDelete(delete) => Some(BotEvent::MessagesDeleted(DeletedMessages::new(
                delete.channel_id,
                vec![delete.id],
                cache,
            ))),
            Event::MessageDeleteBulk(bulk) => Some(BotEvent::MessagesDeleted(
                DeletedMessages::new(bulk.channel_id, bulk.ids, cache),
            )),
            Event::ReactionAdd(add) => Some(BotEvent::Reaction {
                reaction: Box::new(add.0),
                added: true,
            }),
            Event::ReactionRemove(remove) => Some(BotEvent::Reaction {
                reaction: Box::new(remove.0),
                added: false,
            }),
            Event::MemberAdd(add) => {
                cache.member_roles(add.member.user.id, &add.member.roles);
                cache.member_avatar_seen(&add.member.user, add.member.avatar);
                roster_update(&add.member, role, &self.guild.admin_roles()).map(BotEvent::Roster)
            }
            Event::MemberUpdate(update) => {
                cache.member_roles(update.user.id, &update.roles);
                cache.member_profile(update.user.id, update.nick.as_deref(), update.avatar);
                cache.member_avatar_seen(&update.user, update.avatar);
                member_update(&update, role, &self.guild.admin_roles()).map(BotEvent::Roster)
            }
            Event::MemberRemove(remove) => (!remove.user.bot).then(|| {
                cache.member_avatar_gone(remove.user.id);
                BotEvent::Roster(RosterUpdate::Left {
                    user_id: id_text(remove.user.id),
                })
            }),
            Event::InteractionCreate(create) => Some(BotEvent::Interaction(Box::new(create.0))),
            _ => None,
        }
    }

    /// Apply a guild or role change; report it only if the owner or the
    /// Administrator roles moved.
    fn changed(&mut self, apply: impl FnOnce(&mut GuildRoles)) -> Option<BotEvent> {
        let before = (self.guild.owner_id(), self.guild.admin_roles());
        apply(&mut self.guild);
        if (self.guild.owner_id(), self.guild.admin_roles()) == before {
            return None;
        }
        self.guild_access()
    }

    fn guild_access(&self) -> Option<BotEvent> {
        Some(BotEvent::GuildAvailable {
            owner_id: self.guild.owner_id()?,
            admin_roles: self.guild.admin_roles(),
        })
    }
}

/// Consumes routed events; the application composes the concrete handlers.
///
/// `handle` runs inline in the gateway loop: keep it short and never await
/// Discord transport calls in it; spawn them (e.g.
/// [`crate::bot::commands::spawn_interaction`]).
pub trait EventHandler: Send {
    fn handle(&mut self, event: BotEvent) -> impl Future<Output = ()> + Send;
}
