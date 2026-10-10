//! Who may run which command (v4 `require_role`, `is_staff`, `may_debug`).

use twilight_model::id::{
    Id,
    marker::{RoleMarker, UserMarker},
};

/// Role configuration the gates read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPolicy {
    pub bossing_role_id: Id<RoleMarker>,
    pub admin_role_id: Option<Id<RoleMarker>>,
    /// `DEBUG_USER_IDS`: testers allowed `/debug` without being staff.
    pub debug_user_ids: Vec<Id<UserMarker>>,
}

/// The invoking member as the interaction reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invoker {
    pub user_id: Id<UserMarker>,
    pub roles: Vec<Id<RoleMarker>>,
    /// Discord's Administrator permission.
    pub is_guild_admin: bool,
}

impl AccessPolicy {
    /// v4 `BossBot.is_admin`: the admin role only. It lets a member change
    /// runs and timings they are not on and see everyone's in pickers.
    pub fn is_admin(&self, invoker: &Invoker) -> bool {
        self.admin_role_id
            .is_some_and(|role| invoker.roles.contains(&role))
    }
}

/// The check a command requires before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Holds the bossing role (staff get no bypass, as in v4).
    BossingRole,
    /// Admin role, guild owner or Administrator permission (v4 `/say`).
    Staff,
    /// Staff or a listed debug user (v4 `/debug`).
    Debug,
    /// Any guild member; the command applies its own rule (v4 `/style`'s
    /// chatbot-access check).
    Anyone,
}

/// A refused invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    MissingBossingRole,
    NotStaff,
    DebugNotAllowed,
}

impl AccessPolicy {
    /// v4 `is_bot_admin`: Administrator, the owner, or the admin role. The
    /// fallbacks keep an unset or mistyped role from locking the owner out.
    pub fn is_staff(&self, invoker: &Invoker, owner_id: Option<Id<UserMarker>>) -> bool {
        invoker.is_guild_admin
            || owner_id == Some(invoker.user_id)
            || self
                .admin_role_id
                .is_some_and(|role| invoker.roles.contains(&role))
    }

    /// # Errors
    /// The [`Denial`] for `gate`.
    pub fn check(
        &self,
        gate: Gate,
        invoker: &Invoker,
        owner_id: Option<Id<UserMarker>>,
    ) -> Result<(), Denial> {
        let (allowed, denial) = match gate {
            Gate::BossingRole => (
                invoker.roles.contains(&self.bossing_role_id),
                Denial::MissingBossingRole,
            ),
            Gate::Staff => (self.is_staff(invoker, owner_id), Denial::NotStaff),
            Gate::Debug => (
                self.is_staff(invoker, owner_id) || self.debug_user_ids.contains(&invoker.user_id),
                Denial::DebugNotAllowed,
            ),
            Gate::Anyone => (true, Denial::NotStaff),
        };
        if allowed { Ok(()) } else { Err(denial) }
    }
}

impl Denial {
    /// v4's ephemeral refusal text; `command` is the top-level name.
    pub fn message(self, command: &str) -> String {
        match self {
            Self::MissingBossingRole => "❌ You need the bossing role to use this bot.".to_owned(),
            Self::NotStaff => {
                format!(
                    "❌ `/{command}` is for server admins, the server owner and the admin role."
                )
            }
            Self::DebugNotAllowed => {
                "❌ `/debug` is restricted to the server owner, the admin role, \
                 and users listed in `DEBUG_USER_IDS`."
                    .to_owned()
            }
        }
    }
}
