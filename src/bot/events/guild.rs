//! The guild facts the staff rule needs: the owner and which roles grant
//! Administrator, kept from `GUILD_CREATE`, `GUILD_UPDATE` and role events.

use std::collections::{BTreeSet, HashMap};

use twilight_model::guild::{Permissions, Role};
use twilight_model::id::{
    Id,
    marker::{GuildMarker, RoleMarker, UserMarker},
};

use crate::bot::ids::id_text;

/// The roles whose permissions include Administrator, as domain role ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdminRoles {
    /// `@everyone` grants it, so every member holds it.
    pub everyone: bool,
    pub roles: BTreeSet<String>,
}

impl AdminRoles {
    /// Discord's computed Administrator for a member holding `roles`. A role
    /// id the cache has not seen never grants it.
    pub fn grants(&self, roles: &[String]) -> bool {
        self.everyone || roles.iter().any(|role| self.roles.contains(role))
    }
}

/// Role permissions and owner of the configured guild. Empty until the guild
/// becomes available, so nothing computes as Administrator before then.
#[derive(Debug)]
pub struct GuildRoles {
    guild_id: Id<GuildMarker>,
    owner_id: Option<Id<UserMarker>>,
    permissions: HashMap<Id<RoleMarker>, Permissions>,
}

impl GuildRoles {
    pub fn new(guild_id: Id<GuildMarker>) -> Self {
        Self {
            guild_id,
            owner_id: None,
            permissions: HashMap::new(),
        }
    }

    pub fn owner_id(&self) -> Option<Id<UserMarker>> {
        self.owner_id
    }

    /// Replace everything from a full guild payload.
    pub fn reset(&mut self, owner_id: Id<UserMarker>, roles: &[Role]) {
        self.owner_id = Some(owner_id);
        self.permissions = roles
            .iter()
            .map(|role| (role.id, role.permissions))
            .collect();
    }

    pub fn put_role(&mut self, role: &Role) {
        self.permissions.insert(role.id, role.permissions);
    }

    pub fn remove_role(&mut self, role_id: Id<RoleMarker>) {
        self.permissions.remove(&role_id);
    }

    pub fn admin_roles(&self) -> AdminRoles {
        let mut admin = AdminRoles::default();
        for (role, permissions) in &self.permissions {
            if !permissions.contains(Permissions::ADMINISTRATOR) {
                continue;
            }
            // `@everyone` shares the guild's id and is never listed on members.
            if role.get() == self.guild_id.get() {
                admin.everyone = true;
            } else {
                admin.roles.insert(id_text(*role));
            }
        }
        admin
    }
}
