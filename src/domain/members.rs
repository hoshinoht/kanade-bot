//! The one guild-member model the scheduler and the notifier share: who is on
//! the roster, what to call them, whether they may join runs, how loudly they
//! want to be pinged, and which channels the bot watches.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::domain::ids::short_id;
use crate::domain::pytext;

/// A member's mention preference, in v4's `PING_LEVELS` order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PingLevel {
    /// Only posts that ask somebody to act (the default).
    #[default]
    Essential,
    /// Every post that lists them.
    All,
    /// Never; they are still named.
    Off,
}

impl PingLevel {
    pub const ALL: &[Self] = &[Self::Essential, Self::All, Self::Off];
    pub const DEFAULT: Self = Self::Essential;

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Essential => "essential",
            Self::All => "all",
            Self::Off => "off",
        }
    }

    fn exact(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|level| level.as_str() == value)
    }

    /// A level as stored: exact spelling only (v4 `Repo.set_ping_level`).
    ///
    /// # Errors
    /// [`PingError::InvalidLevel`] quoting `value` with Python `repr`.
    pub fn parse_stored(value: &str) -> Result<Self, PingError> {
        Self::exact(value).ok_or_else(|| PingError::InvalidLevel(pytext::repr(value)))
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
    PingLevel::exact(&level)
        .ok_or_else(|| PingError::InvalidLevel(format!("`{}`", value.unwrap_or("None"))))
}

/// Ping-level refusals, with v4's exact messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PingError {
    /// v4 `ValueError`; holds the already-quoted offending value.
    InvalidLevel(String),
    /// v4 `KeyError`: a level set for somebody not on the roster.
    UnknownMember(String),
}

impl fmt::Display for PingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLevel(shown) => {
                write!(
                    f,
                    "ping level must be one of {}, not {shown}",
                    levels_text()
                )
            }
            // `str(KeyError(uid))` is the key's repr.
            Self::UnknownMember(user_id) => f.write_str(&pytext::repr(user_id)),
        }
    }
}

impl std::error::Error for PingError {}

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
