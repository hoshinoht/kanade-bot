//! The bot's own display name and avatar hashes, for the admin masthead and
//! the identity art refresh (`bot::identity`).

use tokio::sync::watch;
use twilight_model::id::{Id, marker::UserMarker};
use twilight_model::user::CurrentUser;
use twilight_model::util::ImageHash;

use super::{GuildCache, State};

/// Nickname and guild avatar of the bot's member; `true` if either changed.
pub(super) fn set_member(state: &mut State, nick: Option<&str>, avatar: Option<ImageHash>) -> bool {
    let nick = nick.map(str::to_owned);
    let changed = state.self_nick != nick || state.self_guild_avatar != avatar;
    state.self_nick = nick;
    state.self_guild_avatar = avatar;
    changed
}

impl GuildCache {
    /// `READY` or `USER_UPDATE`: id, names and user avatar. Always signals a
    /// profile change so each new session refreshes the art once.
    pub fn set_self_user(&self, user: &CurrentUser) {
        {
            let mut state = self.write();
            state.self_id = Some(user.id);
            state.self_names = std::iter::once(user.name.clone())
                .chain(user.global_name.clone())
                .collect();
            state.self_display = user
                .global_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| Some(user.name.clone()));
            state.self_avatar = user.avatar;
        }
        self.profile_changed();
    }

    /// A `GUILD_MEMBER_UPDATE`; only the bot's own nickname and avatar are kept.
    pub fn member_profile(
        &self,
        user_id: Id<UserMarker>,
        nick: Option<&str>,
        avatar: Option<ImageHash>,
    ) {
        let mut state = self.write();
        if state.self_id != Some(user_id) {
            return;
        }
        let changed = set_member(&mut state, nick, avatar);
        drop(state);
        if changed {
            self.profile_changed();
        }
    }

    /// Guild nickname, else global name, else user name; `None` before `READY`.
    pub fn display_name(&self) -> Option<String> {
        let state = self.read();
        state
            .self_nick
            .clone()
            .filter(|nick| !nick.trim().is_empty())
            .or_else(|| state.self_display.clone())
            .filter(|name| !name.trim().is_empty())
    }

    /// The avatar to show: the guild avatar, else the user avatar.
    pub fn self_avatar(&self) -> Option<SelfAvatar> {
        let state = self.read();
        match (state.self_guild_avatar, state.self_avatar) {
            (Some(hash), _) => Some(SelfAvatar::Guild(hash)),
            (None, Some(hash)) => Some(SelfAvatar::User(hash)),
            (None, None) => None,
        }
    }

    /// The avatar to show as a Discord CDN URL (a V2 digest's thumbnail);
    /// `None` before `READY` or without an avatar.
    pub fn self_avatar_url(&self) -> Option<String> {
        let user = self.read().self_id?;
        Some(match self.self_avatar()? {
            SelfAvatar::Guild(hash) => format!(
                "https://cdn.discordapp.com/guilds/{}/users/{user}/avatars/{hash}.png?size=128",
                self.guild_id()
            ),
            SelfAvatar::User(hash) => {
                format!("https://cdn.discordapp.com/avatars/{user}/{hash}.png?size=128")
            }
        })
    }

    /// Changes after this call; one already signalled counts as pending.
    pub fn profile_changes(&self) -> watch::Receiver<u64> {
        let mut changes = self.profile.subscribe();
        if *changes.borrow() > 0 {
            changes.mark_changed();
        }
        changes
    }

    pub(super) fn profile_changed(&self) {
        self.profile.send_modify(|generation| *generation += 1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfAvatar {
    Guild(ImageHash),
    User(ImageHash),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: &str, global: Option<&str>) -> CurrentUser {
        serde_json::from_value(serde_json::json!({
            "id": "42", "username": name, "global_name": global, "discriminator": "0",
            "avatar": "a_0123456789abcdef0123456789abcdef", "bot": true, "mfa_enabled": false,
        }))
        .unwrap()
    }

    #[test]
    fn name_prefers_nickname_then_global_then_user_name() {
        let cache = GuildCache::new(Id::new(1));
        assert_eq!(cache.display_name(), None, "nothing before READY");
        cache.set_self_user(&user("kanade_bot", None));
        assert_eq!(cache.display_name().as_deref(), Some("kanade_bot"));
        cache.set_self_user(&user("kanade_bot", Some("Kanade")));
        assert_eq!(cache.display_name().as_deref(), Some("Kanade"));
        cache.member_profile(Id::new(7), Some("Not me"), None);
        assert_eq!(cache.display_name().as_deref(), Some("Kanade"));
        cache.member_profile(Id::new(42), Some("Kanade-chan"), None);
        assert_eq!(cache.display_name().as_deref(), Some("Kanade-chan"));
        cache.member_profile(Id::new(42), Some("  "), None);
        assert_eq!(cache.display_name().as_deref(), Some("Kanade"));
        assert_eq!(cache.self_names(), vec!["kanade_bot", "Kanade"]);
    }

    #[test]
    fn guild_avatar_wins_and_changes_are_signalled() {
        let cache = GuildCache::new(Id::new(1));
        let mut changes = cache.profile_changes();
        assert!(!changes.has_changed().unwrap());
        cache.set_self_user(&user("k", None));
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        assert!(matches!(cache.self_avatar(), Some(SelfAvatar::User(hash)) if hash.is_animated()));

        let guild: ImageHash = "0123456789abcdef0123456789abcdef".parse().unwrap();
        cache.member_profile(Id::new(42), None, Some(guild));
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        assert_eq!(cache.self_avatar(), Some(SelfAvatar::Guild(guild)));
        cache.member_profile(Id::new(42), None, Some(guild));
        assert!(
            !changes.has_changed().unwrap(),
            "unchanged member: no refresh"
        );
        assert!(
            cache.profile_changes().has_changed().unwrap(),
            "late subscriber sees it"
        );
    }

    #[test]
    fn the_avatar_url_points_at_the_cdn() {
        let cache = GuildCache::new(Id::new(1));
        assert_eq!(cache.self_avatar_url(), None, "nothing before READY");
        cache.set_self_user(&user("k", None));
        assert_eq!(
            cache.self_avatar_url().as_deref(),
            Some(
                "https://cdn.discordapp.com/avatars/42/a_0123456789abcdef0123456789abcdef.png?size=128"
            )
        );
        let guild: ImageHash = "0123456789abcdef0123456789abcdef".parse().unwrap();
        cache.member_profile(Id::new(42), None, Some(guild));
        assert_eq!(
            cache.self_avatar_url().as_deref(),
            Some(
                "https://cdn.discordapp.com/guilds/1/users/42/avatars/0123456789abcdef0123456789abcdef.png?size=128"
            )
        );
    }
}
