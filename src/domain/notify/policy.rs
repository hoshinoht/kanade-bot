//! Post kinds, the one predicate deciding who may be notified, and the
//! quiet-mode gate.

use crate::domain::members::PingLevel;

/// What a post is for. Unrecognised kinds are kept, and treated as informational
/// so a typo costs a missing ping rather than a burst of notifications.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PingKind {
    DayOf,
    Countdown,
    Proposal,
    Decline,
    Amend,
    Status,
    Swap,
    Fixed,
    Digest,
    Retract,
    Rescan,
    Test,
    Unknown(String),
}

impl PingKind {
    /// Posts that ask somebody to act: everyone listed bar `off` is notified.
    pub const ESSENTIAL: &[Self] = &[Self::DayOf, Self::Countdown, Self::Proposal, Self::Decline];
    /// Posts that report a decision: only `all` members are notified.
    pub const INFORMATIONAL: &[Self] = &[
        Self::Amend,
        Self::Status,
        Self::Swap,
        Self::Fixed,
        Self::Digest,
        Self::Retract,
        Self::Rescan,
        Self::Test,
    ];

    pub fn parse(value: &str) -> Self {
        Self::ESSENTIAL
            .iter()
            .chain(Self::INFORMATIONAL)
            .find(|kind| kind.as_str() == value)
            .cloned()
            .unwrap_or_else(|| Self::Unknown(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::DayOf => "day_of",
            Self::Countdown => "countdown",
            Self::Proposal => "proposal",
            Self::Decline => "decline",
            Self::Amend => "amend",
            Self::Status => "status",
            Self::Swap => "swap",
            Self::Fixed => "fixed",
            Self::Digest => "digest",
            Self::Retract => "retract",
            Self::Rescan => "rescan",
            Self::Test => "test",
            Self::Unknown(kind) => kind,
        }
    }

    pub fn is_essential(&self) -> bool {
        Self::ESSENTIAL.contains(self)
    }
}

/// The whole mention policy: `off` never, `all` always, `essential` only for
/// essential kinds.
pub fn wants_mention(level: PingLevel, kind: &PingKind) -> bool {
    match level {
        PingLevel::Off => false,
        PingLevel::All => true,
        PingLevel::Essential => kind.is_essential(),
    }
}

/// The mention allow-list a post goes out with. Roles and `@everyone` are never
/// allowed; [`AllowedMentions::None`] also clears the replied-to user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowedMentions {
    None,
    Users(Vec<String>),
}

impl AllowedMentions {
    /// The users that may be notified, empty for [`AllowedMentions::None`].
    pub fn users(&self) -> &[String] {
        match self {
            Self::None => &[],
            Self::Users(users) => users,
        }
    }

    /// Discord's `replied_user` flag: on unless everything is cleared.
    pub fn replied_user(&self) -> bool {
        matches!(self, Self::Users(_))
    }
}

/// The quiet-mode gate every post passes (v4 `BossBot._prepared`).
///
/// An explicit `mention_users` (the digest passes an empty list) overrides the
/// card's own list; quiet mode clears everything.
pub fn allowed_mentions(
    card_mentions: &[String],
    mention_users: Option<&[String]>,
    quiet_mode: bool,
) -> AllowedMentions {
    if quiet_mode {
        return AllowedMentions::None;
    }
    AllowedMentions::Users(mention_users.unwrap_or(card_mentions).to_vec())
}
