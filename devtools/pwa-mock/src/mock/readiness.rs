//! Run readiness (module `character_data`), as the server answers it
//! (`src/api/admin/character_data/readiness.rs`,
//! `src/domain/chardata/readiness.rs`): `GET /api/admin/runs/{id}/readiness`,
//! each party slot's level (fresh NEXON, else manual) and power (NEXON
//! combat power from fresh data only, shown, never judged), and its level
//! judged per boss of the run. `r-carling`'s party covers every level
//! verdict.

use serde_json::{Value, json};

use super::catalog::entry_level;
use super::clock::now_secs;
use super::run_picks::choice;
use super::{MoveError, Store};

pub const ATTRIBUTION: &str = super::character_data::ATTRIBUTION;

fn level_verdict(level: Option<i64>, entry: Option<u16>) -> &'static str {
    match (level, entry) {
        (Some(level), Some(entry)) if level >= i64::from(entry) => "ok",
        (Some(_), Some(_)) => "under",
        _ => "unknown",
    }
}

impl Store {
    /// `GET /api/admin/runs/{id}/readiness`: never writes.
    pub fn run_readiness(&self, id: &str) -> Result<Value, MoveError> {
        let run = self.run_rec(id)?;
        let now = now_secs();
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
                        json!({
                            "boss": b.key, "difficulty": b.difficulty, "entry_level": entry,
                            "level": level_verdict(level, entry),
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

    fn verdicts(answer: &Value, member: &str) -> Vec<String> {
        slot(answer, member)["bosses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["level"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn the_carling_party_covers_every_verdict() {
        let s = store();
        let ready = s.run_readiness("r-carling").ok().unwrap();
        assert_eq!(ready["nexon_data"], true);
        assert_eq!(ready["attribution"], ATTRIBUTION);
        assert_eq!(
            slot(&ready, "1001")["bosses"][0],
            json!({"boss": "Carling", "difficulty": "h", "entry_level": 275, "level": "ok"})
        );
        assert_eq!(slot(&ready, "1001")["link"], "pending");
        assert_eq!(verdicts(&ready, "1002"), ["ok", "under"]);
        assert_eq!(slot(&ready, "1003")["character"], Value::Null);
        assert_eq!(verdicts(&ready, "1003"), ["unknown", "unknown"]);
        let yuzu = slot(&ready, "1004");
        assert_eq!(
            (&yuzu["link"], &yuzu["level"]["source"]),
            (&json!("linked"), &json!("nexon"))
        );
        assert_eq!(yuzu["power"]["kind"], "nexon_cp");
        assert_eq!(verdicts(&ready, "1004"), ["ok", "under"]);
        assert_eq!(verdicts(&ready, "1005"), ["ok", "ok"]);
        let hotaru = slot(&ready, "1011");
        assert_eq!(hotaru["link"], "expired");
        assert_eq!(
            hotaru["level"],
            json!({"value": 275, "source": "manual", "as_of": null})
        );
        assert_eq!(hotaru["power"], Value::Null);
        assert_eq!(verdicts(&ready, "1011"), ["ok", "under"]);
        assert!(s.run_readiness("nope").is_err());
    }
}
