//! Gateway roster changes and how they reach the store: names, roles and
//! Administrator are persisted, then the web side's [`RosterHooks`] end the
//! sessions that no longer pass and forget a departed member's portrait.

use std::{collections::BTreeSet, future::Future, pin::Pin};

use twilight_model::id::{Id, marker::UserMarker};

use super::{guild::GuildAccess, ids::id_text};
use kanade_domain::{
    members::{GatewayMember, Member, MemberStore, Roster},
    scheduler::StoreError,
};

/// A roster change seen on the gateway. Bots never produce one: a bot holding
/// the role must never become a participant or be pinged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RosterUpdate {
    /// v4 `upsert_member`: names and role flag; the ping level is untouched.
    /// v5 adds the role ids and computed Administrator the staff gate reads.
    Seen {
        user_id: String,
        display_name: String,
        nickname: Option<String>,
        has_role: bool,
        roles: Vec<String>,
        is_guild_admin: bool,
    },
    /// Left the guild. v5 clears the role flag now rather than at the next
    /// full sync; the row and its ping level are kept.
    Left { user_id: String },
}

impl RosterUpdate {
    pub fn user_id(&self) -> &str {
        match self {
            Self::Seen { user_id, .. } | Self::Left { user_id } => user_id,
        }
    }

    /// Apply to an in-memory roster; persistence applies the same rule.
    pub fn apply(&self, roster: &mut Roster) {
        match self {
            Self::Seen {
                user_id,
                display_name,
                nickname,
                has_role,
                ..
            } => {
                let existing = roster.get(user_id).cloned().unwrap_or_default();
                roster.upsert(Member {
                    user_id: user_id.clone(),
                    display_name: Some(display_name.clone()),
                    nickname: nickname.clone(),
                    has_role: *has_role,
                    is_bot: false,
                    ping_level: existing.ping_level,
                });
            }
            Self::Left { user_id } => {
                if let Some(member) = roster.get(user_id).cloned() {
                    roster.upsert(Member {
                        has_role: false,
                        ..member
                    });
                }
            }
        }
    }
}

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

/// What the web side does when the gateway changes a member: end the
/// sessions that no longer pass (the admin staff rule, the portal's bossing
/// role) and forget a departed member's portrait. Each returns the sessions
/// ended.
pub trait RosterHooks: Send + Sync {
    /// Re-check both realms; `eligible` = still holds the bossing role.
    fn changed<'a>(&'a self, user_id: &'a str, eligible: bool) -> HookFuture<'a>;
    /// The member left: drop their portrait, then end both realms' sessions.
    fn left<'a>(&'a self, user_id: &'a str) -> HookFuture<'a>;
    /// Re-check the admin realm only (Administrator or ownership changed).
    fn staff_changed<'a>(&'a self, user_id: &'a str) -> HookFuture<'a>;
}

pub type HookFuture<'a> = Pin<Box<dyn Future<Output = u64> + Send + 'a>>;

/// Persist the update as v4's `upsert_member` did (names and role flag; the
/// ping level, aliases and reply style are kept) plus the gateway's role ids
/// and Administrator, then end sessions of anyone who left or no longer
/// passes the staff rule (admin) or holds the bossing role (public portal,
/// `member`); a member who left also loses their cached portrait. Returns
/// the sessions ended.
pub async fn on_roster_update<S: MemberStore>(
    hooks: &dyn RosterHooks,
    members: &S,
    update: &RosterUpdate,
) -> Result<u64, StoreError> {
    // Gateway-owned fields only, each in one write: a concurrent portal edit
    // (ping level, aliases, reply style) can never be overwritten.
    match update {
        RosterUpdate::Seen {
            user_id,
            display_name,
            nickname,
            has_role,
            roles,
            is_guild_admin,
        } => {
            members
                .apply_gateway(GatewayMember {
                    user_id: user_id.clone(),
                    display_name: Some(display_name.clone()),
                    nickname: nickname.clone(),
                    has_role: *has_role,
                    is_bot: false,
                    roles: roles.clone(),
                    is_guild_admin: *is_guild_admin,
                })
                .await?;
            Ok(hooks.changed(user_id, *has_role).await)
        }
        RosterUpdate::Left { user_id } => {
            // A member who left holds no role, whatever the last update said.
            members.member_departed(user_id).await?;
            Ok(hooks.left(user_id).await)
        }
    }
}

/// `BotEvent::GuildAvailable`: record the owner, clear the stored
/// Administrator of members whose roles no longer grant it and re-check them
/// and a replaced owner. Returns the sessions ended.
///
/// Only revokes: a stored row may belong to someone who left while the bot
/// was offline, so grants wait for that member's next gateway update.
pub async fn on_guild_available<S: MemberStore>(
    hooks: &dyn RosterHooks,
    access: &GuildAccess,
    members: &S,
    owner_id: Id<UserMarker>,
    admin_roles: &AdminRoles,
) -> Result<u64, StoreError> {
    let previous = access.owner();
    access.set_owner(Some(owner_id));
    let mut recheck = Vec::new();
    for profile in members.list_members().await? {
        if profile.is_guild_admin && !admin_roles.grants(&profile.roles) {
            recheck.push(profile.member.user_id.clone());
            members.clear_guild_admin(&profile.member.user_id).await?;
        }
    }
    if let Some(previous) = previous.filter(|previous| *previous != owner_id) {
        recheck.push(id_text(previous));
    }
    let mut ended = 0;
    for user_id in &recheck {
        ended += hooks.staff_changed(user_id).await;
    }
    Ok(ended)
}
