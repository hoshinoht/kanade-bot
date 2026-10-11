//! `characters.json`: a member's roster characters (module `roster`).

use serde::Serialize;

use super::{history::SettingsActor, iso_instant};
use crate::domain::characters::Character;

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterRow {
    pub id: String,
    pub member_id: String,
    pub ign: String,
    pub world: String,
    pub class: Option<String>,
    pub level: Option<u16>,
    #[cfg_attr(test, ts(type = "'main' | 'alt'"))]
    pub role: &'static str,
    /// When it was retired; `null` while active.
    pub retired_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Who made the last write (the create, else the last edit).
    pub updated_by: SettingsActor,
    /// Send it back as the `PATCH` precondition.
    pub version: u32,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberCharacters {
    pub member_id: String,
    /// Retired ones included, oldest first.
    pub characters: Vec<CharacterRow>,
}

pub fn row(character: &Character) -> CharacterRow {
    CharacterRow {
        id: character.id.clone(),
        member_id: character.member_id.clone(),
        ign: character.ign.clone(),
        world: character.world.clone(),
        class: character.class.clone(),
        level: character.level,
        role: character.role.as_str(),
        retired_at: character.retired_at.map(iso_instant),
        created_at: iso_instant(character.created_at),
        updated_at: iso_instant(character.updated_at),
        updated_by: SettingsActor {
            kind: character.updated_by.kind(),
            id: character.updated_by.id().to_owned(),
        },
        version: character.version,
    }
}
