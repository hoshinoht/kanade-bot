//! Run readiness and CP floors (module `character_data`), as the server
//! answers them (`src/api/admin/character_data/readiness.rs`,
//! `src/domain/chardata/readiness.rs`): Config `cp_floors` (`{boss:
//! {difficulty: CP}}`, replaced whole by PATCH) and `GET
//! /api/admin/runs/{id}/readiness`, each party slot's level (fresh NEXON,
//! else manual) and power (NEXON combat power from fresh data only) judged
//! per boss of the run. Seeded floors: Kalos Extreme, Carling Hard, Black
//! Mage Extreme and Limbo Hard; `r-carling`'s party covers every verdict.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::catalog::{entry_level, valid_run_length_override};
use super::clock::now_secs;
use super::run_picks::choice;
use super::{MoveError, Store};

pub type Floors = BTreeMap<String, BTreeMap<String, u64>>;

pub const ATTRIBUTION: &str = super::character_data::ATTRIBUTION;
const MAX_FLOOR: u64 = 10_000_000_000;

/// Invented guild floors (whole CP).
pub fn seed() -> Floors {
    [
        ("Kalos", "x", 70_000_000),
        ("Carling", "h", 45_000_000),
        ("BM", "x", 80_000_000),
        ("Limbo", "h", 55_000_000),
    ]
    .into_iter()
    .fold(Floors::new(), |mut floors, (boss, difficulty, cp)| {
        floors
            .entry(boss.into())
            .or_default()
            .insert(difficulty.into(), cp);
        floors
    })
}

/// `cp_floors.floors` as the server takes it: catalog keys, their own
/// difficulties, whole CP 1 to 10,000,000,000, no empty boss.
pub fn parse(value: &Value) -> Result<Floors, MoveError> {
    let bosses = value
        .as_object()
        .ok_or_else(|| MoveError::invalid("cp_floors.floors must be an object."))?;
    let mut floors = Floors::new();
    for (boss, difficulties) in bosses {
        let difficulties = difficulties.as_object().ok_or_else(|| {
            MoveError::invalid(format!("cp_floors.floors.{boss} must be an object."))
        })?;
        if difficulties.is_empty() {
            return Err(MoveError::invalid(
                "Leave out a boss with no floor rather than sending it empty.",
            ));
        }
        for (difficulty, cp) in difficulties {
            if !valid_run_length_override(boss, difficulty) {
                return Err(MoveError::invalid(
                    "Pick a catalog boss key and difficulty.",
                ));
            }
            let cp = cp
                .as_u64()
                .filter(|cp| (1..=MAX_FLOOR).contains(cp))
                .ok_or_else(|| {
                    MoveError::invalid("A CP floor is a whole number from 1 to 10,000,000,000.")
                })?;
            floors
                .entry(boss.clone())
                .or_default()
                .insert(difficulty.clone(), cp);
        }
    }
    Ok(floors)
}

fn level_verdict(level: Option<i64>, entry: Option<u16>) -> &'static str {
    match (level, entry) {
        (Some(level), Some(entry)) if level >= i64::from(entry) => "ok",
        (Some(_), Some(_)) => "under",
        _ => "unknown",
    }
}

fn power_verdict(power: Option<i64>, floor: Option<u64>) -> &'static str {
    match (floor, power) {
        (None, _) => "no_floor",
        (Some(_), None) => "unknown",
        (Some(floor), Some(power)) if u64::try_from(power).is_ok_and(|p| p >= floor) => "ok",
        (Some(_), Some(_)) => "under",
    }
}

impl Store {
    /// `GET /api/admin/runs/{id}/readiness`: never writes.
    pub fn run_readiness(&self, id: &str) -> Result<Value, MoveError> {
        let run = self.run_rec(id)?;
        let now = now_secs();
        let floors = &self.config.cp_floors;
        let mut nexon_data = false;
        let slots: Vec<Value> = run
            .participants
            .iter()
            .map(|p| {
                let brings = self.brings(run, p.id).and_then(|(id, inferred)| {
                    self.characters
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| (c, inferred))
                });
                let (link, nexon) = match brings {
                    Some((c, _)) => {
                        let (state, values) = self.nexon_values(&c.id, now);
                        (Some(state), values)
                    }
                    None => (None, None),
                };
                let manual = brings.and_then(|(c, _)| c.level);
                let (level, source, as_of) = match &nexon {
                    Some((Some(level), _, as_of)) => (Some(*level), "nexon", Some(as_of.clone())),
                    _ => (manual, "manual", None),
                };
                let power = nexon
                    .as_ref()
                    .and_then(|(_, cp, as_of)| cp.map(|cp| (cp, as_of.clone())));
                nexon_data |= source == "nexon" || power.is_some();
                let bosses: Vec<Value> = run
                    .bosses
                    .iter()
                    .map(|b| {
                        let entry = entry_level(b.key, b.difficulty);
                        let floor = floors.get(b.key).and_then(|f| f.get(b.difficulty)).copied();
                        json!({
                            "boss": b.key, "difficulty": b.difficulty,
                            "entry_level": entry, "cp_floor": floor,
                            "level": level_verdict(level, entry),
                            "power": power_verdict(power.as_ref().map(|(cp, _)| *cp), floor),
                        })
                    })
                    .collect();
                json!({
                    "member_id": p.id,
                    "character": brings.map(|(c, _)| choice(c)),
                    "inferred": brings.is_some_and(|(_, inferred)| inferred),
                    "link": link,
                    "level": level.map(|value| json!({ "value": value, "source": source, "as_of": as_of })),
                    "power": power.map(|(value, as_of)| json!({ "kind": "nexon_cp", "value": value, "as_of": as_of })),
                    "bosses": bosses,
                })
            })
            .collect();
        Ok(json!({
            "run_id": run.id, "slots": slots, "nexon_data": nexon_data, "attribution": ATTRIBUTION,
        }))
    }

    /// Config's `cp_floors` block.
    pub fn cp_floors_view(&self) -> Value {
        json!({
            "floors": self.config.cp_floors,
            "entry_levels": super::catalog::entry_levels(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::tests::store;

    fn slot<'a>(answer: &'a Value, member: &str) -> &'a Value {
        answer["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["member_id"] == member)
            .unwrap()
    }

    fn verdicts(answer: &Value, member: &str) -> Vec<(String, String)> {
        slot(answer, member)["bosses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["level"].as_str().unwrap().to_owned(),
                    b["power"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn pair(level: &str, power: &str) -> (String, String) {
        (level.into(), power.into())
    }

    #[test]
    fn the_carling_party_covers_every_verdict() {
        let s = store();
        let ready = s.run_readiness("r-carling").ok().unwrap();
        assert_eq!(ready["nexon_data"], true);
        assert_eq!(ready["attribution"], ATTRIBUTION);
        assert_eq!(
            slot(&ready, "1001")["bosses"][0],
            json!({"boss": "Carling", "difficulty": "h", "entry_level": 275, "cp_floor": 45_000_000,
                   "level": "ok", "power": "unknown"})
        );
        assert_eq!(slot(&ready, "1001")["link"], "pending");
        assert_eq!(
            verdicts(&ready, "1002"),
            [pair("ok", "unknown"), pair("under", "no_floor")]
        );
        assert_eq!(slot(&ready, "1003")["character"], Value::Null);
        assert_eq!(
            verdicts(&ready, "1003"),
            [pair("unknown", "unknown"), pair("unknown", "no_floor")]
        );
        let yuzu = slot(&ready, "1004");
        assert_eq!(
            (&yuzu["link"], &yuzu["level"]["source"]),
            (&json!("linked"), &json!("nexon"))
        );
        assert_eq!(
            verdicts(&ready, "1004"),
            [pair("ok", "ok"), pair("under", "no_floor")]
        );
        assert_eq!(
            verdicts(&ready, "1005"),
            [pair("ok", "under"), pair("ok", "no_floor")]
        );
        let hotaru = slot(&ready, "1011");
        assert_eq!(hotaru["link"], "expired");
        assert_eq!(
            hotaru["level"],
            json!({"value": 275, "source": "manual", "as_of": null})
        );
        assert_eq!(hotaru["power"], Value::Null);
        assert_eq!(
            verdicts(&ready, "1011"),
            [pair("ok", "unknown"), pair("under", "no_floor")]
        );
        assert!(s.run_readiness("nope").is_err());
    }

    #[test]
    fn floors_parse_as_the_server_takes_them() {
        assert_eq!(seed().len(), 4);
        assert!(parse(&json!({"Kalos": {"x": MAX_FLOOR}})).is_ok());
        for bad in [
            json!({"Lucid": {"h": 1}}),
            json!({"Kalos": {"h": 1}}),
            json!({"Kalos": {"x": 0}}),
            json!({"Kalos": {"x": MAX_FLOOR + 1}}),
            json!({"Kalos": {"x": 1.5}}),
            json!({"Kalos": {}}),
            json!([]),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }
}
