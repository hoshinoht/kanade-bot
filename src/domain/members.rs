//! The one guild-member model the scheduler and the notifier share: who is on
//! the roster, what to call them, whether they may join runs, how loudly they
//! want to be pinged, and which channels the bot watches.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use crate::domain::ids::short_id;
use crate::domain::pytext;
use crate::domain::scheduler::StoreError;

spelled! {
    /// A member's mention preference, in v4's `PING_LEVELS` order.
    #[derive(Default)]
    PingLevel {
        /// Only posts that ask somebody to act (the default).
        #[default]
        Essential => "essential",
        /// Every post that lists them.
        All => "all",
        /// Never; they are still named.
        Off => "off",
    }
}

impl PingLevel {
    pub const DEFAULT: Self = Self::Essential;

    /// A level as stored: exact spelling only (v4 `Repo.set_ping_level`).
    ///
    /// # Errors
    /// [`PingError::InvalidLevel`] quoting `value` with Python `repr`.
    pub fn parse_stored(value: &str) -> Result<Self, PingError> {
        Self::parse(value).ok_or_else(|| PingError::InvalidLevel(pytext::repr(value)))
    }
}

fn levels_text() -> String {
    PingLevel::ALL
        .iter()
        .map(|level| level.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Validate a level typed by a person: trimmed and case-folded; `None` is refused.
///
/// # Errors
/// [`PingError::InvalidLevel`] quoting the raw value in backticks, as v4.
pub fn normalise_level(value: Option<&str>) -> Result<PingLevel, PingError> {
    let level = pytext::strip(value.unwrap_or_default()).to_lowercase();
    PingLevel::parse(&level)
        .ok_or_else(|| PingError::InvalidLevel(format!("`{}`", value.unwrap_or("None"))))
}

/// Ping-level refusals, with v4's exact messages.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PingError {
    /// v4 `ValueError`; holds the already-quoted offending value.
    #[error("ping level must be one of {}, not {}", levels_text(), .0)]
    InvalidLevel(String),
    /// v4 `KeyError`: a level set for somebody not on the roster.
    // `str(KeyError(uid))` is the key's repr.
    #[error("{}", pytext::repr(.0))]
    UnknownMember(String),
}

/// One roster entry (v4 `members` row). The default is what v4 assumes for
/// somebody it has never seen: no role, not a bot, `essential`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub display_name: Option<String>,
    pub nickname: Option<String>,
    /// Holds the bossing role, so may be put on runs.
    pub has_role: bool,
    pub is_bot: bool,
    pub ping_level: PingLevel,
}

impl Member {
    /// What to call them: nickname, else display name, skipping empty text.
    pub fn name(&self) -> Option<&str> {
        [&self.nickname, &self.display_name]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .find(|name| !name.is_empty())
    }
}

/// Member and channel facts owned elsewhere (member table, gateway, config).
pub trait Directory {
    fn member(&self, user_id: &str) -> Option<Member>;

    /// Would the bot listen to, and post in, this channel?
    fn is_watched(&self, channel_id: &str) -> bool;

    /// The member's level, or [`PingLevel::DEFAULT`] for somebody never seen.
    fn ping_level(&self, user_id: &str) -> PingLevel {
        self.member(user_id)
            .map_or(PingLevel::DEFAULT, |member| member.ping_level)
    }

    /// [`Member::name`]; `None` for a stranger or a member with no name.
    fn display_name(&self, user_id: &str) -> Option<String> {
        self.member(user_id)?.name().map(str::to_owned)
    }

    /// The member's stored row when the directory holds one (aliases and
    /// all), else just their roster entry.
    fn profile(&self, user_id: &str) -> Option<MemberProfile> {
        self.member(user_id).map(|member| MemberProfile {
            member,
            ..MemberProfile::default()
        })
    }
}

/// A persisted `members` row: the roster entry plus what the portal edits
/// (aliases, reply style) and the gateway's view of the member's roles, which
/// the admin staff gate reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberProfile {
    pub member: Member,
    /// Lowercase one-word names the extractor also matches; unique across members.
    pub aliases: Vec<String>,
    /// Chosen reply-style profile key; `None` is the default.
    pub reply_style: Option<String>,
    /// Role ids from the last gateway update; cleared when they leave.
    pub roles: Vec<String>,
    /// Discord's computed Administrator permission from the last gateway update.
    pub is_guild_admin: bool,
}

/// Persistence of the member table. Reads never take the writer.
pub trait MemberStore {
    /// Every row, by user id.
    fn list_members(&self) -> impl Future<Output = Result<Vec<MemberProfile>, StoreError>> + Send;

    fn load_member(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<MemberProfile>, StoreError>> + Send;

    /// Insert or replace one row. An alias another member already holds, or an
    /// alias [`is_valid_alias`] refuses, is [`StoreError::Constraint`].
    fn put_member(
        &self,
        profile: MemberProfile,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The gateway's view of a member, in one write that never touches the
    /// portal's fields (ping level, aliases, reply style); a new row gets
    /// their defaults. A concurrent portal edit therefore always survives.
    fn apply_gateway(
        &self,
        update: GatewayMember,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// A departure: clears the role flag, roles and Administrator in one
    /// write; `false` when there is no row.
    fn member_departed(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Clear only the stored Administrator; `false` when there is no row.
    fn clear_guild_admin(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// A portal edit in one write that never touches the gateway's fields.
    /// `None` when there is no row; an alias refused as by [`Self::put_member`]
    /// is [`StoreError::Constraint`] and nothing is written.
    fn apply_portal(
        &self,
        user_id: &str,
        edit: PortalEdit,
    ) -> impl Future<Output = Result<Option<MemberProfile>, StoreError>> + Send;
}

/// Gateway-owned member fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GatewayMember {
    pub user_id: String,
    pub display_name: Option<String>,
    pub nickname: Option<String>,
    pub has_role: bool,
    pub is_bot: bool,
    pub roles: Vec<String>,
    pub is_guild_admin: bool,
}

/// Portal-owned member fields; `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortalEdit {
    pub ping_level: Option<PingLevel>,
    /// `Some(None)` clears back to the default style.
    pub reply_style: Option<Option<String>>,
    /// Appended when not already held.
    pub add_alias: Option<String>,
    /// Dropped when held (exact match); the other aliases keep their order.
    pub remove_alias: Option<String>,
}

forward_store!(MemberStore for [arc] {
    fn list_members(&self) -> Result<Vec<MemberProfile>, StoreError>;
    fn load_member(&self, user_id: &str) -> Result<Option<MemberProfile>, StoreError>;
    fn put_member(&self, profile: MemberProfile) -> Result<(), StoreError>;
    fn apply_gateway(&self, update: GatewayMember) -> Result<(), StoreError>;
    fn member_departed(&self, user_id: &str) -> Result<bool, StoreError>;
    fn clear_guild_admin(&self, user_id: &str) -> Result<bool, StoreError>;
    fn apply_portal(
        &self,
        user_id: &str,
        edit: PortalEdit,
    ) -> Result<Option<MemberProfile>, StoreError>;
});

/// An alias as stored: one lowercase word (1..=64 characters, no whitespace or
/// control characters). Loose enough for any name v4 stored; the portal's
/// stricter input rule is its own.
pub fn is_valid_alias(alias: &str) -> bool {
    (1..=64).contains(&alias.chars().count())
        && alias.to_lowercase() == alias
        && !alias.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A borrowed directory (including `&dyn Directory`) is a directory.
impl<T: Directory + ?Sized> Directory for &T {
    fn member(&self, user_id: &str) -> Option<Member> {
        (**self).member(user_id)
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        (**self).is_watched(channel_id)
    }

    fn ping_level(&self, user_id: &str) -> PingLevel {
        (**self).ping_level(user_id)
    }

    fn display_name(&self, user_id: &str) -> Option<String> {
        (**self).display_name(user_id)
    }

    fn profile(&self, user_id: &str) -> Option<MemberProfile> {
        (**self).profile(user_id)
    }
}

/// v4 `member_name`: nickname, display name, id, or `user <short id>`.
pub fn member_name(directory: &(impl Directory + ?Sized), user_id: &str) -> String {
    match directory.member(user_id) {
        Some(member) => member.name().unwrap_or(user_id).to_owned(),
        None => {
            let short = short_id(user_id);
            format!("user {}", if short.is_empty() { user_id } else { &short })
        }
    }
}

/// An in-memory [`Directory`]; persistence belongs to the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roster {
    members: BTreeMap<String, Member>,
    watched: BTreeSet<String>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, member: Member) {
        self.members.insert(member.user_id.clone(), member);
    }

    pub fn get(&self, user_id: &str) -> Option<&Member> {
        self.members.get(user_id)
    }

    /// Mark a channel watched (exact id text).
    pub fn watch(&mut self, channel_id: &str) {
        self.watched.insert(channel_id.to_owned());
    }

    /// Record a member's level (v4 `Repo.set_ping_level`); the level is checked first.
    ///
    /// # Errors
    /// [`PingError::InvalidLevel`] or [`PingError::UnknownMember`].
    pub fn set_ping_level(&mut self, user_id: &str, level: &str) -> Result<PingLevel, PingError> {
        let level = PingLevel::parse_stored(level)?;
        let member = self
            .members
            .get_mut(user_id)
            .ok_or_else(|| PingError::UnknownMember(user_id.to_owned()))?;
        member.ping_level = level;
        Ok(level)
    }
}

impl Directory for Roster {
    fn member(&self, user_id: &str) -> Option<Member> {
        self.get(user_id).cloned()
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        self.watched.contains(channel_id)
    }

    fn ping_level(&self, user_id: &str) -> PingLevel {
        self.get(user_id)
            .map_or(PingLevel::DEFAULT, |member| member.ping_level)
    }

    fn display_name(&self, user_id: &str) -> Option<String> {
        self.get(user_id)?.name().map(str::to_owned)
    }
}
