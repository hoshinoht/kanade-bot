//! Members' avatar hashes from the gateway (join, update, `GUILD_CREATE`) and
//! the startup member list, for the admin portraits (`api::avatars`). Held in
//! memory only: the reconcile after each `READY` lists every member again.

use twilight_model::guild::Member;
use twilight_model::id::{Id, marker::UserMarker};
use twilight_model::user::User;
use twilight_model::util::ImageHash;

use super::{GuildCache, State};
use crate::api::avatars::AvatarRef;
use crate::bot::ids::id_text;

/// A member's guild avatar (wins) and user avatar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MemberAvatar {
    guild: Option<ImageHash>,
    user: Option<ImageHash>,
}

pub(super) fn put(state: &mut State, user: &User, guild: Option<ImageHash>) {
    if user.bot {
        return;
    }
    state.member_avatars.insert(
        user.id,
        MemberAvatar {
            guild,
            user: user.avatar,
        },
    );
}

impl GuildCache {
    /// A member joined or changed (`GUILD_MEMBER_ADD`/`_UPDATE`).
    pub fn member_avatar_seen(&self, user: &User, guild_avatar: Option<ImageHash>) {
        put(&mut self.write(), user, guild_avatar);
    }

    /// A member left: the hash goes now (the cached file with the roster's purge).
    pub fn member_avatar_gone(&self, user_id: Id<UserMarker>) {
        self.write().member_avatars.remove(&user_id);
    }

    /// The full member list (startup reconcile): exactly these members.
    pub fn replace_member_avatars(&self, members: &[Member]) {
        let mut state = self.write();
        state.member_avatars.clear();
        for member in members {
            put(&mut state, &member.user, member.avatar);
        }
    }

    /// The portrait to show for a member: guild avatar, else user avatar.
    pub fn member_avatar(&self, user_id: Id<UserMarker>) -> Option<AvatarRef> {
        let avatar = *self.read().member_avatars.get(&user_id)?;
        let (guild, user) = (id_text(self.guild_id()), id_text(user_id));
        match avatar {
            MemberAvatar {
                guild: Some(hash), ..
            } => AvatarRef::member(&guild, &user, &hash.to_string()),
            MemberAvatar {
                user: Some(hash), ..
            } => AvatarRef::user(&user, &hash.to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64, avatar: Option<&str>, bot: bool) -> User {
        serde_json::from_value(serde_json::json!({
            "id": id.to_string(), "username": "u", "global_name": null, "discriminator": "0",
            "avatar": avatar, "bot": bot,
        }))
        .unwrap()
    }

    const USER: &str = "0123456789abcdef0123456789abcdef";
    const GUILD: &str = "a_fedcba9876543210fedcba9876543210";

    #[test]
    fn guild_avatar_wins_bots_are_skipped_and_leavers_are_forgotten() {
        let cache = GuildCache::new(Id::new(900));
        cache.member_avatar_seen(&user(7, Some(USER), false), None);
        assert_eq!(
            cache.member_avatar(Id::new(7)).unwrap().path(),
            format!("/avatars/7/{USER}.png?size=128")
        );
        cache.member_avatar_seen(&user(7, Some(USER), false), Some(GUILD.parse().unwrap()));
        assert_eq!(
            cache.member_avatar(Id::new(7)).unwrap().path(),
            format!("/guilds/900/users/7/avatars/{GUILD}.png?size=128")
        );
        cache.member_avatar_seen(&user(8, Some(USER), true), None);
        assert_eq!(
            cache.member_avatar(Id::new(8)),
            None,
            "bots are not members"
        );
        cache.member_avatar_seen(&user(9, None, false), None);
        assert_eq!(cache.member_avatar(Id::new(9)), None, "no avatar: monogram");
        cache.member_avatar_gone(Id::new(7));
        assert_eq!(cache.member_avatar(Id::new(7)), None);
    }
}
