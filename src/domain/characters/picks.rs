//! Which character a party member brings to a run or a weekly timing (user
//! decisions 2026-10-12, 2026-10-13). A member's candidates are their
//! active characters, at any level. Only explicit picks are stored, per run
//! (`roster_run_picks`) and per weekly timing (`roster_fixed_picks`); what
//! a member brings is resolved on every read and never written, in this
//! order: the run's own pick (runs only), the timing's pick (read live by
//! every run made from the timing), their roster main, their only active
//! character, else nothing ([`Basis`]). A pick is roster data outside the
//! history chain: it saves directly (never staged in a draft) and is
//! audited in History's settings changes (section `characters`, key
//! `pick.<run>.<member>` or `fixedpick.<fixed>.<member>`).
//!
//! A pick is ignored on reads while its member is not in the party (the
//! run's, or the timing's as saved), its character is retired or its
//! timing is retired (its rows stay, so undoing the retire brings them
//! back); deleting a character drops its picks.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{CHARACTERS_SECTION, Character, CharacterRecord, CharacterRole};
use crate::domain::catalog::BossTable;
use crate::domain::history::Actor;
use crate::domain::schedule::{FixedRun, Run, RunStatus};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{RowDiff, SettingsChange};

/// What a pick is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PickScope {
    /// One run (`roster_run_picks`).
    Run,
    /// One weekly timing (`roster_fixed_picks`), inherited by its runs.
    Timing,
}

impl PickScope {
    /// The History key's prefix.
    fn key(self) -> &'static str {
        match self {
            Self::Run => "pick",
            Self::Timing => "fixedpick",
        }
    }

    /// The target's field in a History side.
    fn field(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Timing => "fixed",
        }
    }
}

/// One stored pick: a `roster_run_picks` or `roster_fixed_picks` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub scope: PickScope,
    /// The run id, or the weekly timing's id.
    pub target: String,
    pub member_id: String,
    pub character_id: String,
    pub picked_at: DateTime<Utc>,
    pub picked_by: Actor,
    /// 1 on the first pick, +1 on every change. A slot with no stored pick
    /// is at version 0.
    pub version: u32,
}

impl Pick {
    /// The persisted shape, also enforced by SQLite CHECK constraints.
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("pick {what}")));
        let chars = |text: &str| text.chars().count();
        if !(1..=64).contains(&chars(&self.target)) {
            return refuse(match self.scope {
                PickScope::Run => "run is 1-64 characters",
                PickScope::Timing => "timing is 1-64 characters",
            });
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
pub fn pick_version(pick: Option<&Pick>) -> u32 {
    pick.map_or(0, |pick| pick.version)
}

/// One write to a `(scope, target, member)` slot. `version` is the slot
/// version the caller read ([`pick_version`]): another is
/// [`PickWrite::Stale`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickChange {
    Set {
        scope: PickScope,
        target: String,
        member_id: String,
        character_id: String,
        version: u32,
    },
    Clear {
        scope: PickScope,
        target: String,
        member_id: String,
        version: u32,
    },
}

impl PickChange {
    /// The `(scope, target, member)` slot the change names.
    pub fn slot(&self) -> (PickScope, &str, &str) {
        match self {
            Self::Set {
                scope,
                target,
                member_id,
                ..
            }
            | Self::Clear {
                scope,
                target,
                member_id,
                ..
            } => (*scope, target, member_id),
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
    Picked(Pick),
    /// The slot has no stored pick now.
    Cleared,
    /// The slot is at another version; its pick as it is now.
    Stale(Option<Pick>),
    /// No such character.
    UnknownCharacter,
    /// The character is another member's.
    NotTheirs,
    /// The character is retired: never offered.
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
    pub before: Option<Pick>,
    pub after: Option<Pick>,
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
/// [`StoreError::Constraint`] for a pick that fails [`Pick::check`].
pub fn plan_pick(
    change: PickChange,
    current: Option<Pick>,
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
            scope,
            target,
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
            let after = Pick {
                scope: *scope,
                target: target.clone(),
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
    before: Option<(&Pick, Option<&Character>)>,
    after: Option<(&Pick, Option<&Character>)>,
) -> Option<SettingsChange> {
    let side = |side: Option<(&Pick, Option<&Character>)>| {
        side.map_or_else(String::new, |(pick, character)| {
            json!({
                pick.scope.field(): pick.target,
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
            format!("{}.{}.{}", pick.scope.key(), pick.target, pick.member_id),
            RowDiff { from, to },
        )]),
    })
}

spelled! {
    /// Where the character a member brings comes from, first match wins.
    Basis {
        /// The run's own pick (runs only).
        RunPick => "run_pick",
        /// The weekly timing's pick (on a run: inherited from its timing).
        TimingPick => "timing_pick",
        /// Their roster main.
        Main => "main",
        /// Their only active character, with no main.
        Only => "only",
    }
}

/// A party member's characters on one run or timing, as reads report them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartySlot {
    pub member_id: String,
    /// Their active characters, oldest first.
    pub candidates: Vec<Character>,
    /// How many retired characters they have (never offered).
    pub retired: usize,
    /// The slot's own stored pick (the run's on a run, the timing's on a
    /// timing), while it names one of `candidates`.
    pub pick: Option<Pick>,
    /// The character they bring and why; `None` with no single answer.
    pub brings: Option<(String, Basis)>,
    /// The slot version a write sends back ([`pick_version`]).
    pub version: u32,
    /// The other runs this boss week the same character is on for a boss
    /// this run has too (the weekly clear limit); never on a timing. Never
    /// blocks a save.
    pub clashes: Vec<Clash>,
}

impl PartySlot {
    /// The id of the character the member brings.
    pub fn bringing(&self) -> Option<&str> {
        self.brings.as_ref().map(|(id, _)| id.as_str())
    }

    /// The character the member brings and why.
    pub fn brought(&self) -> Option<(&Character, Basis)> {
        let (id, basis) = self.brings.as_ref()?;
        let character = self.candidates.iter().find(|row| row.id == *id)?;
        Some((character, *basis))
    }
}

/// Another run with the same character for a shared boss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clash {
    pub character_id: String,
    pub run_id: String,
    /// The other run's bosses this run has too (by boss, any difficulty).
    pub bosses: Vec<String>,
    /// Why the character is on the other run.
    pub basis: Basis,
}

/// The stored picks reads resolve against: run picks by `(run, member)`,
/// timing picks by `(timing, member)`, and the party of every timing that
/// still exists (a timing pick counts only for a member in it).
pub struct StoredPicks<'a> {
    runs: BTreeMap<(&'a str, &'a str), &'a Pick>,
    timings: BTreeMap<(&'a str, &'a str), &'a Pick>,
    parties: BTreeMap<&'a str, &'a [String]>,
}

impl<'a> StoredPicks<'a> {
    /// `picks` of both scopes; `timings` every weekly timing there is (a
    /// retired one is gone, and so are its picks on reads).
    pub fn new(picks: &'a [Pick], timings: &'a [FixedRun]) -> Self {
        let mut runs = BTreeMap::new();
        let mut timing_picks = BTreeMap::new();
        for pick in picks {
            let slot = (pick.target.as_str(), pick.member_id.as_str());
            match pick.scope {
                PickScope::Run => runs.insert(slot, pick),
                PickScope::Timing => timing_picks.insert(slot, pick),
            };
        }
        Self {
            runs,
            timings: timing_picks,
            parties: timings
                .iter()
                .map(|fixed| (fixed.id.as_str(), fixed.participants.as_slice()))
                .collect(),
        }
    }

    /// The stored row of a slot, whatever it names (for its version).
    pub fn row(&self, scope: PickScope, target: &str, member: &str) -> Option<&'a Pick> {
        let picks = match scope {
            PickScope::Run => &self.runs,
            PickScope::Timing => &self.timings,
        };
        picks.get(&(target, member)).copied()
    }

    /// The timing's pick for `member` while the timing exists and they are
    /// in its saved party.
    fn timing(&self, fixed: &str, member: &str) -> Option<&'a Pick> {
        self.parties
            .get(fixed)
            .filter(|party| party.iter().any(|id| id == member))?;
        self.row(PickScope::Timing, fixed, member)
    }
}

/// The first of `picks` that names one of `active`, else the main, else the
/// only active character.
fn resolve<'c>(
    active: &'c [Character],
    picks: &[(Option<&Pick>, Basis)],
) -> Option<(&'c Character, Basis)> {
    picks
        .iter()
        .find_map(|(pick, basis)| {
            let pick = (*pick)?;
            let row = active.iter().find(|row| row.id == pick.character_id)?;
            Some((row, *basis))
        })
        .or_else(|| {
            active
                .iter()
                .find(|row| row.role == CharacterRole::Main)
                .map(|main| (main, Basis::Main))
        })
        .or(match active {
            [only] => Some((only, Basis::Only)),
            _ => None,
        })
}

/// Who `member` brings to `run` given their `active` characters. `None`
/// off the party or with no single answer.
fn run_brings<'c>(
    run: &Run,
    member: &str,
    active: &'c [Character],
    picks: &StoredPicks<'_>,
) -> Option<(&'c Character, Basis)> {
    if !run.participants.iter().any(|id| id == member) {
        return None;
    }
    let timing = run
        .fixed_run_id
        .as_deref()
        .and_then(|fixed| picks.timing(fixed, member));
    resolve(
        active,
        &[
            (picks.row(PickScope::Run, &run.id, member), Basis::RunPick),
            (timing, Basis::TimingPick),
        ],
    )
}

/// A boss as the weekly clear limit counts it: the catalog boss whatever
/// the difficulty (the catalog has no per-difficulty limit); a token the
/// catalog does not know counts as itself.
fn boss_key<'a>(catalog: &'a BossTable, token: &'a str) -> &'a str {
    catalog.split(token).map_or(token, |(_, boss)| boss.short())
}

/// A member's active characters and how many are retired.
fn roster_of(
    characters: &BTreeMap<String, Vec<Character>>,
    member: &str,
) -> (Vec<Character>, usize) {
    let rows = characters.get(member).map_or(&[][..], Vec::as_slice);
    let active: Vec<Character> = rows.iter().filter(|row| row.is_active()).cloned().collect();
    let retired = rows.len() - active.len();
    (active, retired)
}

/// The slot's own pick while it names a candidate.
fn own_pick(own: Option<&Pick>, candidates: &[Character]) -> Option<Pick> {
    own.filter(|pick| candidates.iter().any(|row| row.id == pick.character_id))
        .cloned()
}

/// `run`'s party slots in party order. `week` is every run of `run`'s boss
/// week (`run` may be among them), `characters` each party member's rows
/// (retired included) and `picks` the stored run picks on `week`'s runs and
/// the timing picks of their timings.
pub fn party_slots(
    run: &Run,
    week: &[Run],
    characters: &BTreeMap<String, Vec<Character>>,
    picks: &StoredPicks<'_>,
    catalog: &BossTable,
) -> Vec<PartySlot> {
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
    run.participants
        .iter()
        .map(|member| {
            let (candidates, retired) = roster_of(characters, member);
            let own = picks.row(PickScope::Run, &run.id, member);
            let here = run_brings(run, member, &candidates, picks);
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
                            let (there, basis) = run_brings(other, member, &candidates, picks)?;
                            (!shared.is_empty() && there.id == character.id).then(|| Clash {
                                character_id: character.id.clone(),
                                run_id: other.id.clone(),
                                bosses: shared,
                                basis,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            PartySlot {
                member_id: member.clone(),
                pick: own_pick(own, &candidates),
                brings: here.map(|(row, basis)| (row.id.clone(), basis)),
                version: pick_version(own),
                retired,
                candidates,
                clashes,
            }
        })
        .collect()
}

/// A weekly timing's slots for `party` in its order: `fixed` is the
/// timing's id (`None` for one not added yet). A member outside the
/// timing's saved party has no pick here (an unsaved party change).
pub fn timing_slots(
    fixed: Option<&str>,
    party: &[String],
    characters: &BTreeMap<String, Vec<Character>>,
    picks: &StoredPicks<'_>,
) -> Vec<PartySlot> {
    party
        .iter()
        .map(|member| {
            let (candidates, retired) = roster_of(characters, member);
            let own = fixed.and_then(|fixed| picks.row(PickScope::Timing, fixed, member));
            let live = fixed.and_then(|fixed| picks.timing(fixed, member));
            let brings = resolve(&candidates, &[(live, Basis::TimingPick)]);
            PartySlot {
                member_id: member.clone(),
                pick: own_pick(live, &candidates),
                brings: brings.map(|(row, basis)| (row.id.clone(), basis)),
                version: pick_version(own),
                retired,
                candidates,
                clashes: Vec::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeDelta, TimeZone, Weekday};

    use super::*;
    use crate::domain::attendance::AttendanceDefault;
    use crate::domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};
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

    fn timing(id: &str, party: &[&str]) -> FixedRun {
        FixedRun {
            id: id.into(),
            owner_id: "m".into(),
            channel_id: None,
            bosses: vec!["HCarling".into()],
            weekday: Weekday::Tue,
            time: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            participants: party.iter().map(|p| (*p).to_owned()).collect(),
            note: None,
            attendance_default: AttendanceDefault::default(),
            standing: Vec::new(),
            owner_pinned: false,
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

    fn main(id: &str, member: &str) -> Character {
        Character {
            role: CharacterRole::Main,
            ..character(id, member, false)
        }
    }

    fn pick(scope: PickScope, target: &str, member: &str, character: &str) -> Pick {
        Pick {
            scope,
            target: target.into(),
            member_id: member.into(),
            character_id: character.into(),
            picked_at: week(0),
            picked_by: Actor::admin("token"),
            version: 1,
        }
    }

    fn run_pick(run: &str, member: &str, character: &str) -> Pick {
        pick(PickScope::Run, run, member, character)
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

    fn slot_with(
        run: &Run,
        week: &[Run],
        rows: &[Character],
        picks: &[Pick],
        timings: &[FixedRun],
    ) -> PartySlot {
        let stored = StoredPicks::new(picks, timings);
        party_slots(run, week, &roster(rows), &stored, &catalog())
            .into_iter()
            .next()
            .unwrap()
    }

    fn slot(run: &Run, week: &[Run], rows: &[Character], picks: &[Pick]) -> PartySlot {
        slot_with(run, week, rows, picks, &[])
    }

    fn brings(found: &PartySlot) -> Option<(&str, Basis)> {
        found
            .brings
            .as_ref()
            .map(|(id, basis)| (id.as_str(), *basis))
    }

    #[test]
    fn the_only_active_character_is_brought_until_someone_picks() {
        let here = run("r-1", 0, 1, &["XKalos"], &["m"]);
        let one = [character("a", "m", false)];
        let found = slot(&here, &[], &one, &[]);
        assert_eq!(
            (found.pick.as_ref(), brings(&found)),
            (None, Some(("a", Basis::Only)))
        );
        assert_eq!(found.version, 0);
        // A second active character and no main: nothing to bring.
        let two = [character("a", "m", false), character("b", "m", false)];
        let found = slot(&here, &[], &two, &[]);
        assert_eq!((found.pick.as_ref(), brings(&found)), (None, None));
        assert_eq!(found.candidates.len(), 2);
        // Retiring one brings the other again.
        let retired = [character("a", "m", true), character("b", "m", false)];
        let found = slot(&here, &[], &retired, &[]);
        assert_eq!(brings(&found), Some(("b", Basis::Only)));
        assert_eq!(
            (found.candidates.len(), found.retired),
            (1, 1),
            "retired characters are no candidates"
        );
        // A pick wins; a retired pick is ignored but keeps the slot version.
        let found = slot(&here, &[], &two, &[run_pick("r-1", "m", "b")]);
        assert_eq!(brings(&found), Some(("b", Basis::RunPick)));
        assert_eq!(found.version, 1);
        assert_eq!(
            found.pick.map(|pick| pick.character_id).as_deref(),
            Some("b")
        );
        let found = slot(&here, &[], &retired, &[run_pick("r-1", "m", "a")]);
        assert_eq!(
            (found.pick.as_ref(), brings(&found), found.version),
            (None, Some(("b", Basis::Only)), 1)
        );
        // No characters: an empty slot.
        let found = slot(&here, &[], &[], &[]);
        assert!(found.candidates.is_empty() && found.bringing().is_none());
    }

    #[test]
    fn the_main_is_the_default_and_only_a_pick_beats_it() {
        let here = run("r-1", 0, 1, &["XKalos"], &["m"]);
        let rows = [character("a", "m", false), main("b", "m")];
        let found = slot(&here, &[], &rows, &[]);
        assert_eq!(brings(&found), Some(("b", Basis::Main)));
        let found = slot(&here, &[], &rows, &[run_pick("r-1", "m", "a")]);
        assert_eq!(brings(&found), Some(("a", Basis::RunPick)));
        // The main alone reads as the main, not the only character.
        let found = slot(&here, &[], &[main("b", "m")], &[]);
        assert_eq!(brings(&found), Some(("b", Basis::Main)));
    }

    #[test]
    fn a_run_reads_its_timings_pick_live_under_its_own() {
        let mut here = run("r-1", 0, 1, &["HCarling"], &["m"]);
        here.fixed_run_id = Some("f-1".into());
        let rows = [
            main("a", "m"),
            character("b", "m", false),
            character("c", "m", false),
        ];
        let timings = [timing("f-1", &["m"])];
        let fixed = pick(PickScope::Timing, "f-1", "m", "b");
        let found = slot_with(&here, &[], &rows, std::slice::from_ref(&fixed), &timings);
        assert_eq!(brings(&found), Some(("b", Basis::TimingPick)));
        assert_eq!(
            (found.pick, found.version),
            (None, 0),
            "the run has no pick of its own"
        );
        // The run's own pick still wins.
        let own = run_pick("r-1", "m", "c");
        let found = slot_with(&here, &[], &rows, &[fixed.clone(), own], &timings);
        assert_eq!(brings(&found), Some(("c", Basis::RunPick)));
        // Ignored once the member left the timing's party, or the timing is gone.
        let left = [timing("f-1", &["o"])];
        let found = slot_with(&here, &[], &rows, std::slice::from_ref(&fixed), &left);
        assert_eq!(brings(&found), Some(("a", Basis::Main)));
        let found = slot_with(&here, &[], &rows, std::slice::from_ref(&fixed), &[]);
        assert_eq!(brings(&found), Some(("a", Basis::Main)));
        // A run from another timing never sees it.
        here.fixed_run_id = Some("f-2".into());
        let found = slot_with(&here, &[], &rows, &[fixed], &timings);
        assert_eq!(brings(&found), Some(("a", Basis::Main)));
    }

    #[test]
    fn a_timing_slot_is_its_pick_else_the_default() {
        let rows = [
            main("a", "m"),
            character("b", "m", false),
            character("x", "o", true),
            character("y", "o", false),
            character("p", "q", false),
            character("r", "q", false),
        ];
        let timings = [timing("f-1", &["m", "o", "q"])];
        let picks = [pick(PickScope::Timing, "f-1", "m", "b")];
        let stored = StoredPicks::new(&picks, &timings);
        let party = [
            "m".to_owned(),
            "o".to_owned(),
            "q".to_owned(),
            "n".to_owned(),
        ];
        let slots = timing_slots(Some("f-1"), &party, &roster(&rows), &stored);
        let found: Vec<_> = slots.iter().map(brings).collect();
        assert_eq!(
            found,
            [
                Some(("b", Basis::TimingPick)),
                Some(("y", Basis::Only)),
                None,
                None
            ]
        );
        assert_eq!(slots[0].version, 1);
        assert_eq!(slots[1].retired, 1);
        assert_eq!(slots[2].candidates.len(), 2, "several, no main: pick one");
        // A new timing has no picks; an unsaved party member none either.
        let slots = timing_slots(None, &party[..1], &roster(&rows), &stored);
        assert_eq!(brings(&slots[0]), Some(("a", Basis::Main)));
        let unsaved = [timing("f-1", &["o"])];
        let stored = StoredPicks::new(&picks, &unsaved);
        let slots = timing_slots(Some("f-1"), &party[..1], &roster(&rows), &stored);
        assert_eq!(
            (brings(&slots[0]), slots[0].version),
            (Some(("a", Basis::Main)), 1),
            "the stored row keeps its version for the next write"
        );
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
                basis: Basis::Only,
            }],
            "the same boss at another difficulty only, this boss week only"
        );
        // With two characters and another picked on r-2, no clash.
        let rows = [character("a", "m", false), character("b", "m", false)];
        let picks = [run_pick("r-1", "m", "a"), run_pick("r-2", "m", "b")];
        assert!(slot(&here, &week, &rows, &picks).clashes.is_empty());
        // Picked on both: a clash that says so.
        let picks = [run_pick("r-1", "m", "a"), run_pick("r-2", "m", "a")];
        let found = slot(&here, &week, &rows, &picks);
        assert_eq!(found.clashes.len(), 1);
        assert_eq!(found.clashes[0].basis, Basis::RunPick);
        // A pick left behind by a member who left r-2's party is ignored.
        let mut left = week.clone();
        left[1].participants = vec!["o".into()];
        assert!(slot(&here, &left, &rows, &picks).clashes.is_empty());
    }
}
