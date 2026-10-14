//! Discord's permission computation for the bot's own member (developer docs
//! "Permissions → Permission Hierarchy"): base from `@everyone` and roles,
//! then the channel's `@everyone`, role and member overwrites.
//! Timeouts and private-thread membership are not modelled.

use twilight_model::channel::permission_overwrite::PermissionOverwriteType;
use twilight_model::guild::Permissions;
use twilight_model::id::{Id, marker::GuildMarker};

use super::{CachedChannel, State};

pub(super) fn in_channel(
    guild_id: Id<GuildMarker>,
    state: &State,
    channel: &CachedChannel,
) -> Option<Permissions> {
    let self_id = state.self_id?;
    let roles = state.self_roles.as_ref()?;
    if state.owner_id == Some(self_id) {
        return Some(Permissions::all());
    }
    // `@everyone` shares the guild's id.
    let everyone = guild_id.cast();
    let mut granted = state
        .roles
        .get(&everyone)
        .copied()
        .unwrap_or(Permissions::empty());
    for role in roles {
        granted |= state
            .roles
            .get(role)
            .copied()
            .unwrap_or(Permissions::empty());
    }
    if granted.contains(Permissions::ADMINISTRATOR) {
        return Some(Permissions::all());
    }

    let (mut allow, mut deny) = (Permissions::empty(), Permissions::empty());
    let mut member = None;
    for overwrite in &channel.overwrites {
        match overwrite.kind {
            PermissionOverwriteType::Role if overwrite.id.cast() == everyone => {
                granted = (granted - overwrite.deny) | overwrite.allow;
            }
            PermissionOverwriteType::Role if roles.contains(&overwrite.id.cast()) => {
                allow |= overwrite.allow;
                deny |= overwrite.deny;
            }
            PermissionOverwriteType::Member if overwrite.id.cast() == self_id => {
                member = Some(overwrite);
            }
            _ => {}
        }
    }
    granted = (granted - deny) | allow;
    if let Some(overwrite) = member {
        granted = (granted - overwrite.deny) | overwrite.allow;
    }
    if !granted.contains(Permissions::VIEW_CHANNEL) {
        return Some(Permissions::empty());
    }
    Some(granted)
}
