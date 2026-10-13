//! Run readiness (design §9): for one party character, its level and power,
//! and its level against the entry level of each boss of the run. Computed
//! on read, never stored, never blocking a pick or a save. NEXON values
//! count only from an active link's unexpired snapshot; otherwise the level
//! is the manual roster level and there is no power.

use chrono::{DateTime, NaiveDate, Utc};

use super::{LinkStatus, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelSource {
    Nexon,
    /// The roster level an admin entered.
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub value: u16,
    pub source: LevelSource,
    /// The NEXON data's day ([`Snapshot::as_of`]); `None` for a manual level.
    pub as_of: Option<NaiveDate>,
}

/// The NEXON combat power of usable data; shown, never judged.
/// Scouter readings are per boss and come from the estimates' memory
/// (API layer), not from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    pub value: u64,
    pub as_of: Option<NaiveDate>,
}

/// One party character as readiness sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Subject<'a> {
    pub manual_level: Option<u16>,
    /// The link's current status (`None`: not linked).
    pub link: Option<LinkStatus>,
    /// The link's snapshot, whatever its age.
    pub snapshot: Option<&'a Snapshot>,
}

/// What one boss of the run asks: the knowledge entry level, `None` when
/// there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    /// Catalog boss key.
    pub boss: String,
    /// Lowercase difficulty letter.
    pub difficulty: String,
    pub entry_level: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelVerdict {
    Ok,
    Under,
    /// No level, or no entry level for the boss.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Readiness {
    pub level: Option<Level>,
    pub power: Option<Power>,
    /// A NEXON value is shown (the attribution must be too).
    pub nexon: bool,
    /// One level verdict per requirement, in the same order.
    pub bosses: Vec<LevelVerdict>,
}

/// The snapshot whose values may count at `now`: an active link's, not
/// expired and not stamped in the future.
pub fn usable_nexon<'a>(subject: &Subject<'a>, now: DateTime<Utc>) -> Option<&'a Snapshot> {
    subject
        .snapshot
        .filter(|snapshot| subject.link == Some(LinkStatus::Active) && snapshot.is_fresh(now))
}

/// Equal to the entry level is ready.
pub fn judge(level: Option<Level>, boss: &Requirement) -> LevelVerdict {
    match (level, boss.entry_level) {
        (Some(level), Some(entry)) if level.value >= entry => LevelVerdict::Ok,
        (Some(_), Some(_)) => LevelVerdict::Under,
        _ => LevelVerdict::Unknown,
    }
}

/// `subject` against each of `bosses`: the NEXON level when usable, else
/// the manual one; the power is the usable data's NEXON combat power.
pub fn readiness(subject: &Subject<'_>, bosses: &[Requirement], now: DateTime<Utc>) -> Readiness {
    let nexon = usable_nexon(subject, now);
    let level = nexon
        .and_then(|snapshot| {
            snapshot.level.map(|value| Level {
                value,
                source: LevelSource::Nexon,
                as_of: Some(snapshot.as_of()),
            })
        })
        .or_else(|| {
            subject.manual_level.map(|value| Level {
                value,
                source: LevelSource::Manual,
                as_of: None,
            })
        });
    let power = nexon.and_then(|snapshot| {
        snapshot.combat_power.map(|value| Power {
            value,
            as_of: Some(snapshot.as_of()),
        })
    });
    Readiness {
        level,
        power,
        nexon: level.is_some_and(|level| level.source == LevelSource::Nexon) || power.is_some(),
        bosses: bosses.iter().map(|boss| judge(level, boss)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::domain::chardata::SNAPSHOT_TTL;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap()
    }

    /// Invented NEXON data fetched `age` ago.
    fn snapshot(level: Option<u16>, power: Option<u64>, age: TimeDelta) -> Snapshot {
        let fetched_at = now() - age;
        Snapshot {
            character_id: "ch-1".into(),
            ocid: "ocid-invented".into(),
            ocid_fetched_at: fetched_at,
            character_name: Some("Wisteria".into()),
            world_name: None,
            character_class: None,
            level,
            combat_power: power,
            image_url: None,
            data_date: None,
            fetched_at,
            expires_at: Snapshot::expiry(fetched_at, fetched_at),
        }
    }

    fn boss(key: &str, entry_level: Option<u16>) -> Requirement {
        Requirement {
            boss: key.into(),
            difficulty: "h".into(),
            entry_level,
        }
    }

    fn linked(snapshot: &Snapshot) -> Subject<'_> {
        Subject {
            manual_level: Some(270),
            link: Some(LinkStatus::Active),
            snapshot: Some(snapshot),
        }
    }

    #[test]
    fn equal_to_the_entry_level_is_ok() {
        let data = snapshot(Some(280), Some(45_000_000), TimeDelta::hours(3));
        let ready = readiness(&linked(&data), &[boss("Star", Some(280))], now());
        assert_eq!(ready.bosses, [LevelVerdict::Ok]);
        let under = readiness(&linked(&data), &[boss("Star", Some(281))], now());
        assert_eq!(under.bosses, [LevelVerdict::Under]);
    }

    #[test]
    fn fresh_nexon_data_wins_over_the_manual_level() {
        let data = snapshot(Some(285), Some(50_000_000), TimeDelta::hours(3));
        let ready = readiness(&linked(&data), &[], now());
        let level = ready.level.unwrap();
        assert_eq!(
            (level.value, level.source, level.as_of),
            (285, LevelSource::Nexon, Some(data.as_of()))
        );
        assert_eq!(
            ready.power,
            Some(Power {
                value: 50_000_000,
                as_of: Some(data.as_of()),
            })
        );
        assert!(ready.nexon);
    }

    #[test]
    fn expired_or_unlinked_data_falls_back_to_the_manual_level_and_no_power() {
        let expired = snapshot(Some(285), Some(50_000_000), SNAPSHOT_TTL);
        let future = snapshot(Some(285), Some(50_000_000), -TimeDelta::hours(1));
        let fresh = snapshot(Some(285), Some(50_000_000), TimeDelta::hours(3));
        let carling = [boss("Carling", Some(275))];
        for subject in [
            linked(&expired),
            linked(&future),
            Subject {
                link: Some(LinkStatus::Stale),
                ..linked(&fresh)
            },
            Subject {
                link: Some(LinkStatus::Pending),
                ..linked(&fresh)
            },
            Subject {
                link: None,
                ..linked(&fresh)
            },
        ] {
            let ready = readiness(&subject, &carling, now());
            assert_eq!(
                ready.level,
                Some(Level {
                    value: 270,
                    source: LevelSource::Manual,
                    as_of: None,
                }),
                "{:?}",
                subject.link
            );
            assert_eq!(ready.power, None);
            assert!(!ready.nexon);
            // NEXON's 285 would pass; the manual 270 does not.
            assert_eq!(ready.bosses, [LevelVerdict::Under]);
        }
    }

    #[test]
    fn nexon_data_without_a_level_keeps_the_manual_level_but_its_power() {
        let data = snapshot(None, Some(50_000_000), TimeDelta::hours(3));
        let ready = readiness(&linked(&data), &[], now());
        assert_eq!(ready.level.unwrap().source, LevelSource::Manual);
        assert_eq!(ready.power.unwrap().value, 50_000_000);
        assert!(ready.nexon);
    }

    #[test]
    fn missing_knowledge_or_levels_are_not_judged() {
        let nothing = Subject::default();
        let ready = readiness(
            &nothing,
            &[boss("Lucid", None), boss("Carling", Some(275))],
            now(),
        );
        assert_eq!((ready.level, ready.power, ready.nexon), (None, None, false));
        assert_eq!(ready.bosses, [LevelVerdict::Unknown, LevelVerdict::Unknown]);
        // A known level against a boss without knowledge.
        let data = snapshot(Some(290), Some(90_000_000), TimeDelta::hours(3));
        assert_eq!(
            readiness(&linked(&data), &[boss("Lucid", None)], now()).bosses,
            [LevelVerdict::Unknown]
        );
    }

    #[test]
    fn each_boss_of_a_run_is_judged_on_its_own() {
        let data = snapshot(Some(278), Some(48_000_000), TimeDelta::hours(3));
        let ready = readiness(
            &linked(&data),
            &[
                boss("Carling", Some(275)),
                boss("Star", Some(280)),
                boss("Limbo", Some(285)),
            ],
            now(),
        );
        assert_eq!(
            ready.bosses,
            [LevelVerdict::Ok, LevelVerdict::Under, LevelVerdict::Under]
        );
    }
}
