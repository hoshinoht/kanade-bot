//! `character_data.json` run readiness (module `character_data`): each party
//! slot's level and power, its level against each boss of the run, and the
//! character's MapleScouter reading per boss when the estimates' 24-hour
//! memory holds one. NEXON values appear only from an active link's
//! unexpired data, with the attribution.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    characters::{CharacterChoice, choice},
    iso_instant,
};
use crate::{
    domain::{
        characters::Character,
        chardata::{ATTRIBUTION, LevelSource, LevelVerdict, Readiness, Requirement, date_text},
    },
    infrastructure::scouter::{Member, scouter_keys},
};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunReadiness {
    pub run_id: String,
    /// The slots of `GET …/characters`, in party order.
    pub slots: Vec<ReadinessSlot>,
    /// Some slot shows a NEXON value: show the attribution.
    pub nexon_data: bool,
    /// `Data based on NEXON Open API`.
    pub attribution: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReadinessSlot {
    pub member_id: String,
    /// The character the member brings (picked, else inferred); `null` with
    /// none.
    pub character: Option<CharacterChoice>,
    /// The character is the only candidate, not a pick.
    pub inferred: bool,
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
    /// The character's MapleScouter reading for this boss from the
    /// estimates' 24-hour memory (never looked up here); `null` without
    /// one, or while estimates are off.
    pub scouter: Option<ScouterReading>,
}

/// Shown, never judged: "Estimate for planning only, not eligibility".
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ScouterReading {
    /// The character's share (`effective_percentage`), else the card's
    /// percentage.
    pub percentage: f64,
    pub rating: String,
    pub estimated_minutes: Option<f64>,
    /// When kanade received it from MapleScouter.
    pub fetched_at: String,
}

/// The card for `boss` in `member`'s result, through the scouter key table.
fn reading(
    boss: &Requirement,
    member: &Member,
    fetched_at: DateTime<Utc>,
) -> Option<ScouterReading> {
    let token = format!("{}{}", boss.difficulty.to_ascii_uppercase(), boss.boss);
    let card = scouter_keys(&token).find_map(|key| member.bosses.get(key))?;
    Some(ScouterReading {
        percentage: card.effective_percentage.unwrap_or(card.percentage),
        rating: card.rating.clone(),
        estimated_minutes: card.estimated_minutes,
        fetched_at: iso_instant(fetched_at),
    })
}

/// One slot: `brings` is the character and whether it is inferred;
/// `scouter` its cached MapleScouter result and when it was fetched.
pub fn slot(
    member_id: &str,
    brings: Option<(&Character, bool)>,
    link: Option<&'static str>,
    readiness: &Readiness,
    bosses: &[Requirement],
    scouter: Option<&(Member, DateTime<Utc>)>,
) -> ReadinessSlot {
    ReadinessSlot {
        member_id: member_id.to_owned(),
        character: brings.map(|(character, _)| choice(character)),
        inferred: brings.is_some_and(|(_, inferred)| inferred),
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
                scouter: scouter
                    .and_then(|(member, fetched_at)| reading(boss, member, *fetched_at)),
            })
            .collect(),
    }
}

pub fn run(run_id: &str, slots: Vec<ReadinessSlot>, nexon_data: bool) -> RunReadiness {
    RunReadiness {
        run_id: run_id.to_owned(),
        slots,
        nexon_data,
        attribution: ATTRIBUTION,
    }
}
