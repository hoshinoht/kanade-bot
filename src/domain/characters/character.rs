//! A roster character and the input rules the admin API applies to it.

use std::fmt;

use chrono::{DateTime, Utc};

use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

/// The world a character is on when none is named.
pub const DEFAULT_WORLD: &str = "msea";
/// IGN length, in characters (MapleStory's naming rule).
pub const IGN_CHARS: std::ops::RangeInclusive<usize> = 4..=12;
pub const MAX_CLASS_CHARS: usize = 32;
pub const MAX_WORLD_CHARS: usize = 16;
pub const LEVELS: std::ops::RangeInclusive<u16> = 1..=300;

/// A member's main character, or one of their alts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CharacterRole {
    Main,
    Alt,
}

impl CharacterRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Alt => "alt",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "main" => Some(Self::Main),
            "alt" => Some(Self::Alt),
            _ => None,
        }
    }
}

/// One `roster_characters` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Character {
    pub id: String,
    /// The Discord user id of the member who plays it.
    pub member_id: String,
    pub ign: String,
    pub world: String,
    pub class: Option<String>,
    pub level: Option<u16>,
    pub role: CharacterRole,
    /// Set while the character is retired; a retired main frees the slot.
    pub retired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who made the last effective write (the create, else the last edit).
    pub updated_by: Actor,
    /// 1 on create, +1 on every effective edit.
    pub version: u32,
}

/// The fields an edit may set; `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterEdit {
    pub ign: Option<String>,
    pub world: Option<String>,
    /// `Some(None)` clears the class.
    pub class: Option<Option<String>>,
    /// `Some(None)` clears the level.
    pub level: Option<Option<u16>>,
    /// `None` keeps the role, except that an active character takes the
    /// main when the member has no other active main, and a restored main
    /// becomes an alt when another active main exists.
    pub role: Option<CharacterRole>,
    /// `true` retires (keeping the first retirement time), `false` restores.
    pub retired: Option<bool>,
}

/// The role a new character gets: the main while the member has no active
/// main among `others` (their first character), else the one asked for
/// (a main takes the crown from the current one), else an alt.
pub fn new_role(asked: Option<CharacterRole>, others: &[Character]) -> CharacterRole {
    if others.iter().any(Character::is_active_main) {
        asked.unwrap_or(CharacterRole::Alt)
    } else {
        CharacterRole::Main
    }
}

impl Character {
    /// The IGN as uniqueness compares it: case-insensitive.
    pub fn ign_key(&self) -> String {
        ign_key(&self.ign)
    }

    pub fn is_active(&self) -> bool {
        self.retired_at.is_none()
    }

    /// Holds the member's one active main slot.
    pub fn is_active_main(&self) -> bool {
        self.role == CharacterRole::Main && self.is_active()
    }

    /// Equal apart from `version`, `updated_at` and `updated_by`.
    pub fn same_content(&self, other: &Self) -> bool {
        (
            &self.id,
            &self.member_id,
            &self.ign,
            &self.world,
            &self.class,
            self.level,
            self.role,
            self.retired_at,
            self.created_at,
        ) == (
            &other.id,
            &other.member_id,
            &other.ign,
            &other.world,
            &other.class,
            other.level,
            other.role,
            other.retired_at,
            other.created_at,
        )
    }

    /// This row stamped as the next version, written by `by` at `at`.
    pub fn next_version(mut self, at: DateTime<Utc>, by: &Actor) -> Self {
        self.version += 1;
        self.updated_at = at;
        self.updated_by = by.clone();
        self
    }

    /// This row after `edit` by `by` at `at`, given the member's `others`
    /// characters: the next version, or this row itself when nothing
    /// changes.
    pub fn edited(
        &self,
        edit: &CharacterEdit,
        others: &[Character],
        at: DateTime<Utc>,
        by: &Actor,
    ) -> Self {
        let mut next = self.clone();
        if let Some(ign) = &edit.ign {
            next.ign.clone_from(ign);
        }
        if let Some(world) = &edit.world {
            next.world.clone_from(world);
        }
        if let Some(class) = &edit.class {
            next.class.clone_from(class);
        }
        if let Some(level) = edit.level {
            next.level = level;
        }
        match edit.retired {
            Some(true) if next.retired_at.is_none() => next.retired_at = Some(at),
            Some(false) => next.retired_at = None,
            _ => {}
        }
        match edit.role {
            Some(role) => next.role = role,
            None if next.is_active() => {
                let other_main = others.iter().any(Character::is_active_main);
                next.role = match (next.role, other_main) {
                    (CharacterRole::Main, true) => CharacterRole::Alt,
                    (CharacterRole::Alt, false) => CharacterRole::Main,
                    (role, _) => role,
                };
            }
            None => {}
        }
        if next.same_content(self) {
            return self.clone();
        }
        next.next_version(at, by)
    }

    /// The shape both stores keep (SQLite by CHECK as well). Looser than
    /// the input rules, so those can change without a migration.
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("character {what}")));
        let chars = |text: &str| text.chars().count();
        if !(1..=64).contains(&chars(&self.id)) {
            return refuse("id is 1-64 characters");
        }
        if !(1..=32).contains(&chars(&self.member_id)) {
            return refuse("member is 1-32 characters");
        }
        if !(1..=32).contains(&chars(&self.ign)) {
            return refuse("IGN is 1-32 characters");
        }
        if !(1..=MAX_WORLD_CHARS).contains(&chars(&self.world)) {
            return refuse("world is 1-16 characters");
        }
        if self
            .class
            .as_deref()
            .is_some_and(|class| !(1..=MAX_CLASS_CHARS).contains(&chars(class)))
        {
            return refuse("class is 1-32 characters");
        }
        if self.level.is_some_and(|level| !LEVELS.contains(&level)) {
            return refuse("level is 1-300");
        }
        if self.version == 0 {
            return refuse("version starts at 1");
        }
        if !(1..=320).contains(&self.updated_by.id().len()) {
            return refuse("writer is 1-320 bytes");
        }
        // As SQLite's `to_iso`: instants outside years 1..=9999 are refused.
        for at in [
            Some(&self.created_at),
            Some(&self.updated_at),
            self.retired_at.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            crate::domain::time::to_iso(at)
                .map_err(|error| StoreError::Constraint(format!("{at:?}: {error}")))?;
        }
        Ok(())
    }
}

/// SQLite's `lower()`: ASCII only, as the IGN rule allows only ASCII.
pub fn ign_key(ign: &str) -> String {
    ign.to_ascii_lowercase()
}

/// A field the input rules refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterInvalid {
    Ign,
    World,
    Class,
    Level,
    Role,
}

impl fmt::Display for CharacterInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ign => "An IGN is 4-12 letters or digits.",
            Self::World => "A world is 1-16 lowercase letters, digits or '-'.",
            Self::Class => "A class is at most 32 characters.",
            Self::Level => "A level is 1-300.",
            Self::Role => "A role is main or alt.",
        })
    }
}

impl std::error::Error for CharacterInvalid {}

/// An IGN as typed: trimmed, 4-12 ASCII letters or digits, case kept.
pub fn parse_ign(text: &str) -> Result<String, CharacterInvalid> {
    let ign = text.trim();
    (IGN_CHARS.contains(&ign.len()) && ign.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .then(|| ign.to_owned())
        .ok_or(CharacterInvalid::Ign)
}

/// A world key: trimmed and lowercased; empty is the default world.
pub fn parse_world(text: &str) -> Result<String, CharacterInvalid> {
    let world = text.trim().to_ascii_lowercase();
    if world.is_empty() {
        return Ok(DEFAULT_WORLD.to_owned());
    }
    (world.len() <= MAX_WORLD_CHARS
        && world
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    .then_some(world)
    .ok_or(CharacterInvalid::World)
}

/// A class as typed: trimmed; empty clears it.
pub fn parse_class(text: &str) -> Result<Option<String>, CharacterInvalid> {
    let class = text.trim();
    if class.is_empty() {
        return Ok(None);
    }
    (class.chars().count() <= MAX_CLASS_CHARS && !class.chars().any(char::is_control))
        .then(|| Some(class.to_owned()))
        .ok_or(CharacterInvalid::Class)
}

pub fn parse_level(level: i64) -> Result<u16, CharacterInvalid> {
    u16::try_from(level)
        .ok()
        .filter(|level| LEVELS.contains(level))
        .ok_or(CharacterInvalid::Level)
}

pub fn parse_role(text: &str) -> Result<CharacterRole, CharacterInvalid> {
    CharacterRole::parse(text).ok_or(CharacterInvalid::Role)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 11, hour, 0, 0).unwrap()
    }

    fn admin() -> Actor {
        Actor::admin("token")
    }

    fn sample() -> Character {
        Character {
            id: "c-1".into(),
            member_id: "1001".into(),
            ign: "Wisteria".into(),
            world: DEFAULT_WORLD.into(),
            class: None,
            level: None,
            role: CharacterRole::Main,
            retired_at: None,
            created_at: at(1),
            updated_at: at(1),
            updated_by: admin(),
            version: 1,
        }
    }

    #[test]
    fn igns_are_four_to_twelve_ascii_letters_or_digits() {
        for good in ["Abcd", " Wisteria9 ", "abcdefghijkl", "1234"] {
            assert_eq!(parse_ign(good), Ok(good.trim().to_owned()), "{good:?}");
        }
        for bad in [
            "abc",
            "abcdefghijklm",
            "two words",
            "Ünïcode",
            "dash-ed",
            "",
            "a_bc",
        ] {
            assert_eq!(parse_ign(bad), Err(CharacterInvalid::Ign), "{bad:?}");
        }
        assert_eq!(ign_key("WiStErIa"), "wisteria");
    }

    #[test]
    fn class_world_level_and_role_rules() {
        assert_eq!(parse_class("  "), Ok(None));
        assert_eq!(parse_class(" Bishop "), Ok(Some("Bishop".into())));
        assert_eq!(parse_class(&"x".repeat(33)), Err(CharacterInvalid::Class));
        assert_eq!(parse_class("a\nb"), Err(CharacterInvalid::Class));
        assert_eq!(parse_world(""), Ok("msea".into()));
        assert_eq!(parse_world(" MSEA "), Ok("msea".into()));
        assert_eq!(parse_world("a b"), Err(CharacterInvalid::World));
        assert_eq!(parse_level(1), Ok(1));
        assert_eq!(parse_level(300), Ok(300));
        for bad in [0, 301, -1, i64::MAX] {
            assert_eq!(parse_level(bad), Err(CharacterInvalid::Level));
        }
        assert_eq!(parse_role("main"), Ok(CharacterRole::Main));
        assert_eq!(parse_role("Main"), Err(CharacterInvalid::Role));
    }

    #[test]
    fn an_edit_bumps_the_version_only_when_something_changes() {
        let row = sample();
        let edit = |edit: CharacterEdit, hour| row.edited(&edit, &[], at(hour), &admin());
        let same = edit(
            CharacterEdit {
                ign: Some("Wisteria".into()),
                retired: Some(false),
                ..CharacterEdit::default()
            },
            2,
        );
        assert_eq!(same, row, "a no-op edit is the row itself");
        let retired = edit(
            CharacterEdit {
                retired: Some(true),
                level: Some(Some(260)),
                ..CharacterEdit::default()
            },
            2,
        );
        assert_eq!(
            (
                retired.version,
                retired.updated_at,
                retired.retired_at,
                retired.level
            ),
            (2, at(2), Some(at(2)), Some(260))
        );
        let again = retired.edited(
            &CharacterEdit {
                retired: Some(true),
                ..CharacterEdit::default()
            },
            &[],
            at(3),
            &Actor::admin("other"),
        );
        assert_eq!(again, retired, "retiring twice keeps the first time");
    }

    #[test]
    fn roles_follow_the_members_main() {
        let main = sample();
        let alt = Character {
            id: "c-2".into(),
            ign: "Heather".into(),
            role: CharacterRole::Alt,
            ..sample()
        };
        assert_eq!(new_role(Some(CharacterRole::Alt), &[]), CharacterRole::Main);
        let one = std::slice::from_ref::<Character>;
        assert_eq!(new_role(None, one(&main)), CharacterRole::Alt);
        assert_eq!(
            new_role(Some(CharacterRole::Main), one(&main)),
            CharacterRole::Main
        );
        let retired_main = Character {
            retired_at: Some(at(1)),
            ..main.clone()
        };
        assert_eq!(new_role(None, one(&retired_main)), CharacterRole::Main);
        let restore = CharacterEdit {
            retired: Some(false),
            ..CharacterEdit::default()
        };
        let back = retired_main.edited(&restore, one(&alt), at(2), &admin());
        assert_eq!(
            back.role,
            CharacterRole::Main,
            "no other main: it keeps the crown"
        );
        let promoted_alt = Character {
            role: CharacterRole::Main,
            ..alt.clone()
        };
        let back = retired_main.edited(&restore, &[promoted_alt], at(2), &admin());
        assert_eq!(
            back.role,
            CharacterRole::Alt,
            "another main: it comes back an alt"
        );
        let edited = alt.edited(
            &CharacterEdit {
                level: Some(Some(200)),
                ..CharacterEdit::default()
            },
            &[],
            at(2),
            &admin(),
        );
        assert_eq!(
            edited.role,
            CharacterRole::Main,
            "an active member always has a main"
        );
    }

    #[test]
    fn the_store_shape_is_looser_than_the_input_rules() {
        let mut row = sample();
        assert!(row.check().is_ok());
        row.ign = "x".repeat(20);
        assert!(row.check().is_ok(), "an IGN rule change needs no migration");
        row.level = Some(301);
        assert!(row.check().is_err());
        row.level = None;
        row.version = 0;
        assert!(row.check().is_err());
    }
}
