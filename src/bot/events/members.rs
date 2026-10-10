//! Guild member events as roster updates (v4 `_track_member`).

use twilight_model::gateway::payload::incoming::MemberUpdate;
use twilight_model::guild::Member as GuildMember;
use twilight_model::id::{Id, marker::RoleMarker};
use twilight_model::user::User;

use super::guild::AdminRoles;
use crate::bot::ids::id_text;
use crate::domain::members::{Member, Roster};

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

/// discord.py `Member.display_name`: nickname, global name, then username,
/// skipping empty text.
fn display_name(nick: Option<&str>, user: &User) -> String {
    [nick, user.global_name.as_deref()]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .unwrap_or(&user.name)
        .to_owned()
}

fn seen(
    user: &User,
    nick: Option<&String>,
    roles: &[Id<RoleMarker>],
    bossing_role: Id<RoleMarker>,
    admin: &AdminRoles,
) -> Option<RosterUpdate> {
    if user.bot {
        return None;
    }
    let role_ids: Vec<String> = roles.iter().copied().map(id_text).collect();
    Some(RosterUpdate::Seen {
        user_id: id_text(user.id),
        display_name: display_name(nick.map(String::as_str), user),
        nickname: nick.cloned(),
        has_role: roles.contains(&bossing_role),
        is_guild_admin: admin.grants(&role_ids),
        roles: role_ids,
    })
}

/// A joined (or fully known) member.
pub fn roster_update(
    member: &GuildMember,
    bossing_role: Id<RoleMarker>,
    admin: &AdminRoles,
) -> Option<RosterUpdate> {
    seen(
        &member.user,
        member.nick.as_ref(),
        &member.roles,
        bossing_role,
        admin,
    )
}

/// A member update carries the full role list and nickname.
pub fn member_update(
    update: &MemberUpdate,
    bossing_role: Id<RoleMarker>,
    admin: &AdminRoles,
) -> Option<RosterUpdate> {
    seen(
        &update.user,
        update.nick.as_ref(),
        &update.roles,
        bossing_role,
        admin,
    )
}
