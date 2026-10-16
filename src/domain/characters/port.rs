//! The character store port: reads, and writes that record themselves in
//! History's settings changes (section [`CHARACTERS_SECTION`]) in the same
//! transaction. Outside the hash chain and never revertible.
//!
//! The rules every write keeps: an IGN is unique per world,
//! case-insensitively (retired characters included); a member with an
//! active character has exactly one active main; a new main takes the crown
//! (the old main becomes an alt in the same write); the main cannot be
//! retired, demoted or deleted while the member has other active characters.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{Character, CharacterRole, Pick, PickChange, PickScope, PickWrite};
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{RowDiff, SettingsChange};

/// History's settings section of a character write (key `character.<id>`).
pub const CHARACTERS_SECTION: &str = "characters";

/// One write. `version` is the row version the caller read: a row at
/// another version is [`CharacterWrite::Stale`] and nothing is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharacterChange {
    /// A new row at version 1 (its role from [`super::new_role`]).
    Create(Character),
    /// The row as [`Character::edited`] made it: the next version, or the
    /// row as it is (a no-op that writes and records nothing).
    Update {
        version: u32,
        after: Character,
    },
    Delete {
        id: String,
        version: u32,
    },
}

impl CharacterChange {
    /// The id of the row the change names.
    pub fn id(&self) -> &str {
        match self {
            Self::Create(row) | Self::Update { after: row, .. } => &row.id,
            Self::Delete { id, .. } => id,
        }
    }

    /// The row as it will be stored, for a create or an update.
    pub fn row(&self) -> Option<&Character> {
        match self {
            Self::Create(row) | Self::Update { after: row, .. } => Some(row),
            Self::Delete { .. } => None,
        }
    }
}

/// What a write did. Only the first three wrote anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharacterWrite {
    Created(Character),
    /// The row as stored now (unchanged by a no-op).
    Updated(Character),
    /// The row as it was.
    Deleted(Character),
    /// No such character.
    Missing,
    /// The row is at another version; the row as it is now.
    Stale(Character),
    /// Another character on the same world holds the IGN.
    IgnTaken,
    /// The write would leave the member's active characters without one
    /// main: the main kept while others are active, or (a concurrent write
    /// only) a first character that is not the main.
    MainRequired,
}

impl CharacterWrite {
    /// The write went through (a no-op update included).
    pub fn applied(&self) -> bool {
        matches!(self, Self::Created(_) | Self::Updated(_) | Self::Deleted(_))
    }
}

/// Who made a character write, and when, for its History record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterRecord {
    pub at: DateTime<Utc>,
    pub actor: Actor,
    pub surface: Surface,
}

impl CharacterRecord {
    /// The History record of a write's rows, each `(before, after)` with
    /// `None` for the side that does not exist: one row
    /// `character.<id>` each whose sides are the character's fields as
    /// JSON, an absent side empty. `None` when nothing changed.
    pub fn change(
        &self,
        rows: &[(Option<&Character>, Option<&Character>)],
    ) -> Option<SettingsChange> {
        let values: BTreeMap<String, RowDiff> = rows
            .iter()
            .filter_map(|(before, after)| {
                let id = &after.or(*before)?.id;
                let (from, to) = (fields(*before), fields(*after));
                (from != to).then(|| (format!("character.{id}"), RowDiff { from, to }))
            })
            .collect();
        (!values.is_empty()).then(|| SettingsChange {
            id: 0,
            at: self.at,
            actor: self.actor.clone(),
            surface: self.surface,
            section: CHARACTERS_SECTION.into(),
            revision: 0,
            values,
        })
    }
}

fn fields(character: Option<&Character>) -> String {
    character.map_or_else(String::new, |character| {
        json!({
            "member": character.member_id,
            "ign": character.ign,
            "world": character.world,
            "class": character.class,
            "level": character.level,
            "role": character.role.as_str(),
            "retired": character.retired_at.is_some(),
        })
        .to_string()
    })
}

/// A planned write: the outcome, the named row's two sides and the old
/// main it demotes, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub written: CharacterWrite,
    pub before: Option<Character>,
    pub after: Option<Character>,
    /// The member's previous main as `(before, after)`: now an alt.
    pub demoted: Option<(Character, Character)>,
}

impl Planned {
    /// An outcome that writes no row (a refusal or a no-op).
    fn unwritten(written: CharacterWrite) -> Self {
        Self {
            written,
            before: None,
            after: None,
            demoted: None,
        }
    }

    /// The rows to write, as `(before, after)`.
    pub fn rows(&self) -> Vec<(Option<&Character>, Option<&Character>)> {
        let mut rows = Vec::new();
        if self.before.is_some() || self.after.is_some() {
            rows.push((self.before.as_ref(), self.after.as_ref()));
        }
        if let Some((before, after)) = &self.demoted {
            rows.push((Some(before), Some(after)));
        }
        rows
    }
}

/// `change` against `current` (the stored row its id names) and `others`
/// (the member's other rows); `ign_taken` when another row on the same
/// world holds the IGN. A refusal or no-op plans no rows.
///
/// # Errors
/// [`StoreError::Constraint`] for a row that fails [`Character::check`], a
/// create over an existing id, or an update that moves the row to another
/// member, rewrites its creation or skips a version.
pub fn plan(
    change: CharacterChange,
    current: Option<Character>,
    others: &[Character],
    ign_taken: bool,
    record: &CharacterRecord,
) -> Result<Planned, StoreError> {
    let refuse = |what: &str| Err(StoreError::Constraint(format!("character {what}")));
    let (before, after) = match change {
        CharacterChange::Create(row) => {
            row.check()?;
            if current.is_some() {
                return refuse("id is taken");
            }
            if row.version != 1 {
                return refuse("starts at version 1");
            }
            (None, Some(row))
        }
        CharacterChange::Update { version, after } => {
            after.check()?;
            let Some(before) = current else {
                return Ok(Planned::unwritten(CharacterWrite::Missing));
            };
            if before.version != version {
                return Ok(Planned::unwritten(CharacterWrite::Stale(before)));
            }
            if after == before {
                return Ok(Planned::unwritten(CharacterWrite::Updated(before)));
            }
            if after.member_id != before.member_id
                || after.created_at != before.created_at
                || after.version != version + 1
            {
                return refuse("update keeps its member and creation and takes the next version");
            }
            (Some(before), Some(after))
        }
        CharacterChange::Delete { version, .. } => match current {
            None => return Ok(Planned::unwritten(CharacterWrite::Missing)),
            Some(before) if before.version != version => {
                return Ok(Planned::unwritten(CharacterWrite::Stale(before)));
            }
            Some(before) => (Some(before), None),
        },
    };
    if after.is_some() && ign_taken {
        return Ok(Planned::unwritten(CharacterWrite::IgnTaken));
    }
    let other_main = others.iter().find(|other| other.is_active_main());
    let others_active = others.iter().any(Character::is_active);
    let was_main = before.as_ref().is_some_and(Character::is_active_main);
    let mut demoted = None;
    match &after {
        Some(row) if row.is_active_main() => {
            demoted = other_main.map(|main| {
                let alt = Character {
                    role: CharacterRole::Alt,
                    ..main.clone()
                }
                .next_version(record.at, &record.actor);
                (main.clone(), alt)
            });
        }
        Some(row) if row.is_active() && other_main.is_none() => {
            return Ok(Planned::unwritten(CharacterWrite::MainRequired));
        }
        _ if was_main && others_active => {
            return Ok(Planned::unwritten(CharacterWrite::MainRequired));
        }
        _ => {}
    }
    let written = match (&before, &after) {
        (None, Some(row)) => CharacterWrite::Created(row.clone()),
        (Some(_), Some(row)) => CharacterWrite::Updated(row.clone()),
        (Some(row), None) => CharacterWrite::Deleted(row.clone()),
        (None, None) => CharacterWrite::Missing,
    };
    Ok(Planned {
        written,
        before,
        after,
        demoted,
    })
}

/// Persistence of `roster_characters`, `roster_run_picks`,
/// `roster_fixed_picks` and their per-boss tables. Reads never take the
/// writer.
pub trait CharacterStore {
    /// One member's characters, retired included, oldest first.
    fn member_characters(
        &self,
        member_id: &str,
    ) -> impl Future<Output = Result<Vec<Character>, StoreError>> + Send;

    fn character(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<Character>, StoreError>> + Send;

    /// Apply `change` under the rules above and append its History record
    /// ([`CharacterRecord::change`], a demoted main included) in one
    /// transaction. A refusal ([`CharacterWrite::applied`] false) writes
    /// nothing; a row that fails [`Character::check`] is
    /// [`StoreError::Constraint`]. A delete drops the character's run and
    /// timing picks in the same write.
    fn apply_character(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
    ) -> impl Future<Output = Result<CharacterWrite, StoreError>> + Send;

    /// The stored picks of `scope` on these targets (runs, or weekly
    /// timings; every member), per-boss picks included, by target then
    /// member (then boss).
    fn picks(
        &self,
        scope: PickScope,
        targets: &[String],
    ) -> impl Future<Output = Result<Vec<Pick>, StoreError>> + Send;

    /// Apply `change` under [`super::plan_pick`] (against the slot's stored
    /// pick and the character a set names) or, for "same for every boss",
    /// [`super::plan_same`] (against the member's per-boss picks it names),
    /// and append its History record in one transaction. A refusal
    /// ([`PickWrite::applied`] false) or a no-op writes nothing.
    fn apply_pick(
        &self,
        change: PickChange,
        record: CharacterRecord,
    ) -> impl Future<Output = Result<PickWrite, StoreError>> + Send;
}
