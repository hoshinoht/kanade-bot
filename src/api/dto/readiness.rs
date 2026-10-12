//! `character_data.json` run readiness (module `character_data`): each party
//! slot's level and power, and its level against each boss of the run.
//! NEXON values appear only from an active link's unexpired data, with the
//! attribution.

use serde::Serialize;

use super::characters::{CharacterChoice, choice};
use crate::domain::{
    characters::Character,
    chardata::{
        ATTRIBUTION, LevelSource, LevelVerdict, PowerKind, Readiness, Requirement, date_text,
    },
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
    /// `scouter` is reserved for MapleScouter's boss values (not sent yet).
    #[cfg_attr(test, ts(type = "'nexon_cp' | 'scouter'"))]
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
}

/// One slot: `brings` is the character and whether it is inferred.
pub fn slot(
    member_id: &str,
    brings: Option<(&Character, bool)>,
    link: Option<&'static str>,
    readiness: &Readiness,
    bosses: &[Requirement],
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
            kind: match power.kind {
                PowerKind::NexonCp => "nexon_cp",
            },
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
