//! `character_data.json` readiness (module `character_data`) of a run or a
//! weekly timing: each party slot's level and power, its level against
//! each boss, and the character's stored scouter reading per boss, of any
//! age, marked `stale` once at least `estimate_days` old. NEXON values
//! appear only from an active link's unexpired data, with the attribution.

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
    /// The character the member brings (as `…/characters` resolves it);
    /// `null` with none.
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
    /// One per boss of the run, in run order.
    pub bosses: Vec<BossReadiness>,
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
    /// From boss knowledge; `null` when it has none.
    pub entry_level: Option<u16>,
    #[cfg_attr(test, ts(type = "'ok' | 'under' | 'unknown'"))]
    pub level: &'static str,
    /// The character's stored scouter reading for this boss, of any age
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

/// One slot: `candidates` is how many active characters the member has,
/// `brings` the character and why; `scouter` its stored scouter result.
pub fn slot(
    member_id: &str,
    candidates: usize,
    brings: Option<(&Character, Basis)>,
    link: Option<&'static str>,
    readiness: &Readiness,
    bosses: &[Requirement],
    scouter: Option<&Cached>,
) -> ReadinessSlot {
    ReadinessSlot {
        member_id: member_id.to_owned(),
        character: brings.map(|(character, _)| choice(character)),
        basis: brings.map(|(_, basis)| basis.as_str()),
        candidates: u32::try_from(candidates).unwrap_or(u32::MAX),
        link,
        level: readiness.level.map(|level| ReadinessLevel {
            value: level.value,
            source: match level.source {
                LevelSource::Nexon => "nexon",
                LevelSource::Manual => "manual",
            },
            as_of: level.as_of.map(date_text),
        }),
        power: readiness.power.map(|power| ReadinessPower {
            kind: "nexon_cp",
            value: power.value,
            as_of: power.as_of.map(date_text),
        }),
        bosses: bosses
            .iter()
            .zip(&readiness.bosses)
            .map(|(boss, verdict)| BossReadiness {
                boss: boss.boss.clone(),
                difficulty: boss.difficulty.clone(),
                entry_level: boss.entry_level,
                level: match verdict {
                    LevelVerdict::Ok => "ok",
                    LevelVerdict::Under => "under",
                    LevelVerdict::Unknown => "unknown",
                },
                scouter: scouter.and_then(|cached| reading(boss, cached)),
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
