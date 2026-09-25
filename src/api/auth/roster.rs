//! Adapter from the bot's roster events to the member rows and the admin
//! session hooks. The serve composition calls it for every
//! `BotEvent::Roster` and `BotEvent::GuildAvailable`.

use twilight_model::id::{Id, marker::UserMarker};

use super::AdminAuth;
use crate::{
    api::state::GuildAccess,
    bot::{
        events::{AdminRoles, RosterUpdate},
        ids::id_text,
    },
    domain::{
        members::{Member, MemberProfile, MemberStore},
        scheduler::StoreError,
    },
};

/// Persist the update as v4's `upsert_member` did (names and role flag; the
/// ping level, aliases and reply style are kept) plus the gateway's role ids
/// and Administrator, then end sessions of anyone who left or no longer
/// passes the staff rule. Returns the sessions ended.
pub async fn on_roster_update<S: MemberStore>(
    auth: &AdminAuth,
    members: &S,
    update: &RosterUpdate,
) -> Result<u64, StoreError> {
    let existing = members.load_member(update.user_id()).await?;
    match update {
        RosterUpdate::Seen {
            user_id,
            display_name,
            nickname,
            has_role,
            roles,
            is_guild_admin,
        } => {
            let profile = existing.unwrap_or_default();
            members
                .put_member(MemberProfile {
                    member: Member {
                        user_id: user_id.clone(),
                        display_name: Some(display_name.clone()),
                        nickname: nickname.clone(),
                        has_role: *has_role,
                        is_bot: false,
                        ping_level: profile.member.ping_level,
                    },
                    roles: roles.clone(),
                    is_guild_admin: *is_guild_admin,
                    ..profile
                })
                .await?;
            Ok(auth.member_changed(user_id).await)
        }
        RosterUpdate::Left { user_id } => {
            if let Some(profile) = existing {
                // A member who left holds no role, whatever the last update said.
                members
                    .put_member(MemberProfile {
                        member: Member {
                            has_role: false,
                            ..profile.member
                        },
                        roles: Vec::new(),
                        is_guild_admin: false,
                        ..profile
                    })
                    .await?;
            }
            Ok(auth.member_left(user_id).await)
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
            members
                .put_member(MemberProfile {
                    is_guild_admin: false,
                    ..profile
                })
                .await?;
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
