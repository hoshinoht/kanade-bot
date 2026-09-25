//! Adapter from the bot's roster events to the member rows and the admin
//! session hooks. The serve composition calls it for every
//! `BotEvent::Roster` and `BotEvent::GuildAvailable`.

use twilight_model::id::{Id, marker::UserMarker};

use super::AdminAuth;
use crate::{
    api::state::GuildAccess,
    bot::events::RosterUpdate,
    domain::{
        members::{Member, MemberProfile, MemberStore},
        scheduler::StoreError,
    },
};

/// Persist the update as v4's `upsert_member` did (names and role flag; the
/// ping level, aliases and reply style are kept), then end sessions of anyone
/// who left or no longer passes the staff rule. Returns the sessions ended.
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

/// `BotEvent::GuildAvailable`: the owner counts as staff.
pub fn on_guild_available(access: &GuildAccess, owner_id: Id<UserMarker>) {
    access.set_owner(Some(owner_id));
}
