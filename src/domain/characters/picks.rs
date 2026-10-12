//! Which character a party member brings to a run (user decisions
//! 2026-10-12). A member's candidates are their active characters, at any
//! level. Only explicit picks are stored; with no pick, a member with
//! exactly one active character reads as `inferred`, recomputed on every
//! read and never written. A pick is roster data outside the history chain:
//! it saves directly (never staged in a draft) and is audited in History's
//! settings changes (section `characters`, key `pick.<run>.<member>`).
//!
//! A pick is ignored on reads while its member is not in the run's party
//! or its character is retired; deleting a character drops its picks.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{CHARACTERS_SECTION, Character, CharacterRecord};
use crate::domain::catalog::BossTable;
use crate::domain::history::Actor;
use crate::domain::schedule::{Run, RunStatus};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{RowDiff, SettingsChange};

/// One `roster_run_picks` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPick {
    pub run_id: String,
    pub member_id: String,
    pub character_id: String,
    pub picked_at: DateTime<Utc>,
    pub picked_by: Actor,
    /// 1 on the first pick, +1 on every change. A slot with no stored pick
    /// is at version 0.
    pub version: u32,
}

impl RunPick {
    /// The shape both stores keep (SQLite by CHECK as well).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("run pick {what}")));
        let chars = |text: &str| text.chars().count();
        if !(1..=64).contains(&chars(&self.run_id)) {
            return refuse("run is 1-64 characters");
        }
        if !(1..=32).contains(&chars(&self.member_id)) {
            return refuse("member is 1-32 characters");
        }
        if !(1..=64).contains(&chars(&self.character_id)) {
            return refuse("character is 1-64 characters");
        }
        if self.version < 1 {
            return refuse("version starts at 1");
        }
        Ok(())
    }
}

/// The version of a slot whose stored pick is `pick`.
pub fn pick_version(pick: Option<&RunPick>) -> u32 {
    pick.map_or(0, |pick| pick.version)
}

/// One write to a `(run, member)` slot. `version` is the slot version the
/// caller read ([`pick_version`]): another is [`PickWrite::Stale`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickChange {
    Set {
        run_id: String,
        member_id: String,
        character_id: String,
        version: u32,
    },
    Clear {
        run_id: String,
        member_id: String,
        version: u32,
    },
}

impl PickChange {
    /// The `(run, member)` slot the change names.
    pub fn slot(&self) -> (&str, &str) {
        match self {
            Self::Set {
                run_id, member_id, ..
            }
            | Self::Clear {
                run_id, member_id, ..
            } => (run_id, member_id),
        }
    }

    /// The character a set names.
    pub fn character_id(&self) -> Option<&str> {
        match self {
            Self::Set { character_id, .. } => Some(character_id),
            Self::Clear { .. } => None,
        }
    }

    fn version(&self) -> u32 {
        match self {
            Self::Set { version, .. } | Self::Clear { version, .. } => *version,
        }
    }
}

/// What a pick write did. Only the first two went through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickWrite {
    /// The pick as stored now (unchanged by a no-op).
    Picked(RunPick),
    /// The slot has no stored pick now.
    Cleared,
    /// The slot is at another version; its pick as it is now.
    Stale(Option<RunPick>),
    /// No such character.
    UnknownCharacter,
    /// The character is another member's.
    NotTheirs,
    /// The character is retired: never offered for runs.
    Retired,
}

impl PickWrite {
    /// The write went through (a no-op included).
    pub fn applied(&self) -> bool {
        matches!(self, Self::Picked(_) | Self::Cleared)
    }
}

/// A planned pick write: the outcome, the slot's two sides (both `None`
/// for a refusal or a no-op) and its History record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedPick {
    pub written: PickWrite,
    pub before: Option<RunPick>,
    pub after: Option<RunPick>,
    pub change: Option<SettingsChange>,
}

impl PlannedPick {
    fn unwritten(written: PickWrite) -> Self {
        Self {
            written,
            before: None,
            after: None,
            change: None,
        }
    }
}

/// `change` against the slot's stored pick `current` (with its character,
/// for the record) and `character`, the row a set names.
///
/// # Errors
/// [`StoreError::Constraint`] for a pick that fails [`RunPick::check`].
pub fn plan_pick(
    change: PickChange,
    current: Option<RunPick>,
    current_character: Option<&Character>,
    character: Option<&Character>,
    record: &CharacterRecord,
) -> Result<PlannedPick, StoreError> {
    if pick_version(current.as_ref()) != change.version() {
        return Ok(PlannedPick::unwritten(PickWrite::Stale(current)));
    }
    let after = match &change {
        PickChange::Clear { .. } => None,
        PickChange::Set {
            run_id,
            member_id,
            character_id,
            ..
        } => {
            let Some(character) = character.filter(|row| row.id == *character_id) else {
                return Ok(PlannedPick::unwritten(PickWrite::UnknownCharacter));
            };
            if character.member_id != *member_id {
                return Ok(PlannedPick::unwritten(PickWrite::NotTheirs));
            }
            if !character.is_active() {
                return Ok(PlannedPick::unwritten(PickWrite::Retired));
            }
            if let Some(current) = current.as_ref().filter(|c| c.character_id == *character_id) {
                return Ok(PlannedPick::unwritten(PickWrite::Picked(current.clone())));
            }
            let after = RunPick {
                run_id: run_id.clone(),
                member_id: member_id.clone(),
                character_id: character_id.clone(),
                picked_at: record.at,
                picked_by: record.actor.clone(),
                version: pick_version(current.as_ref()) + 1,
            };
            after.check()?;
            Some(after)
        }
    };
    if current.is_none() && after.is_none() {
        return Ok(PlannedPick::unwritten(PickWrite::Cleared));
    }
    let change = pick_record(
        record,
        current.as_ref().map(|pick| (pick, current_character)),
        after.as_ref().map(|pick| (pick, character)),
    );
    Ok(PlannedPick {
        written: after.clone().map_or(PickWrite::Cleared, PickWrite::Picked),
        before: current,
        after,
        change,
    })
}

/// The History record of a slot going from `before` to `after`, each with
/// its character (for the IGN); `None` when nothing changed.
fn pick_record(
    record: &CharacterRecord,
    before: Option<(&RunPick, Option<&Character>)>,
    after: Option<(&RunPick, Option<&Character>)>,
) -> Option<SettingsChange> {
    let side = |side: Option<(&RunPick, Option<&Character>)>| {
        side.map_or_else(String::new, |(pick, character)| {
            json!({
                "run": pick.run_id,
                "member": pick.member_id,
                "character": pick.character_id,
                "ign": character.map(|row| &row.ign),
            })
            .to_string()
        })
    };
    let pick = after.or(before)?.0;
    let (from, to) = (side(before), side(after));
    (from != to).then(|| SettingsChange {
        id: 0,
        at: record.at,
        actor: record.actor.clone(),
        surface: record.surface,
        section: CHARACTERS_SECTION.into(),
        revision: 0,
        values: BTreeMap::from([(
            format!("pick.{}.{}", pick.run_id, pick.member_id),
            RowDiff { from, to },
        )]),
    })
}

/// A party member's characters on one run, as reads report them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartySlot {
    pub member_id: String,
    /// Their active characters, oldest first.
    pub candidates: Vec<Character>,
    /// The stored pick, while it names one of `candidates`.
    pub picked: Option<String>,
    /// With nothing picked, the only candidate.
    pub inferred: Option<String>,
    /// The slot version a write sends back ([`pick_version`]).
    pub version: u32,
    /// The other runs this boss week the same character is on for a boss
    /// this run has too (the weekly clear limit). Never blocks a save.
    pub clashes: Vec<Clash>,
}

impl PartySlot {
    /// The character the member brings: picked, else inferred.
    pub fn bringing(&self) -> Option<&str> {
        self.picked.as_deref().or(self.inferred.as_deref())
    }
}

/// Another run with the same character for a shared boss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clash {
    pub character_id: String,
    pub run_id: String,
    /// The other run's bosses this run has too (by boss, any difficulty).
    pub bosses: Vec<String>,
    /// The character is inferred there, not picked.
    pub inferred: bool,
}

/// The week's picks by `(run, member)`.
type Picks<'a> = BTreeMap<(&'a str, &'a str), &'a RunPick>;

/// Who `member` brings to `run` given their `active` characters: the
/// picked one while it is active (`false`), else the only active one
/// (`true`). `None` off the party or with no single answer.
fn brings<'a>(
    run: &Run,
    member: &str,
    active: &'a [Character],
    picks: &Picks<'_>,
) -> Option<(&'a str, bool)> {
    if !run.participants.iter().any(|id| id == member) {
        return None;
    }
    let picked = picks
        .get(&(run.id.as_str(), member))
        .and_then(|pick| active.iter().find(|row| row.id == pick.character_id));
    match (picked, active) {
        (Some(row), _) => Some((&row.id, false)),
        (None, [only]) => Some((&only.id, true)),
        (None, _) => None,
    }
}

/// A boss as the weekly clear limit counts it: the catalog boss whatever
/// the difficulty (the catalog has no per-difficulty limit); a token the
/// catalog does not know counts as itself.
fn boss_key<'a>(catalog: &'a BossTable, token: &'a str) -> &'a str {
    catalog.split(token).map_or(token, |(_, boss)| boss.short())
}

/// `run`'s party slots in party order. `week` is every run of `run`'s boss
/// week (`run` may be among them), `characters` each party member's rows
/// (retired included) and `picks` the stored picks on `week`'s runs.
pub fn party_slots(
    run: &Run,
    week: &[Run],
    characters: &BTreeMap<String, Vec<Character>>,
    picks: &[RunPick],
    catalog: &BossTable,
) -> Vec<PartySlot> {
    let picks: Picks<'_> = picks
        .iter()
        .map(|pick| ((pick.run_id.as_str(), pick.member_id.as_str()), pick))
        .collect();
    let bosses: BTreeSet<&str> = run
        .bosses
        .iter()
        .map(|token| boss_key(catalog, token))
        .collect();
    let mut others: Vec<&Run> = week
        .iter()
        .filter(|other| {
            other.id != run.id
                && other.week_start == run.week_start
                && other.status != RunStatus::Cancelled
        })
        .collect();
    others.sort_by(|a, b| (a.datetime, &a.id).cmp(&(b.datetime, &b.id)));
    let none = Vec::new();
    run.participants
        .iter()
        .map(|member| {
            let candidates: Vec<Character> = characters
                .get(member)
                .unwrap_or(&none)
                .iter()
                .filter(|row| row.is_active())
                .cloned()
                .collect();
            let version = pick_version(picks.get(&(run.id.as_str(), member.as_str())).copied());
            let here = brings(run, member, &candidates, &picks);
            let clashes = here
                .map(|(character, _)| {
                    others
                        .iter()
                        .filter_map(|other| {
                            let shared: Vec<String> = other
                                .bosses
                                .iter()
                                .filter(|token| bosses.contains(boss_key(catalog, token)))
                                .cloned()
                                .collect();
                            let (there, inferred) = brings(other, member, &candidates, &picks)?;
                            (!shared.is_empty() && there == character).then(|| Clash {
                                character_id: character.to_owned(),
                                run_id: other.id.clone(),
                                bosses: shared,
                                inferred,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (picked, inferred) = match here {
                Some((id, false)) => (Some(id.to_owned()), None),
                Some((id, true)) => (None, Some(id.to_owned())),
                None => (None, None),
            };
            PartySlot {
                member_id: member.clone(),
                candidates,
                picked,
                inferred,
                version,
                clashes,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};
    use crate::domain::characters::CharacterRole;
    use crate::domain::schedule::RunSource;

    fn catalog() -> BossTable {
        let difficulty = |prefix: &str| DifficultySpec {
            prefix: prefix.into(),
            label: prefix.to_uppercase(),
        };
        let boss = |short: &str| BossSpec {
            short: short.into(),
            difficulties: Some(vec!["n".into(), "h".into(), "x".into()]),
            ..BossSpec::default()
        };
        BossTable::from_spec(&CatalogSpec {
            difficulties: vec![difficulty("n"), difficulty("h"), difficulty("x")],
            bosses: vec![boss("Kalos"), boss("Star"), boss("Carling")],
        })
        .unwrap()
    }

    fn week(n: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 23, 16, 0, 0).unwrap() + TimeDelta::weeks(n)
    }

    fn run(id: &str, week_n: i64, day: i64, bosses: &[&str], party: &[&str]) -> Run {
        Run {
            id: id.into(),
            fixed_run_id: None,
            channel_id: None,
            week_start: week(week_n),
            datetime: week(week_n) + TimeDelta::days(day),
            bosses: bosses.iter().map(|b| (*b).to_owned()).collect(),
            participants: party.iter().map(|p| (*p).to_owned()).collect(),
            status: RunStatus::Planned,
            source: RunSource::Amend,
            attendance: Vec::new(),
            status_pin: None,
        }
    }

    fn character(id: &str, member: &str, retired: bool) -> Character {
        Character {
            id: id.into(),
            member_id: member.into(),
            ign: format!("Ign{id}"),
            world: "msea".into(),
            class: None,
            level: None,
            role: CharacterRole::Alt,
            retired_at: retired.then(|| week(0)),
            created_at: week(0),
            updated_at: week(0),
            updated_by: Actor::admin("token"),
            version: 1,
        }
    }

    fn pick(run: &str, member: &str, character: &str) -> RunPick {
        RunPick {
            run_id: run.into(),
            member_id: member.into(),
            character_id: character.into(),
            picked_at: week(0),
            picked_by: Actor::admin("token"),
            version: 1,
        }
    }

    fn roster(rows: &[Character]) -> BTreeMap<String, Vec<Character>> {
        let mut by_member: BTreeMap<String, Vec<Character>> = BTreeMap::new();
        for row in rows {
            by_member
                .entry(row.member_id.clone())
                .or_default()
                .push(row.clone());
        }
        by_member
    }

    fn slot(run: &Run, week: &[Run], rows: &[Character], picks: &[RunPick]) -> PartySlot {
        party_slots(run, week, &roster(rows), picks, &catalog())
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn inferred_is_the_only_active_character_until_someone_picks() {
        let here = run("r-1", 0, 1, &["XKalos"], &["m"]);
        let one = [character("a", "m", false)];
        let found = slot(&here, &[], &one, &[]);
        assert_eq!((found.picked, found.inferred), (None, Some("a".into())));
        assert_eq!(found.version, 0);
        // A second active character: nothing to infer.
        let two = [character("a", "m", false), character("b", "m", false)];
        let found = slot(&here, &[], &two, &[]);
        assert_eq!((found.picked, found.inferred), (None, None));
        assert_eq!(found.candidates.len(), 2);
        // Retiring one infers the other again.
        let retired = [character("a", "m", true), character("b", "m", false)];
        let found = slot(&here, &[], &retired, &[]);
        assert_eq!(found.inferred.as_deref(), Some("b"));
        assert_eq!(
            found.candidates.len(),
            1,
            "retired characters are no candidates"
        );
        // A pick wins; a retired pick is ignored but keeps the slot version.
        let found = slot(&here, &[], &two, &[pick("r-1", "m", "b")]);
        assert_eq!((found.picked.as_deref(), found.version), (Some("b"), 1));
        let found = slot(&here, &[], &retired, &[pick("r-1", "m", "a")]);
        assert_eq!(
            (found.picked, found.inferred.as_deref(), found.version),
            (None, Some("b"), 1)
        );
        // No characters: an empty slot.
        let found = slot(&here, &[], &[], &[]);
        assert!(found.candidates.is_empty() && found.bringing().is_none());
    }

    #[test]
    fn a_character_on_another_run_of_a_shared_boss_this_boss_week_warns() {
        let here = run("r-1", 0, 5, &["HCarling", "HStar"], &["m"]);
        let rows = [character("a", "m", false)];
        let star = run("r-2", 0, 2, &["NStar"], &["m", "o"]);
        let kalos = run("r-3", 0, 1, &["XKalos"], &["m"]);
        let next_week = run("r-4", 1, 2, &["HStar"], &["m"]);
        let mut cancelled = run("r-5", 0, 3, &["HCarling"], &["m"]);
        cancelled.status = RunStatus::Cancelled;
        let not_there = run("r-6", 0, 4, &["HCarling"], &["o"]);
        let week = [here.clone(), star, kalos, next_week, cancelled, not_there];
        let found = slot(&here, &week, &rows, &[]);
        assert_eq!(
            found.clashes,
            [Clash {
                character_id: "a".into(),
                run_id: "r-2".into(),
                bosses: vec!["NStar".into()],
                inferred: true,
            }],
            "the same boss at another difficulty only, this boss week only"
        );
        // With two characters and another picked on r-2, no clash.
        let rows = [character("a", "m", false), character("b", "m", false)];
        let picks = [pick("r-1", "m", "a"), pick("r-2", "m", "b")];
        assert!(slot(&here, &week, &rows, &picks).clashes.is_empty());
        // Picked on both: a clash that says so.
        let picks = [pick("r-1", "m", "a"), pick("r-2", "m", "a")];
        let found = slot(&here, &week, &rows, &picks);
        assert_eq!(found.clashes.len(), 1);
        assert!(!found.clashes[0].inferred);
        // A pick left behind by a member who left r-2's party is ignored.
        let mut left = week.clone();
        left[1].participants = vec!["o".into()];
        assert!(slot(&here, &left, &rows, &picks).clashes.is_empty());
    }
}
