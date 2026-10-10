//! Adapter from the bot's roster events to the member rows and both realms'
//! session hooks. The serve composition calls it for every
//! `BotEvent::Roster` and `BotEvent::GuildAvailable`.

use twilight_model::id::{Id, marker::UserMarker};

use super::{AdminAuth, member::MemberAuth};
use crate::{
    api::{avatars::AvatarCache, state::GuildAccess},
    bot::{
        events::{AdminRoles, RosterUpdate},
        ids::id_text,
    },
    domain::{
        members::{GatewayMember, MemberStore},
        scheduler::StoreError,
    },
};

/// Persist the update as v4's `upsert_member` did (names and role flag; the
/// ping level, aliases and reply style are kept) plus the gateway's role ids
/// and Administrator, then end sessions of anyone who left or no longer
/// passes the staff rule (admin) or holds the bossing role (public portal,
/// `member`); a member who left also loses their cached portrait. Returns
/// the sessions ended.
pub async fn on_roster_update<S: MemberStore>(
    auth: &AdminAuth,
    member: Option<&MemberAuth>,
    members: &S,
    avatars: Option<&AvatarCache>,
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
            let public = match member {
                Some(member) => member.member_changed(user_id, *has_role).await,
                None => 0,
            };
            Ok(auth.member_changed(user_id).await + public)
        }
        RosterUpdate::Left { user_id } => {
            // A member who left holds no role, whatever the last update said.
            members.member_departed(user_id).await?;
            if let Some(avatars) = avatars {
                avatars.purge(user_id);
            }
            let public = match member {
                Some(member) => member.member_left(user_id).await,
                None => 0,
            };
            Ok(auth.member_left(user_id).await + public)
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
    auth: &AdminAuth,
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
        ended += auth.member_changed(user_id).await;
    }
    Ok(ended)
}
