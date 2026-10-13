//! `characters.json`: a member's roster characters and the character each
//! party member brings to a run or a weekly timing (module `roster`).

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

/// The create's answer: the row and, with module `character_data`, the
/// member's link request the create asked for.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterCreated {
    #[serde(flatten)]
    pub row: CharacterRow,
    /// `pending`: the member was asked to link it (a MapleSEA character
    /// with the module on and a Nexon key); `failed`: that request could
    /// not be made (the character is saved); `null`: none was asked.
    #[cfg_attr(test, ts(type = "'pending' | 'failed' | null"))]
    pub link: Option<&'static str>,
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
    /// Why the character is on the other run.
    #[cfg_attr(test, ts(type = "'run_pick' | 'timing_pick' | 'main' | 'only'"))]
    pub basis: &'static str,
}

/// One party member's slot on a run or a weekly timing.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunCharacterSlot {
    pub member_id: String,
    /// Their active characters, oldest first; empty when they have none.
    pub candidates: Vec<CharacterChoice>,
    /// How many retired characters they have (never offered).
    pub retired: u32,
    /// This slot's own stored pick (the run's on a run, the timing's on a
    /// timing) while it names a candidate.
    pub picked: Option<String>,
    /// When `picked` was picked; `null` without it.
    pub picked_at: Option<String>,
    /// Who picked `picked`; `null` without it.
    pub picked_by: Option<SettingsActor>,
    /// The character the member brings: on a run its pick, else its
    /// timing's pick, else their main, else their only active character;
    /// on a timing its pick, else the main, else the only one. `null` with
    /// none (no characters, or several and no main). Never stored.
    pub brings: Option<String>,
    /// Where `brings` comes from; `null` with nothing brought.
    #[cfg_attr(test, ts(type = "'run_pick' | 'timing_pick' | 'main' | 'only' | null"))]
    pub basis: Option<&'static str>,
    /// Send it back with the pick: 0 while nothing is picked.
    pub version: u32,
    /// Other runs with the character brought here for a shared boss (always
    /// empty on a timing).
    pub warnings: Vec<CharacterClash>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunCharacters {
    pub run_id: String,
    /// In party order.
    pub slots: Vec<RunCharacterSlot>,
}

/// A weekly timing's party slots (modules `roster` and `character_data`).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct TimingCharacters {
    pub fixed_run_id: String,
    /// In party order (the saved party).
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
        basis: clash.basis.as_str(),
    })
}

/// `slot` without its clashes, as a timing answers it.
pub fn slot(slot: &PartySlot) -> RunCharacterSlot {
    RunCharacterSlot {
        member_id: slot.member_id.clone(),
        candidates: slot.candidates.iter().map(choice).collect(),
        retired: u32::try_from(slot.retired).unwrap_or(u32::MAX),
        picked: slot.pick.as_ref().map(|pick| pick.character_id.clone()),
        picked_at: slot.pick.as_ref().map(|pick| iso_instant(pick.picked_at)),
        picked_by: slot.pick.as_ref().map(|pick| SettingsActor {
            kind: pick.picked_by.kind(),
            id: pick.picked_by.id().to_owned(),
        }),
        brings: slot.bringing().map(str::to_owned),
        basis: slot.brings.as_ref().map(|(_, basis)| basis.as_str()),
        version: slot.version,
        warnings: Vec::new(),
    }
}

/// `party` as a run answers it; `week` holds the runs its clashes name.
pub fn run_slot(ctx: &Context<'_>, week: &[Run], party: &PartySlot) -> RunCharacterSlot {
    RunCharacterSlot {
        warnings: party
            .clashes
            .iter()
            .filter_map(|found| clash(ctx, week, found))
            .collect(),
        ..slot(party)
    }
}
