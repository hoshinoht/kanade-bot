//! `character_data.json` readiness (module `character_data`) of a run or a
//! weekly timing: each party slot's level and power, its level against
//! each boss, and the character's stored scouter reading per boss, of any
//! age, marked `stale` once at least `estimate_days` old. Each boss is
//! judged for the character brought to it (per-boss picks), and every
//! other character brought shows its own values once. NEXON values appear
//! only from an active link's unexpired data, with the attribution.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    characters::{CharacterChoice, choice},
    iso_instant,
};
use crate::{
    domain::{
        characters::{Basis, Character},
        chardata::{ATTRIBUTION, LevelSource, LevelVerdict, Readiness, Requirement, date_text},
    },
    infrastructure::scouter::{Estimates, Member, RosterIgn, scouter_keys},
};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunReadiness {
    pub run_id: String,
    /// The slots of `GET …/characters`, in party order.
    pub slots: Vec<ReadinessSlot>,
    /// Some slot shows a NEXON value: show the attribution.
    pub nexon_data: bool,
    /// The saved `estimate_days`: a reading at least this old is `stale`
    /// ("older than n days"); `null` while estimates are off.
    pub fresh_days: Option<u32>,
    /// `Data based on NEXON Open API`.
    pub attribution: &'static str,
}

/// A weekly timing's readiness (modules `roster` and `character_data`), or
/// a preview of one being edited.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct TimingReadiness {
    /// `null` for a timing not added yet.
    pub fixed_run_id: Option<String>,
    /// One per party member, in party order.
    pub slots: Vec<ReadinessSlot>,
    /// Some slot shows a NEXON value: show the attribution.
    pub nexon_data: bool,
    /// As [`RunReadiness::fresh_days`].
    pub fresh_days: Option<u32>,
    /// `Data based on NEXON Open API`.
    pub attribution: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReadinessSlot {
    pub member_id: String,
    /// The character the member brings to the first boss (as
    /// `…/characters` resolves it); `null` with none.
    pub character: Option<CharacterChoice>,
    /// Where `character` comes from; `null` with none.
    #[cfg_attr(test, ts(type = "'run_pick' | 'timing_pick' | 'main' | 'only' | null"))]
    pub basis: Option<&'static str>,
    /// How many active characters the member has (with no character: 0 is
    /// "no characters", more is "no main").
    pub candidates: u32,
    /// The character's link as the strip shows it; `null` without a
    /// character.
    #[cfg_attr(
        test,
        ts(
            type = "'not_connected' | 'not_linked' | 'pending' | 'linked' | 'stale' | 'expired' | null"
        )
    )]
    pub link: Option<&'static str>,
    /// Fresh NEXON level, else the manual roster level; `null` with neither.
    pub level: Option<ReadinessLevel>,
    /// `null` without usable data.
    pub power: Option<ReadinessPower>,
    /// One per boss of the run, in run order, each judged for the character
    /// brought to it.
    pub bosses: Vec<BossReadiness>,
    /// Every other character the member brings to a later boss, once each
    /// in boss order, with its own values; empty when every boss brings
    /// `character`.
    pub characters: Vec<ReadinessCharacter>,
}

/// A character brought to some later boss but not the first, with its
/// values (the slot's `link`, `level` and `power` are the first boss's).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReadinessCharacter {
    pub character: CharacterChoice,
    #[cfg_attr(
        test,
        ts(
            type = "'not_connected' | 'not_linked' | 'pending' | 'linked' | 'stale' | 'expired' | null"
        )
    )]
    pub link: Option<&'static str>,
    pub level: Option<ReadinessLevel>,
    pub power: Option<ReadinessPower>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReadinessLevel {
    pub value: u16,
    #[cfg_attr(test, ts(type = "'nexon' | 'manual'"))]
    pub source: &'static str,
    /// `YYYY-MM-DD` (Singapore): the NEXON response date, else its fetch
    /// day; `null` for a manual level.
    pub as_of: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReadinessPower {
    #[cfg_attr(test, ts(type = "'nexon_cp'"))]
    pub kind: &'static str,
    #[cfg_attr(test, ts(type = "number"))]
    pub value: u64,
    pub as_of: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BossReadiness {
    /// Catalog boss key.
    pub boss: String,
    #[cfg_attr(test, ts(type = "Difficulty"))]
    pub difficulty: String,
    /// The character judged for this boss (the one brought to it); `null`
    /// with none.
    pub character_id: Option<String>,
    /// From boss knowledge; `null` when it has none.
    pub entry_level: Option<u16>,
    #[cfg_attr(test, ts(type = "'ok' | 'under' | 'unknown'"))]
    pub level: &'static str,
    /// That character's stored scouter reading for this boss, of any age
    /// (never looked up here); `null` without one, or while estimates are
    /// off.
    pub scouter: Option<ScouterReading>,
}

/// Shown, never judged: "For planning only, not eligibility".
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ScouterReading {
    /// The character's share (`effective_percentage`), else the card's
    /// percentage.
    pub percentage: f64,
    /// The character's own share only; `null` when the scouter can't
    /// tell (shares of a party total count only this, never `percentage`).
    pub effective_percentage: Option<f64>,
    pub rating: String,
    pub estimated_minutes: Option<f64>,
    /// When kanade received it from the scouter.
    pub fetched_at: String,
    /// Fetched at least `estimate_days` ago: show it marked "older than n
    /// days".
    pub stale: bool,
    /// The scouter assumed a 100% critical rate for this character (theirs
    /// is under it): label it "Assumes 100% critical rate (actual N%)".
    pub critical_rate_assumed: bool,
    /// The character's own critical rate with an assumed reading; `null`
    /// otherwise, or when the scouter didn't say.
    pub critical_rate_actual: Option<f64>,
}

/// A character's stored scouter result: when it was fetched, and whether
/// that is at least `estimate_days` ago.
pub struct Cached {
    pub member: Member,
    pub fetched_at: DateTime<Utc>,
    pub stale: bool,
}

impl Cached {
    /// `ign`'s stored result of any age; never starts a lookup.
    pub fn of(estimates: &Estimates, ign: &RosterIgn) -> Option<Self> {
        estimates.cached(ign).map(|(member, fetched_at)| Self {
            stale: estimates.stale(fetched_at),
            member,
            fetched_at,
        })
    }
}

/// The card for `boss` in `cached`'s result, through the scouter key table.
pub fn reading(boss: &Requirement, cached: &Cached) -> Option<ScouterReading> {
    let member = &cached.member;
    let token = format!("{}{}", boss.difficulty.to_ascii_uppercase(), boss.boss);
    let card = scouter_keys(&token).find_map(|key| member.bosses.get(key))?;
    let assumed = member.crit_assumed();
    Some(ScouterReading {
        percentage: card.effective_percentage.unwrap_or(card.percentage),
        effective_percentage: card.effective_percentage,
        rating: card.rating.clone(),
        estimated_minutes: card.estimated_minutes,
        fetched_at: iso_instant(cached.fetched_at),
        stale: cached.stale,
        critical_rate_assumed: assumed,
        critical_rate_actual: member.critical_rate_actual.filter(|_| assumed),
    })
}

/// One character's values as readiness shows them: the character (`None`
/// with nothing brought), its link, its readiness against every boss and
/// its stored scouter result.
pub struct Judged<'a> {
    pub character: Option<&'a Character>,
    pub link: Option<&'static str>,
    pub readiness: Readiness,
    pub scouter: Option<Cached>,
}

fn level(readiness: &Readiness) -> Option<ReadinessLevel> {
    readiness.level.map(|level| ReadinessLevel {
        value: level.value,
        source: match level.source {
            LevelSource::Nexon => "nexon",
            LevelSource::Manual => "manual",
        },
        as_of: level.as_of.map(date_text),
    })
}

fn power(readiness: &Readiness) -> Option<ReadinessPower> {
    readiness.power.map(|power| ReadinessPower {
        kind: "nexon_cp",
        value: power.value,
        as_of: power.as_of.map(date_text),
    })
}

/// One slot: `candidates` is how many active characters the member has,
/// `brings` the first boss's character and why, `first` its values;
/// `judged[i]` is the values of the character brought to `bosses[i]`, and
/// `others` every other character brought, once each.
pub fn slot(
    member_id: &str,
    candidates: usize,
    brings: Option<(&Character, Basis)>,
    first: &Judged<'_>,
    judged: &[&Judged<'_>],
    bosses: &[Requirement],
    others: &[&Judged<'_>],
) -> ReadinessSlot {
    ReadinessSlot {
        member_id: member_id.to_owned(),
        character: brings.map(|(character, _)| choice(character)),
        basis: brings.map(|(_, basis)| basis.as_str()),
        candidates: u32::try_from(candidates).unwrap_or(u32::MAX),
        link: first.link,
        level: level(&first.readiness),
        power: power(&first.readiness),
        bosses: bosses
            .iter()
            .enumerate()
            .map(|(at, boss)| {
                let judged = judged.get(at).copied().unwrap_or(first);
                BossReadiness {
                    boss: boss.boss.clone(),
                    difficulty: boss.difficulty.clone(),
                    character_id: judged.character.map(|character| character.id.clone()),
                    entry_level: boss.entry_level,
                    level: match judged.readiness.bosses.get(at) {
                        Some(LevelVerdict::Ok) => "ok",
                        Some(LevelVerdict::Under) => "under",
                        Some(LevelVerdict::Unknown) | None => "unknown",
                    },
                    scouter: judged
                        .scouter
                        .as_ref()
                        .and_then(|cached| reading(boss, cached)),
                }
            })
            .collect(),
        characters: others
            .iter()
            .filter_map(|judged| {
                Some(ReadinessCharacter {
                    character: choice(judged.character?),
                    link: judged.link,
                    level: level(&judged.readiness),
                    power: power(&judged.readiness),
                })
            })
            .collect(),
    }
}

pub fn run(
    run_id: &str,
    slots: Vec<ReadinessSlot>,
    nexon_data: bool,
    fresh_days: Option<u32>,
) -> RunReadiness {
    RunReadiness {
        run_id: run_id.to_owned(),
        slots,
        nexon_data,
        fresh_days,
        attribution: ATTRIBUTION,
    }
}

pub fn timing(
    fixed_run_id: Option<&str>,
    slots: Vec<ReadinessSlot>,
    nexon_data: bool,
    fresh_days: Option<u32>,
) -> TimingReadiness {
    TimingReadiness {
        fixed_run_id: fixed_run_id.map(str::to_owned),
        slots,
        nexon_data,
        fresh_days,
        attribution: ATTRIBUTION,
    }
}
