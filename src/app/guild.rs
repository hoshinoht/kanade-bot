//! The guild facts owned by the gateway side that every surface reads:
//! channels and roles, the bot's grants, the staff rule and portraits.

use std::sync::RwLock;

use twilight_model::id::{Id, marker::UserMarker};

use super::access::{AccessPolicy, Invoker};
use kanade_domain::members::MemberProfile;

/// A channel the admin may pick (home channels, digest override).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelEntry {
    pub id: String,
    pub name: String,
    /// The extractor reads it.
    pub watched: bool,
}

/// A guild role for id→name display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleEntry {
    pub id: String,
    pub name: String,
    /// `0xRRGGBB`; `None` when the role has no colour.
    pub color: Option<u32>,
}

/// Guild channels as the bot sees them (gateway cache in production).
pub trait ChannelList: Send + Sync {
    fn channels(&self) -> Vec<ChannelEntry>;

    /// Offline and test lists know no roles.
    fn roles(&self) -> Vec<RoleEntry> {
        Vec::new()
    }

    /// The bot's own Discord user id, once the gateway said `READY`.
    fn bot_user_id(&self) -> Option<String> {
        None
    }

    /// The bot's live display name, once the gateway said `READY`.
    fn bot_name(&self) -> Option<String> {
        None
    }

    /// The guild is loaded, so [`ChannelList::grants`] can answer.
    fn connected(&self) -> bool {
        false
    }

    /// What the bot may do in one channel; `None` while its permissions are
    /// unknown (v4 counts unknown as allowed).
    fn grants(&self, _id: &str) -> Option<ChannelGrants> {
        None
    }

    /// A member's gateway portrait; `None` (the monogram) when the gateway
    /// has not shown one.
    fn member_avatar(&self, _user_id: &str) -> Option<AvatarRef> {
        None
    }
}

/// The bot's effective permissions in a channel, as the access report needs
/// them (v4 `access_report`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelGrants {
    pub view: bool,
    pub send: bool,
    pub history: bool,
    pub embed: bool,
    pub react: bool,
    pub manage_messages: bool,
    /// Attach files (guide art uploads).
    pub attach: bool,
}

impl ChannelGrants {
    /// v4's reading of unknown permissions.
    pub const UNKNOWN: Self = Self {
        view: true,
        send: true,
        history: true,
        embed: true,
        react: true,
        manage_messages: true,
        attach: true,
    };
}

impl std::fmt::Debug for dyn ChannelList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelList")
    }
}

/// A fixed list: offline use and tests.
pub struct StaticChannels(pub Vec<ChannelEntry>);

impl ChannelList for StaticChannels {
    fn channels(&self) -> Vec<ChannelEntry> {
        self.0.clone()
    }
}

/// The bot's staff rule plus the facts it reads that are not member rows:
/// the guild owner (from `GuildAvailable`) and the chat pilot role.
pub struct GuildAccess {
    pub policy: AccessPolicy,
    pub pilot_role: Option<String>,
    owner: RwLock<Option<Id<UserMarker>>>,
}

impl GuildAccess {
    pub fn new(policy: AccessPolicy, pilot_role: Option<String>) -> Self {
        Self {
            policy,
            pilot_role,
            owner: RwLock::new(None),
        }
    }

    pub fn set_owner(&self, owner: Option<Id<UserMarker>>) {
        *self
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = owner;
    }

    pub fn owner(&self) -> Option<Id<UserMarker>> {
        *self
            .owner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The member as the staff rule sees them; bots never qualify.
    pub fn invoker(profile: &MemberProfile) -> Option<Invoker> {
        let user_id = profile
            .member
            .user_id
            .parse::<u64>()
            .ok()
            .and_then(Id::new_checked)?;
        (!profile.member.is_bot).then(|| Invoker {
            user_id,
            roles: profile
                .roles
                .iter()
                .filter_map(|role| role.parse::<u64>().ok().and_then(Id::new_checked))
                .collect(),
            is_guild_admin: profile.is_guild_admin,
        })
    }

    pub fn is_staff(&self, profile: &MemberProfile) -> bool {
        Self::invoker(profile).is_some_and(|invoker| self.policy.is_staff(&invoker, self.owner()))
    }

    /// `staff`, `pilot` (chatbot pilot role) or `none`.
    pub fn access(&self, profile: &MemberProfile) -> &'static str {
        if self.is_staff(profile) {
            "staff"
        } else if self
            .pilot_role
            .as_ref()
            .is_some_and(|role| profile.roles.contains(role))
        {
            "pilot"
        } else {
            "none"
        }
    }
}

/// Requested edge in pixels: a 2x portrait for the largest place it shows.
pub const SIZE: u16 = 128;

/// Which CDN image is a user's portrait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvatarRef {
    user_id: String,
    hash: String,
    /// CDN path with query.
    path: String,
}

/// 1–20 ASCII digits: a snowflake as a path segment.
pub fn snowflake(text: &str) -> bool {
    (1..=20).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn image_hash(text: &str) -> bool {
    let hex = text.strip_prefix("a_").unwrap_or(text);
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl AvatarRef {
    /// The user avatar. Animated hashes still ask for PNG: the first frame.
    pub fn user(user_id: &str, hash: &str) -> Option<Self> {
        (snowflake(user_id) && image_hash(hash)).then(|| Self {
            user_id: user_id.to_owned(),
            hash: hash.to_owned(),
            path: format!("/avatars/{user_id}/{hash}.png?size={SIZE}"),
        })
    }

    /// A guild member's own avatar for this guild.
    pub fn member(guild_id: &str, user_id: &str, hash: &str) -> Option<Self> {
        (snowflake(guild_id) && snowflake(user_id) && image_hash(hash)).then(|| Self {
            user_id: user_id.to_owned(),
            hash: hash.to_owned(),
            path: format!("/guilds/{guild_id}/users/{user_id}/avatars/{hash}.png?size={SIZE}"),
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// The cached file name without its extension: `<user id>-<hash>`.
    pub fn stem(&self) -> String {
        format!("{}-{}", self.user_id, self.hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_take_only_snowflakes_and_image_hashes() {
        let hash = "0123456789abcdef0123456789abcdef";
        let user = AvatarRef::user("1003", hash).unwrap();
        assert_eq!(
            user.path(),
            format!("/avatars/1003/{hash}.png?size=128").as_str()
        );
        let member = AvatarRef::member("900", "1003", &format!("a_{hash}")).unwrap();
        assert_eq!(
            member.path(),
            format!("/guilds/900/users/1003/avatars/a_{hash}.png?size=128").as_str()
        );
        for (user_id, hash) in [
            ("../1", hash),
            ("", hash),
            ("1003", "../../etc"),
            ("1003", "0123456789ABCDEF0123456789ABCDEF"),
            ("1003", "clyde"),
        ] {
            assert_eq!(AvatarRef::user(user_id, hash), None, "{user_id} {hash}");
        }
    }
}
