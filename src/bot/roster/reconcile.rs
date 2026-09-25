//! Startup roster reconciliation: the guild's full member list against the
//! stored rows, so changes made while the bot was offline are applied.

use std::collections::{BTreeMap, BTreeSet};

use twilight_model::guild::Member as GuildMember;
use twilight_model::id::{
    Id,
    marker::{GuildMarker, RoleMarker},
};

use crate::bot::events::{AdminRoles, RosterUpdate, roster_update};
use crate::bot::ids::id_text;
use crate::bot::transport::{DiscordTransport, MAX_MEMBERS_PAGE, Outcome};
use crate::domain::members::MemberProfile;

/// A page could not be read; nothing may be concluded about departures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileError {
    /// Members read before the failure.
    pub read: usize,
    pub outcome: String,
}

/// What a reconciliation wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub members: usize,
    pub seen: usize,
    pub left: usize,
    pub sessions_ended: u64,
}

/// Every guild member, paging by user id until a short page. `stop` is
/// polled between pages so shutdown is not held up by a large guild.
///
/// # Errors
/// Any page that was not delivered, or `stop` before the last page.
pub async fn fetch_members<T: DiscordTransport>(
    transport: &T,
    guild: Id<GuildMarker>,
    stop: impl Fn() -> bool,
) -> Result<Vec<GuildMember>, ReconcileError> {
    let mut members: Vec<GuildMember> = Vec::new();
    loop {
        if stop() {
            return Err(ReconcileError {
                read: members.len(),
                outcome: "stopped".into(),
            });
        }
        let after = members.last().map(|member| member.user.id);
        let page = match transport.list_members(guild, after, MAX_MEMBERS_PAGE).await {
            Outcome::Delivered(page) => page,
            other => {
                return Err(ReconcileError {
                    read: members.len(),
                    outcome: format!("{:?}", other.map(|_| ())),
                });
            }
        };
        let short = page.len() < usize::from(MAX_MEMBERS_PAGE);
        members.extend(page);
        if short {
            return Ok(members);
        }
    }
}

fn same(stored: &MemberProfile, seen: &RosterUpdate) -> bool {
    let RosterUpdate::Seen {
        display_name,
        nickname,
        has_role,
        roles,
        is_guild_admin,
        ..
    } = seen
    else {
        return false;
    };
    let sorted = |list: &[String]| list.iter().cloned().collect::<BTreeSet<_>>();
    !stored.member.is_bot
        && stored.member.display_name.as_ref() == Some(display_name)
        && stored.member.nickname == *nickname
        && stored.member.has_role == *has_role
        && stored.is_guild_admin == *is_guild_admin
        && sorted(&stored.roles) == sorted(roles)
}

/// The updates that bring `stored` in line with `fetched`: `Seen` for every
/// non-bot member whose row differs (or is missing), `Left` for rows that
/// still look present (role flag, roles or Administrator) but are not in the
/// guild. Rows already cleared are left alone.
pub fn diff(
    fetched: &[GuildMember],
    stored: &[MemberProfile],
    bossing_role: Id<RoleMarker>,
    admin: &AdminRoles,
) -> Vec<RosterUpdate> {
    let present: BTreeSet<String> = fetched
        .iter()
        .map(|member| id_text(member.user.id))
        .collect();
    let rows: BTreeMap<&str, &MemberProfile> = stored
        .iter()
        .map(|row| (row.member.user_id.as_str(), row))
        .collect();
    let mut updates: Vec<RosterUpdate> = fetched
        .iter()
        .filter_map(|member| roster_update(member, bossing_role, admin))
        .filter(|update| {
            !rows
                .get(update.user_id())
                .is_some_and(|row| same(row, update))
        })
        .collect();
    updates.extend(
        stored
            .iter()
            .filter(|row| {
                !present.contains(&row.member.user_id)
                    && (row.member.has_role || !row.roles.is_empty() || row.is_guild_admin)
            })
            .map(|row| RosterUpdate::Left {
                user_id: row.member.user_id.clone(),
            }),
    );
    updates
}
