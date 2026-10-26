//! Guild members as roster updates (v4 `_track_member`) and the startup
//! reconciliation of the member list.

mod reconcile;

use twilight_model::gateway::payload::incoming::MemberUpdate;
use twilight_model::guild::Member as GuildMember;
use twilight_model::id::{Id, marker::RoleMarker};
use twilight_model::user::User;

use crate::app::ids::id_text;
use crate::app::roster::{AdminRoles, RosterUpdate};

pub use reconcile::{ReconcileError, ReconcileReport, diff, fetch_members};

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
