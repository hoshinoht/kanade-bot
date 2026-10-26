//! Manual character roster (module `roster`, behind `modules.roster`): the
//! MapleStory characters an admin records per member and the one each party
//! member brings to a run or a weekly timing. Kept when a member leaves,
//! removed only by an explicit admin delete, audited in History's settings
//! changes, and never sent to a model: chat and extraction never import this
//! module (pinned below).

mod character;
mod party;
mod picks;
mod port;

pub use character::{
    Character, CharacterEdit, CharacterInvalid, CharacterRole, DEFAULT_WORLD, IGN_CHARS, LEVELS,
    MAX_CLASS_CHARS, MAX_WORLD_CHARS, ign_key, new_role, parse_class, parse_ign, parse_level,
    parse_role, parse_world,
};
pub use party::{PartyReads, PartyRoster};
pub use picks::{
    Basis, BossSlot, Clash, PartySlot, Pick, PickChange, PickScope, PickWrite, PlannedPick,
    PlannedSame, StoredPicks, party_slots, pick_version, plan_pick, plan_same, timing_slots,
};
pub use port::{
    CHARACTERS_SECTION, CharacterChange, CharacterRecord, CharacterStore, CharacterWrite, Planned,
    plan,
};

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every `.rs` file under `dir` that names roster character data.
    fn naming_characters(dir: &Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                naming_characters(&path, hits);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if [
                    "domain::characters",
                    "dto::characters",
                    "admin::characters",
                    "CharacterStore",
                    "CharacterRoster",
                    "roster_characters",
                    "roster_run_picks",
                    "roster_fixed_picks",
                    "roster_run_boss_picks",
                    "roster_fixed_boss_picks",
                    "StoredPicks",
                    "PartySlot",
                ]
                .iter()
                .any(|name| text.contains(name))
                {
                    hits.push(path.display().to_string());
                }
            }
        }
    }

    /// Roster data never reaches a model: the chat and extraction code that
    /// builds model prompts must not read it.
    #[test]
    fn chat_and_extraction_never_read_the_roster() {
        let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../src"));
        let mut hits = Vec::new();
        for dir in ["chat", "extract"] {
            naming_characters(&src.join(dir), &mut hits);
        }
        assert!(hits.is_empty(), "roster characters named in {hits:?}");
    }
}
