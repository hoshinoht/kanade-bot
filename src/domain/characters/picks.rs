//! Which character a party member brings to a run or a weekly timing (user
//! decisions 2026-10-12, 2026-10-13, 2026-10-16). A member's candidates are
//! their active characters, at any level. Only explicit picks are stored,
//! per run (`roster_run_picks`) and per weekly timing (`roster_fixed_picks`);
//! what a member brings is resolved on every read and never written, in
//! this order: the run's own pick (runs only), the timing's pick (read live
//! by every run made from the timing), their roster main, their only active
//! character, else nothing ([`Basis`]). That is what the first boss brings.
//! With several bosses, each later boss may have its own pick
//! (`roster_run_boss_picks`, `roster_fixed_boss_picks`, keyed by the catalog
//! boss at any difficulty): the run's pick for that boss, else the timing's,
//! else the first boss's character ([`Basis::SameAsFirst`]); each boss's own
//! pick wins at its level. A pick is roster data outside the history chain:
//! it saves directly (never staged in a draft) and is audited in History's
//! settings changes (section `characters`, key `pick.<run>.<member>` or
//! `fixedpick.<fixed>.<member>`, `.<boss>` added for a per-boss pick, `.*`
//! for "same for every boss").
//!
//! A pick is ignored on reads while its member is not in the party (the
//! run's, or the timing's as saved), its character is retired or its
//! timing is retired; a per-boss pick also while its boss is not on the run
//! or timing, or is its first boss (its rows stay, so undoing the change
//! brings them back); deleting a character drops its picks.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

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

/// One stored pick: a `roster_run_picks` or `roster_fixed_picks` row, or
/// with a `boss` a `roster_run_boss_picks` or `roster_fixed_boss_picks` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub scope: PickScope,
    /// The run id, or the weekly timing's id.
    pub target: String,
    pub member_id: String,
    /// The catalog boss (any difficulty) of a per-boss pick; `None` for the
    /// slot's own pick, which the first boss brings.
    pub boss: Option<String>,
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
        if self
            .boss
            .as_deref()
            .is_some_and(|boss| !(1..=64).contains(&chars(boss)))
        {
            return refuse("boss is 1-64 characters");
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

/// One write to a `(scope, target, member)` slot, or with a `boss` to that
/// member's per-boss slot. `version` is the slot version the caller read
/// ([`pick_version`]): another is [`PickWrite::Stale`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickChange {
    Set {
        scope: PickScope,
        target: String,
        member_id: String,
        boss: Option<String>,
        character_id: String,
        version: u32,
    },
    Clear {
        scope: PickScope,
        target: String,
        member_id: String,
        boss: Option<String>,
        version: u32,
    },
    /// "Same for every boss": clear the member's per-boss picks on the
    /// target for the bosses in `versions` (catalog boss → the version
    /// read, 0 for none) in one write, or nothing when any is at another
    /// version.
    Same {
        scope: PickScope,
        target: String,
        member_id: String,
        versions: BTreeMap<String, u32>,
    },
}

impl PickChange {
    /// The `(scope, target, member)` the change names.
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
            }
            | Self::Same {
                scope,
                target,
                member_id,
                ..
            } => (*scope, target, member_id),
        }
    }

    /// The boss of a per-boss set or clear.
    pub fn boss(&self) -> Option<&str> {
        match self {
            Self::Set { boss, .. } | Self::Clear { boss, .. } => boss.as_deref(),
            Self::Same { .. } => None,
        }
    }

    /// The character a set names.
    pub fn character_id(&self) -> Option<&str> {
        match self {
            Self::Set { character_id, .. } => Some(character_id),
            Self::Clear { .. } | Self::Same { .. } => None,
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

/// `change` (a set or clear) against the slot's stored pick `current`
/// (with its character, for the record) and `character`, the row a set
/// names.
///
/// # Errors
/// [`StoreError::Constraint`] for a pick that fails [`Pick::check`], or a
/// [`PickChange::Same`] (planned by [`plan_same`]).
pub fn plan_pick(
    change: PickChange,
    current: Option<Pick>,
    current_character: Option<&Character>,
    character: Option<&Character>,
    record: &CharacterRecord,
) -> Result<PlannedPick, StoreError> {
    let version = match &change {
        PickChange::Set { version, .. } | PickChange::Clear { version, .. } => *version,
        PickChange::Same { .. } => {
            return Err(StoreError::Constraint(
                "pick: same for every boss is planned with plan_same".into(),
            ));
        }
    };
    if pick_version(current.as_ref()) != version {
        return Ok(PlannedPick::unwritten(PickWrite::Stale(current)));
    }
    let after = match &change {
        PickChange::Clear { .. } | PickChange::Same { .. } => None,
        PickChange::Set {
            scope,
            target,
            member_id,
            boss,
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
                boss: boss.clone(),
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

/// A planned "same for every boss": the outcome, the rows it deletes and
/// its History record (none for a refusal or a no-op).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSame {
    pub written: PickWrite,
    pub cleared: Vec<Pick>,
    pub change: Option<SettingsChange>,
}

/// `versions` (catalog boss → the version read) against `current`, the
/// member's stored per-boss picks on the target for those bosses, each with
/// its character (for the record). Any boss at another version is
/// [`PickWrite::Stale`]; otherwise every row goes in one write with one
/// History record (key `<scope>.<target>.<member>.*`, the cleared picks as
/// a JSON list on its `from` side).
pub fn plan_same(
    versions: &BTreeMap<String, u32>,
    current: Vec<(Pick, Option<Character>)>,
    record: &CharacterRecord,
) -> PlannedSame {
    let stored = |boss: &str| {
        current
            .iter()
            .find(|(pick, _)| pick.boss.as_deref() == Some(boss))
            .map(|(pick, _)| pick)
    };
    let fresh = versions
        .iter()
        .all(|(boss, version)| pick_version(stored(boss)) == *version);
    let named: Vec<(Pick, Option<Character>)> = current
        .into_iter()
        .filter(|(pick, _)| {
            pick.boss
                .as_deref()
                .is_some_and(|boss| versions.contains_key(boss))
        })
        .collect();
    if !fresh {
        return PlannedSame {
            written: PickWrite::Stale(None),
            cleared: Vec::new(),
            change: None,
        };
    }
    let Some((first, _)) = named.first() else {
        return PlannedSame {
            written: PickWrite::Cleared,
            cleared: Vec::new(),
            change: None,
        };
    };
    let key = format!(
        "{}.{}.{}.*",
        first.scope.key(),
        first.target,
        first.member_id
    );
    let sides: Vec<Value> = named
        .iter()
        .map(|(pick, character)| side_value(pick, character.as_ref()))
        .collect();
    let change = SettingsChange {
        id: 0,
        at: record.at,
        actor: record.actor.clone(),
        surface: record.surface,
        section: CHARACTERS_SECTION.into(),
        revision: 0,
        values: BTreeMap::from([(
            key,
            RowDiff {
                from: Value::Array(sides).to_string(),
                to: String::new(),
            },
        )]),
    };
    PlannedSame {
        written: PickWrite::Cleared,
        cleared: named.into_iter().map(|(pick, _)| pick).collect(),
        change: Some(change),
    }
}

/// A pick as one History side: its target, member, boss (per-boss picks
/// only), character and the character's IGN.
fn side_value(pick: &Pick, character: Option<&Character>) -> Value {
    let mut side = json!({
        pick.scope.field(): pick.target,
        "member": pick.member_id,
        "character": pick.character_id,
        "ign": character.map(|row| &row.ign),
    });
    if let (Some(boss), Some(fields)) = (&pick.boss, side.as_object_mut()) {
        fields.insert("boss".into(), json!(boss));
    }
    side
}

/// The History record of a slot going from `before` to `after`, each with
/// its character (for the IGN); `None` when nothing changed. A per-boss
/// pick's key and sides name its boss; an empty side is "no pick of its
/// own" (for a per-boss pick: the same as the first boss).
fn pick_record(
    record: &CharacterRecord,
    before: Option<(&Pick, Option<&Character>)>,
    after: Option<(&Pick, Option<&Character>)>,
) -> Option<SettingsChange> {
    let side = |side: Option<(&Pick, Option<&Character>)>| {
        side.map_or_else(String::new, |(pick, character)| {
            side_value(pick, character).to_string()
        })
    };
    let pick = after.or(before)?.0;
    let (from, to) = (side(before), side(after));
    let mut key = format!("{}.{}.{}", pick.scope.key(), pick.target, pick.member_id);
    if let Some(boss) = &pick.boss {
        key = format!("{key}.{boss}");
    }
    (from != to).then(|| SettingsChange {
        id: 0,
        at: record.at,
        actor: record.actor.clone(),
        surface: record.surface,
        section: CHARACTERS_SECTION.into(),
        revision: 0,
        values: BTreeMap::from([(key, RowDiff { from, to })]),
    })
}

spelled! {
    /// Where the character a member brings comes from, first match wins.
    Basis {
        /// The run's own pick (runs only; for a later boss, the run's pick
        /// for that boss).
        RunPick => "run_pick",
        /// The weekly timing's pick (on a run: inherited from its timing;
        /// for a later boss, the timing's pick for that boss).
        TimingPick => "timing_pick",
        /// Their roster main.
        Main => "main",
        /// Their only active character, with no main.
        Only => "only",
        /// A later boss with no pick of its own: the first boss's character.
        SameAsFirst => "same_as_first",
    }
}

/// One boss of a run or timing as a party member's slot reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossSlot {
    /// The boss token as the run or timing lists it (`HMaleficStar`).
    pub token: String,
    /// The catalog boss a per-boss pick is kept under (`MaleficStar`).
    pub boss: String,
    /// This boss's own stored pick at this level while it counts (the
    /// first boss: the slot's own pick).
    pub pick: Option<Pick>,
    /// The character brought to this boss and why; `None` with none.
    pub brings: Option<(String, Basis)>,
    /// The version a write for this boss sends back (the first boss: the
    /// slot's).
    pub version: u32,
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
    /// The character they bring to the first boss and why; `None` with no
    /// single answer.
    pub brings: Option<(String, Basis)>,
    /// The slot version a write sends back ([`pick_version`]).
    pub version: u32,
    /// The other runs this boss week the same character is on for a boss
    /// this run has too, compared boss by boss (the weekly clear limit);
    /// never on a timing. Never blocks a save.
    pub clashes: Vec<Clash>,
    /// Each boss in order with what it brings; the first is the slot's own
    /// pick and character. Empty for no bosses.
    pub bosses: Vec<BossSlot>,
}

impl PartySlot {
    /// The id of the character the member brings to the first boss.
    pub fn bringing(&self) -> Option<&str> {
        self.brings.as_ref().map(|(id, _)| id.as_str())
    }

    /// The character the member brings to the first boss and why.
    pub fn brought(&self) -> Option<(&Character, Basis)> {
        let (id, basis) = self.brings.as_ref()?;
        let character = self.candidates.iter().find(|row| row.id == *id)?;
        Some((character, *basis))
    }

    /// The character the member brings to boss `boss` (in order) and why;
    /// a slot read without bosses brings its first boss's character to
    /// each.
    pub fn brought_to(&self, boss: usize) -> Option<(&Character, Basis)> {
        if self.bosses.is_empty() {
            return self.brought();
        }
        let (id, basis) = self.bosses.get(boss)?.brings.as_ref()?;
        let character = self.candidates.iter().find(|row| row.id == *id)?;
        Some((character, *basis))
    }

    /// Some boss brings another character than the first boss does.
    pub fn per_boss(&self) -> bool {
        let first = self.bringing();
        self.bosses
            .iter()
            .any(|boss| boss.brings.as_ref().map(|(id, _)| id.as_str()) != first)
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
/// timing picks by `(timing, member)`, per-boss picks by `(target, member,
/// boss)`, and every timing that still exists (a timing pick counts only
/// for a member of its party, a per-boss one only for a later boss of it).
pub struct StoredPicks<'a> {
    runs: BTreeMap<(&'a str, &'a str), &'a Pick>,
    timings: BTreeMap<(&'a str, &'a str), &'a Pick>,
    run_bosses: BTreeMap<(&'a str, &'a str, &'a str), &'a Pick>,
    timing_bosses: BTreeMap<(&'a str, &'a str, &'a str), &'a Pick>,
    fixed: BTreeMap<&'a str, &'a FixedRun>,
}

impl<'a> StoredPicks<'a> {
    /// `picks` of both scopes and kinds; `timings` every weekly timing
    /// there is (a retired one is gone, and so are its picks on reads).
    pub fn new(picks: &'a [Pick], timings: &'a [FixedRun]) -> Self {
        let mut stored = Self {
            runs: BTreeMap::new(),
            timings: BTreeMap::new(),
            run_bosses: BTreeMap::new(),
            timing_bosses: BTreeMap::new(),
            fixed: timings
                .iter()
                .map(|fixed| (fixed.id.as_str(), fixed))
                .collect(),
        };
        for pick in picks {
            let (target, member) = (pick.target.as_str(), pick.member_id.as_str());
            match (pick.scope, pick.boss.as_deref()) {
                (PickScope::Run, None) => stored.runs.insert((target, member), pick),
                (PickScope::Timing, None) => stored.timings.insert((target, member), pick),
                (PickScope::Run, Some(boss)) => {
                    stored.run_bosses.insert((target, member, boss), pick)
                }
                (PickScope::Timing, Some(boss)) => {
                    stored.timing_bosses.insert((target, member, boss), pick)
                }
            };
        }
        stored
    }

    /// The stored row of a slot, whatever it names (for its version).
    pub fn row(&self, scope: PickScope, target: &str, member: &str) -> Option<&'a Pick> {
        let picks = match scope {
            PickScope::Run => &self.runs,
            PickScope::Timing => &self.timings,
        };
        picks.get(&(target, member)).copied()
    }

    /// The stored per-boss row of `boss` (a catalog boss), whatever it
    /// names (for its version).
    pub fn boss_row(
        &self,
        scope: PickScope,
        target: &str,
        member: &str,
        boss: &str,
    ) -> Option<&'a Pick> {
        let picks = match scope {
            PickScope::Run => &self.run_bosses,
            PickScope::Timing => &self.timing_bosses,
        };
        picks.get(&(target, member, boss)).copied()
    }

    /// `member` is in `fixed`'s saved party and `fixed` still exists.
    fn in_timing(&self, fixed: &str, member: &str) -> bool {
        self.fixed
            .get(fixed)
            .is_some_and(|timing| timing.participants.iter().any(|id| id == member))
    }

    /// The timing's pick for `member` while the timing exists and they are
    /// in its saved party.
    fn timing(&self, fixed: &str, member: &str) -> Option<&'a Pick> {
        self.in_timing(fixed, member)
            .then(|| self.row(PickScope::Timing, fixed, member))
            .flatten()
    }

    /// The timing's pick for `member` for `boss` while it counts at the
    /// timing: as [`Self::timing`], and `boss` is one of the timing's saved
    /// bosses other than its first.
    fn timing_boss(
        &self,
        fixed: &str,
        member: &str,
        boss: &str,
        catalog: &BossTable,
    ) -> Option<&'a Pick> {
        let timing = self.fixed.get(fixed)?;
        let mut keys = timing.bosses.iter().map(|token| boss_key(catalog, token));
        let first = keys.next()?;
        let later = boss != first && keys.any(|key| key == boss);
        (later && self.in_timing(fixed, member))
            .then(|| self.boss_row(PickScope::Timing, fixed, member, boss))
            .flatten()
    }
}

/// The first of `picks` that names one of `active`.
fn picked<'c>(
    active: &'c [Character],
    picks: &[(Option<&Pick>, Basis)],
) -> Option<(&'c Character, Basis)> {
    picks.iter().find_map(|(pick, basis)| {
        let pick = (*pick)?;
        let row = active.iter().find(|row| row.id == pick.character_id)?;
        Some((row, *basis))
    })
}

/// The first of `picks` that names one of `active`, else the main, else the
/// only active character.
fn resolve<'c>(
    active: &'c [Character],
    picks: &[(Option<&Pick>, Basis)],
) -> Option<(&'c Character, Basis)> {
    picked(active, picks)
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

/// Each of `tokens`' characters in order: the first boss brings `first`; a
/// later boss its first pick in `picks(boss)` (by catalog boss) that names
/// one of `active`, else `first` as [`Basis::SameAsFirst`]. A later token
/// of the first boss's own catalog boss is the first boss.
fn per_boss<'c, 'p>(
    tokens: &[String],
    catalog: &BossTable,
    first: Option<(&'c Character, Basis)>,
    active: &'c [Character],
    picks: impl Fn(&str) -> [(Option<&'p Pick>, Basis); 2],
) -> Vec<Option<(&'c Character, Basis)>> {
    let first_key = tokens.first().map(|token| boss_key(catalog, token));
    let same = first.map(|(row, _)| (row, Basis::SameAsFirst));
    tokens
        .iter()
        .enumerate()
        .map(|(at, token)| {
            let key = boss_key(catalog, token);
            match at {
                0 => first,
                _ if Some(key) == first_key => same,
                _ => picked(active, &picks(key)).or(same),
            }
        })
        .collect()
}

/// Who `member` brings to the first boss of `run` given their `active`
/// characters. `None` off the party or with no single answer.
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

/// Who `member` brings to each of `run`'s bosses, in order (all `None` off
/// the party): the first boss as [`run_brings`]; a later boss the run's
/// pick for it, else its timing's, else the first boss's character.
fn run_bosses<'c>(
    run: &Run,
    member: &str,
    active: &'c [Character],
    picks: &StoredPicks<'_>,
    catalog: &BossTable,
) -> Vec<Option<(&'c Character, Basis)>> {
    if !run.participants.iter().any(|id| id == member) {
        return vec![None; run.bosses.len()];
    }
    let first = run_brings(run, member, active, picks);
    per_boss(&run.bosses, catalog, first, active, |boss| {
        let timing = run
            .fixed_run_id
            .as_deref()
            .and_then(|fixed| picks.timing_boss(fixed, member, boss, catalog));
        [
            (
                picks.boss_row(PickScope::Run, &run.id, member, boss),
                Basis::RunPick,
            ),
            (timing, Basis::TimingPick),
        ]
    })
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

/// The pick and version a boss shows at one level: `first` (the slot's own
/// pick and version) for the first boss and its catalog boss; for a later
/// boss its stored row (`row(boss)`, for the version) and the pick while
/// `counts` and it names a candidate.
struct Level<'p, R: Fn(&str) -> Option<&'p Pick>> {
    first: (Option<Pick>, u32),
    row: R,
    counts: bool,
}

/// `tokens` as boss slots, each with what `brings` says it brings.
fn boss_slots<'p, R: Fn(&str) -> Option<&'p Pick>>(
    tokens: &[String],
    catalog: &BossTable,
    brings: &[Option<(&Character, Basis)>],
    level: &Level<'p, R>,
    candidates: &[Character],
) -> Vec<BossSlot> {
    let first_key = tokens.first().map(|token| boss_key(catalog, token));
    tokens
        .iter()
        .zip(brings)
        .enumerate()
        .map(|(at, (token, brings))| {
            let boss = boss_key(catalog, token);
            let (pick, version) = if at == 0 || Some(boss) == first_key {
                level.first.clone()
            } else {
                let row = (level.row)(boss);
                (
                    level.counts.then(|| own_pick(row, candidates)).flatten(),
                    pick_version(row),
                )
            };
            BossSlot {
                token: token.clone(),
                boss: boss.to_owned(),
                pick,
                brings: brings.map(|(row, basis)| (row.id.clone(), basis)),
                version,
            }
        })
        .collect()
}

/// The clashes of `member` on `run` (bringing `here` to its bosses) with
/// `others`: per other run, each character brought to a boss both runs
/// have (by catalog boss) by the member there and here, with the other
/// run's tokens of those bosses and why it is there.
fn clashes(
    run: &Run,
    here: &[Option<(&Character, Basis)>],
    others: &[&Run],
    member: &str,
    candidates: &[Character],
    picks: &StoredPicks<'_>,
    catalog: &BossTable,
) -> Vec<Clash> {
    // This run's character per catalog boss (its first token of each).
    let mut mine: Vec<(&str, &Character)> = Vec::new();
    for (token, brings) in run.bosses.iter().zip(here) {
        let key = boss_key(catalog, token);
        if !mine.iter().any(|(seen, _)| *seen == key)
            && let Some((row, _)) = brings
        {
            mine.push((key, row));
        }
    }
    let mut found = Vec::new();
    for other in others {
        let there = run_bosses(other, member, candidates, picks, catalog);
        let first_basis = there.first().copied().flatten().map(|(_, basis)| basis);
        let mut by_character: Vec<Clash> = Vec::new();
        for (token, brings) in other.bosses.iter().zip(&there) {
            let key = boss_key(catalog, token);
            let Some((row, basis)) = brings else {
                continue;
            };
            if !mine
                .iter()
                .any(|(boss, mine)| *boss == key && mine.id == row.id)
            {
                continue;
            }
            // Why the character is there: a later boss without its own
            // pick brings it for the first boss's reason.
            let basis = match basis {
                Basis::SameAsFirst => first_basis.unwrap_or(*basis),
                basis => *basis,
            };
            match by_character
                .iter_mut()
                .find(|clash| clash.character_id == row.id)
            {
                Some(clash) => clash.bosses.push(token.clone()),
                None => by_character.push(Clash {
                    character_id: row.id.clone(),
                    run_id: other.id.clone(),
                    bosses: vec![token.clone()],
                    basis,
                }),
            }
        }
        found.extend(by_character);
    }
    found
}

/// `run`'s party slots in party order. `week` is every run of `run`'s boss
/// week (`run` may be among them), `characters` each party member's rows
/// (retired included) and `picks` the stored run picks on `week`'s runs and
/// the timing picks of their timings, per-boss ones included.
pub fn party_slots(
    run: &Run,
    week: &[Run],
    characters: &BTreeMap<String, Vec<Character>>,
    picks: &StoredPicks<'_>,
    catalog: &BossTable,
) -> Vec<PartySlot> {
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
            let first = run_brings(run, member, &candidates, picks);
            let here = run_bosses(run, member, &candidates, picks, catalog);
            let clashes = clashes(run, &here, &others, member, &candidates, picks, catalog);
            let level = Level {
                first: (own_pick(own, &candidates), pick_version(own)),
                row: |boss: &str| picks.boss_row(PickScope::Run, &run.id, member, boss),
                counts: true,
            };
            let bosses = boss_slots(&run.bosses, catalog, &here, &level, &candidates);
            PartySlot {
                member_id: member.clone(),
                pick: level.first.0.clone(),
                brings: first.map(|(row, basis)| (row.id.clone(), basis)),
                version: level.first.1,
                retired,
                bosses,
                candidates,
                clashes,
            }
        })
        .collect()
}

/// A weekly timing's slots for `party` in its order, against `bosses` (the
/// timing's tokens, or a form's as it stands): `fixed` is the timing's id
/// (`None` for one not added yet). A member outside the timing's saved
/// party has no pick here (an unsaved party change).
pub fn timing_slots(
    fixed: Option<&str>,
    party: &[String],
    bosses: &[String],
    characters: &BTreeMap<String, Vec<Character>>,
    picks: &StoredPicks<'_>,
    catalog: &BossTable,
) -> Vec<PartySlot> {
    party
        .iter()
        .map(|member| {
            let (candidates, retired) = roster_of(characters, member);
            let own = fixed.and_then(|fixed| picks.row(PickScope::Timing, fixed, member));
            let live = fixed.and_then(|fixed| picks.timing(fixed, member));
            let counts = fixed.is_some_and(|fixed| picks.in_timing(fixed, member));
            let row = |boss: &str| {
                fixed.and_then(|fixed| picks.boss_row(PickScope::Timing, fixed, member, boss))
            };
            let first = resolve(&candidates, &[(live, Basis::TimingPick)]);
            let brings = per_boss(bosses, catalog, first, &candidates, |boss| {
                [
                    (row(boss).filter(|_| counts), Basis::TimingPick),
                    (None, Basis::TimingPick),
                ]
            });
            let level = Level {
                first: (own_pick(live, &candidates), pick_version(own)),
                row,
                counts,
            };
            let slots = boss_slots(bosses, catalog, &brings, &level, &candidates);
            PartySlot {
                member_id: member.clone(),
                pick: level.first.0.clone(),
                brings: first.map(|(row, basis)| (row.id.clone(), basis)),
                version: level.first.1,
                retired,
                bosses: slots,
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
            boss: None,
            character_id: character.into(),
            picked_at: week(0),
            picked_by: Actor::admin("token"),
            version: 1,
        }
    }

    fn boss_pick(
        scope: PickScope,
        target: &str,
        member: &str,
        boss: &str,
        character: &str,
    ) -> Pick {
        Pick {
            boss: Some(boss.into()),
            ..pick(scope, target, member, character)
        }
    }

    fn timing_with(id: &str, party: &[&str], bosses: &[&str]) -> FixedRun {
        FixedRun {
            bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
            ..timing(id, party)
        }
    }

    fn timing_of(
        fixed: Option<&str>,
        party: &[String],
        bosses: &[&str],
        rows: &[Character],
        stored: &StoredPicks<'_>,
    ) -> Vec<PartySlot> {
        let bosses: Vec<String> = bosses.iter().map(|boss| (*boss).to_owned()).collect();
        timing_slots(fixed, party, &bosses, &roster(rows), stored, &catalog())
    }

    /// What each boss brings, in order.
    fn per_boss(found: &PartySlot) -> Vec<Option<(&str, Basis)>> {
        found
            .bosses
            .iter()
            .map(|boss| {
                boss.brings
                    .as_ref()
                    .map(|(id, basis)| (id.as_str(), *basis))
            })
            .collect()
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
        let slots = timing_of(Some("f-1"), &party, &["HCarling"], &rows, &stored);
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
        let slots = timing_of(None, &party[..1], &["HCarling"], &rows, &stored);
        assert_eq!(brings(&slots[0]), Some(("a", Basis::Main)));
        let unsaved = [timing("f-1", &["o"])];
        let stored = StoredPicks::new(&picks, &unsaved);
        let slots = timing_of(Some("f-1"), &party[..1], &["HCarling"], &rows, &stored);
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

    #[test]
    fn a_later_boss_brings_its_own_pick_at_either_level_else_the_first_bosss() {
        use Basis::{Main, RunPick, SameAsFirst, TimingPick};
        let mut here = run("r-1", 0, 5, &["HCarling", "HStar", "XKalos"], &["m"]);
        here.fixed_run_id = Some("f-1".into());
        let rows = [
            main("a", "m"),
            character("b", "m", false),
            character("c", "m", false),
        ];
        let timings = [timing_with("f-1", &["m"], &["HCarling", "HStar", "XKalos"])];
        // Nothing picked per boss: every boss brings the first boss's.
        let found = slot_with(&here, &[], &rows, &[], &timings);
        assert_eq!(
            per_boss(&found),
            [
                Some(("a", Main)),
                Some(("a", SameAsFirst)),
                Some(("a", SameAsFirst))
            ]
        );
        assert!(!found.per_boss());
        assert_eq!(
            found
                .bosses
                .iter()
                .map(|boss| (boss.token.as_str(), boss.boss.as_str()))
                .collect::<Vec<_>>(),
            [
                ("HCarling", "Carling"),
                ("HStar", "Star"),
                ("XKalos", "Kalos")
            ]
        );
        // The timing's pick for Star, read live by the run.
        let star = boss_pick(PickScope::Timing, "f-1", "m", "Star", "b");
        let found = slot_with(&here, &[], &rows, std::slice::from_ref(&star), &timings);
        assert_eq!(
            per_boss(&found),
            [
                Some(("a", Main)),
                Some(("b", TimingPick)),
                Some(("a", SameAsFirst))
            ]
        );
        assert!(found.per_boss());
        assert_eq!(
            (found.bosses[1].pick.as_ref(), found.bosses[1].version),
            (None, 0),
            "the run has no Star pick of its own"
        );
        // The run's pick for the first boss leaves the timing's Star pick.
        let first = run_pick("r-1", "m", "c");
        let picks = [star.clone(), first.clone()];
        let found = slot_with(&here, &[], &rows, &picks, &timings);
        assert_eq!(
            per_boss(&found),
            [
                Some(("c", RunPick)),
                Some(("b", TimingPick)),
                Some(("c", SameAsFirst))
            ]
        );
        // The run's own Star pick wins over the timing's.
        let own = boss_pick(PickScope::Run, "r-1", "m", "Star", "a");
        let picks = [star.clone(), first, own.clone()];
        let found = slot_with(&here, &[], &rows, &picks, &timings);
        assert_eq!(
            per_boss(&found),
            [
                Some(("c", RunPick)),
                Some(("a", RunPick)),
                Some(("c", SameAsFirst))
            ]
        );
        assert_eq!(
            (found.bosses[1].pick.as_ref(), found.bosses[1].version),
            (Some(&own), 1)
        );
        assert_eq!(
            (found.bosses[0].pick.as_ref(), found.bosses[0].version),
            (found.pick.as_ref(), found.version),
            "the first boss is the member's own pick"
        );
        // A retired character's per-boss pick is ignored.
        let retired = [
            main("a", "m"),
            character("b", "m", true),
            character("c", "m", false),
        ];
        let found = slot_with(&here, &[], &retired, std::slice::from_ref(&star), &timings);
        assert_eq!(per_boss(&found)[1], Some(("a", SameAsFirst)));
        // No first character: a later boss with a pick still brings it.
        let two = [character("b", "m", false), character("c", "m", false)];
        let found = slot_with(&here, &[], &two, std::slice::from_ref(&star), &timings);
        assert_eq!(per_boss(&found), [None, Some(("b", TimingPick)), None]);
    }

    #[test]
    fn a_per_boss_pick_follows_its_boss_and_is_kept_while_ignored() {
        use Basis::{Main, SameAsFirst, TimingPick};
        let rows = [main("a", "m"), character("b", "m", false)];
        let picks = [boss_pick(PickScope::Timing, "f-1", "m", "Star", "b")];
        let party = ["m".to_owned()];
        let read = |bosses: &[&str], party_of: &[&str]| {
            let timings = [timing_with("f-1", party_of, bosses)];
            let stored = StoredPicks::new(&picks, &timings);
            timing_of(Some("f-1"), &party, bosses, &rows, &stored)
                .pop()
                .unwrap()
        };
        let found = read(&["HCarling", "HStar"], &["m"]);
        assert_eq!(
            per_boss(&found),
            [Some(("a", Main)), Some(("b", TimingPick))]
        );
        assert_eq!(
            (found.bosses[1].pick.as_ref(), found.bosses[1].version),
            (Some(&picks[0]), 1)
        );
        // Hard to Normal: still Star's pick.
        let found = read(&["HCarling", "NStar"], &["m"]);
        assert_eq!(
            per_boss(&found),
            [Some(("a", Main)), Some(("b", TimingPick))]
        );
        // Star off the timing: nothing reads it, and the row is still there
        // for when Star comes back.
        let found = read(&["HCarling", "XKalos"], &["m"]);
        assert_eq!(
            per_boss(&found),
            [Some(("a", Main)), Some(("a", SameAsFirst))]
        );
        // Reordered so Star is first: the first boss is the member's own pick,
        // and the kept Star pick is ignored.
        let found = read(&["HStar", "HCarling"], &["m"]);
        assert_eq!(
            per_boss(&found),
            [Some(("a", Main)), Some(("a", SameAsFirst))]
        );
        assert_eq!(
            (found.bosses[0].pick.as_ref(), found.bosses[0].version),
            (None, 0)
        );
        // The member left the saved party: ignored, its version kept.
        let found = read(&["HCarling", "HStar"], &["o"]);
        assert_eq!(
            per_boss(&found),
            [Some(("a", Main)), Some(("a", SameAsFirst))]
        );
        assert_eq!(
            (found.bosses[1].pick.as_ref(), found.bosses[1].version),
            (None, 1)
        );

        // A run reads the timing's pick only while Star is a later boss of
        // the timing as saved.
        let mut here = run("r-1", 0, 5, &["HCarling", "HStar"], &["m"]);
        here.fixed_run_id = Some("f-1".into());
        let on_run = |timing_bosses: &[&str]| {
            let timings = [timing_with("f-1", &["m"], timing_bosses)];
            per_boss(&slot_with(&here, &[], &rows, &picks, &timings))
                .into_iter()
                .map(|brought| brought.map(|(id, basis)| (id.to_owned(), basis)))
                .collect::<Vec<_>>()
        };
        let main_then =
            |id: &str, basis| vec![Some(("a".to_owned(), Main)), Some((id.to_owned(), basis))];
        assert_eq!(on_run(&["HCarling", "NStar"]), main_then("b", TimingPick));
        assert_eq!(on_run(&["HCarling"]), main_then("a", SameAsFirst));
        assert_eq!(on_run(&["HStar", "HCarling"]), main_then("a", SameAsFirst));
    }

    #[test]
    fn clashes_compare_the_character_each_shared_boss_brings() {
        let here = run("r-1", 0, 5, &["HCarling", "HStar"], &["m"]);
        let rows = [character("a", "m", false), character("b", "m", false)];
        let star = run("r-2", 0, 2, &["NStar"], &["m"]);
        let carling = run("r-3", 0, 3, &["NCarling"], &["m"]);
        let week = [here.clone(), star, carling];
        let mine = [
            run_pick("r-1", "m", "a"),
            boss_pick(PickScope::Run, "r-1", "m", "Star", "b"),
        ];
        let with = |others: &[Pick]| {
            let picks: Vec<Pick> = mine.iter().chain(others).cloned().collect();
            slot(&here, &week, &rows, &picks).clashes
        };
        let clash = |character: &str, run: &str, boss: &str| Clash {
            character_id: character.into(),
            run_id: run.into(),
            bosses: vec![boss.into()],
            basis: Basis::RunPick,
        };
        assert_eq!(
            with(&[run_pick("r-2", "m", "b"), run_pick("r-3", "m", "a")]),
            [clash("b", "r-2", "NStar"), clash("a", "r-3", "NCarling")]
        );
        // `a` on r-2 is no clash: here it brings `b` to Star.
        assert_eq!(
            with(&[run_pick("r-2", "m", "a"), run_pick("r-3", "m", "a")]),
            [clash("a", "r-3", "NCarling")]
        );
        // A later boss without its own pick there clashes for the first
        // boss's reason.
        let both = run("r-4", 0, 4, &["NCarling", "NStar"], &["m"]);
        let week = [here.clone(), both];
        let picks: Vec<Pick> = mine
            .iter()
            .cloned()
            .chain([run_pick("r-4", "m", "b")])
            .collect();
        assert_eq!(
            slot(&here, &week, &rows, &picks).clashes,
            [clash("b", "r-4", "NStar")]
        );
    }

    #[test]
    fn per_boss_writes_are_recorded_by_boss_and_same_clears_them_at_once() {
        use crate::domain::history::Surface;
        let record = CharacterRecord {
            at: week(1),
            actor: Actor::admin("token"),
            surface: Surface::AdminPortal,
        };
        let wisteria = character("b", "m", false);
        let set = PickChange::Set {
            scope: PickScope::Timing,
            target: "f-1".into(),
            member_id: "m".into(),
            boss: Some("Star".into()),
            character_id: "b".into(),
            version: 0,
        };
        let planned = plan_pick(set, None, None, Some(&wisteria), &record).unwrap();
        let after = planned.after.clone().expect("a per-boss row");
        assert_eq!(after.boss.as_deref(), Some("Star"));
        let change = planned.change.expect("recorded");
        let row = &change.values["fixedpick.f-1.m.Star"];
        assert_eq!(row.from, "", "from the same as the first boss");
        assert_eq!(
            serde_json::from_str::<Value>(&row.to).unwrap(),
            json!({"fixed": "f-1", "member": "m", "boss": "Star", "character": "b", "ign": "Ignb"})
        );

        let stored = Pick {
            picked_at: week(1),
            ..after
        };
        let versions = |pairs: &[(&str, u32)]| -> BTreeMap<String, u32> {
            pairs
                .iter()
                .map(|(boss, version)| ((*boss).to_owned(), *version))
                .collect()
        };
        let current = || vec![(stored.clone(), Some(wisteria.clone()))];
        let stale = plan_same(&versions(&[("Star", 2)]), current(), &record);
        assert_eq!(
            (stale.written, stale.cleared, stale.change),
            (PickWrite::Stale(None), Vec::new(), None)
        );
        let done = plan_same(&versions(&[("Star", 1), ("Kalos", 0)]), current(), &record);
        assert_eq!(done.written, PickWrite::Cleared);
        assert_eq!(done.cleared, std::slice::from_ref(&stored));
        let change = done.change.expect("one row");
        let row = &change.values["fixedpick.f-1.m.*"];
        assert_eq!(row.to, "");
        assert_eq!(
            serde_json::from_str::<Value>(&row.from).unwrap(),
            json!([{"fixed": "f-1", "member": "m", "boss": "Star", "character": "b", "ign": "Ignb"}])
        );
        let nothing = plan_same(&versions(&[("Star", 0)]), Vec::new(), &record);
        assert_eq!(
            (nothing.written, nothing.change),
            (PickWrite::Cleared, None)
        );
    }
}
