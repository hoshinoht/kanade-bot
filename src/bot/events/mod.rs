//! Gateway events narrowed to the configured guild and mapped to typed
//! adapter events.

mod guild;
mod members;
mod reactions;

use std::fmt;
use std::future::Future;

use twilight_gateway::Event;
use twilight_model::application::interaction::Interaction;
use twilight_model::gateway::GatewayReaction;
use twilight_model::gateway::payload::incoming::GuildCreate;
use twilight_model::id::{
    Id,
    marker::{GuildMarker, RoleMarker, UserMarker},
};

use super::ids::id_text;

pub use guild::{AdminRoles, GuildRoles};
pub use members::{RosterUpdate, member_update, roster_update};
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
    /// The session is ready; the bot's own user id.
    Ready {
        self_id: Id<UserMarker>,
    },
    /// The guild became available, or its owner or Administrator roles
    /// changed. The owner counts as staff.
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
}

impl fmt::Debug for BotEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready { self_id } => f.debug_struct("Ready").field("self_id", self_id).finish(),
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
        }
    }
}

/// Maps gateway events for one guild, keeping the role permissions and owner
/// that member updates need to compute Administrator.
#[derive(Debug)]
pub struct Router {
    scope: GuildScope,
    guild: GuildRoles,
}

impl Router {
    pub fn new(scope: GuildScope) -> Self {
        Self {
            scope,
            guild: GuildRoles::new(scope.guild_id),
        }
    }

    /// Map one gateway event, or `None` for other guilds, DMs, bot members,
    /// unchanged guild access and kinds the adapter does not handle.
    pub fn route(&mut self, event: Event) -> Option<BotEvent> {
        if let Event::Ready(ready) = &event {
            return Some(BotEvent::Ready {
                self_id: ready.user.id,
            });
        }
        if event.guild_id() != Some(self.scope.guild_id) {
            return None;
        }
        let role = self.scope.bossing_role_id;
        match event {
            Event::GuildCreate(create) => match *create {
                GuildCreate::Available(guild) => {
                    self.guild.reset(guild.owner_id, &guild.roles);
                    self.guild_access()
                }
                GuildCreate::Unavailable(_) => None,
            },
            Event::GuildUpdate(update) => {
                self.changed(|guild| guild.reset(update.owner_id, &update.roles))
            }
            Event::RoleCreate(create) => self.changed(|guild| guild.put_role(&create.role)),
            Event::RoleUpdate(update) => self.changed(|guild| guild.put_role(&update.role)),
            Event::RoleDelete(delete) => self.changed(|guild| guild.remove_role(delete.role_id)),
            Event::ReactionAdd(add) => Some(BotEvent::Reaction {
                reaction: Box::new(add.0),
                added: true,
            }),
            Event::ReactionRemove(remove) => Some(BotEvent::Reaction {
                reaction: Box::new(remove.0),
                added: false,
            }),
            Event::MemberAdd(add) => {
                roster_update(&add.member, role, &self.guild.admin_roles()).map(BotEvent::Roster)
            }
            Event::MemberUpdate(update) => {
                member_update(&update, role, &self.guild.admin_roles()).map(BotEvent::Roster)
            }
            Event::MemberRemove(remove) => (!remove.user.bot).then(|| {
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
