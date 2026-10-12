//! `characters.json`: a member's roster characters and the character each
//! party member brings to a run (module `roster`).

use serde::Serialize;

use super::{
    history::SettingsActor,
    iso_instant,
    week::{Context, day_index, run_time},
};
use crate::domain::{
    characters::{Character, Clash, PartySlot},
    ids::short_id,
    schedule::Run,
};

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

/// A party member's candidate: one of their active characters.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterChoice {
    pub id: String,
    pub ign: String,
    pub world: String,
    pub class: Option<String>,
    pub level: Option<u16>,
    #[cfg_attr(test, ts(type = "'main' | 'alt'"))]
    pub role: &'static str,
}

/// The same character on another run this boss week for a boss this run
/// has too (the weekly clear limit). A warning only: saves go through.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterClash {
    pub character_id: String,
    pub run_id: String,
    pub short_id: String,
    /// Day index within the boss week (0 = reset day).
    pub day: u8,
    /// `null` for an own-time run.
    pub time: Option<String>,
    /// The other run's bosses this run has too, any difficulty.
    pub bosses: Vec<String>,
    /// The character is inferred on the other run, not picked.
    pub inferred: bool,
}

/// One party member's slot on a run.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunCharacterSlot {
    pub member_id: String,
    /// Their active characters, oldest first; empty when they have none.
    pub candidates: Vec<CharacterChoice>,
    /// The picked character's id.
    pub picked: Option<String>,
    /// With nothing picked, the only candidate's id (never stored).
    pub inferred: Option<String>,
    /// Send it back with the pick: 0 while nothing is picked.
    pub version: u32,
    /// Other runs with the picked or inferred character for a shared boss.
    pub warnings: Vec<CharacterClash>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunCharacters {
    pub run_id: String,
    /// In party order.
    pub slots: Vec<RunCharacterSlot>,
}

pub fn choice(character: &Character) -> CharacterChoice {
    CharacterChoice {
        id: character.id.clone(),
        ign: character.ign.clone(),
        world: character.world.clone(),
        class: character.class.clone(),
        level: character.level,
        role: character.role.as_str(),
    }
}

fn clash(ctx: &Context<'_>, week: &[Run], clash: &Clash) -> Option<CharacterClash> {
    let run = week.iter().find(|run| run.id == clash.run_id)?;
    Some(CharacterClash {
        character_id: clash.character_id.clone(),
        run_id: run.id.clone(),
        short_id: short_id(&run.id),
        day: day_index(ctx, ctx.local_date(run.week_start), run.datetime),
        time: run_time(ctx, run),
        bosses: clash.bosses.clone(),
        inferred: clash.inferred,
    })
}

/// `slot` as the API answers it; `week` holds the runs its clashes name.
pub fn run_slot(ctx: &Context<'_>, week: &[Run], slot: &PartySlot) -> RunCharacterSlot {
    RunCharacterSlot {
        member_id: slot.member_id.clone(),
        candidates: slot.candidates.iter().map(choice).collect(),
        picked: slot.picked.clone(),
        inferred: slot.inferred.clone(),
        version: slot.version,
        warnings: slot
            .clashes
            .iter()
            .filter_map(|found| clash(ctx, week, found))
            .collect(),
    }
}
