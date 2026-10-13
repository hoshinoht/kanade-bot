//! Readiness (module `character_data`), as the server answers it
//! (`src/api/admin/character_data/readiness.rs`,
//! `src/domain/chardata/readiness.rs`): `GET /api/admin/runs/{id}/readiness`
//! and, with the roster on too, a weekly timing's `GET
//! /api/admin/fixed/{id}/readiness` and the editor's preview `POST
//! /api/admin/fixed/readiness`: each party slot's level (fresh NEXON, else
//! manual) and power (NEXON combat power from fresh data only, shown, never
//! judged), its level judged per boss, and per boss the character's
//! MapleScouter reading once a done mock lookup holds one (`null` before,
//! and while estimates are off). `r-carling`'s party covers every level
//! verdict.

use serde::Deserialize;
use serde_json::{Value, json};

use super::catalog::{BossRef, entry_level, parse_bosses};
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

/// `TimingPreview`: the editor's bosses and party as they stand.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingPreview {
    #[serde(default)]
    fixed_run_id: Option<String>,
    bosses: Vec<String>,
    participants: Vec<String>,
}

const PREVIEW_MAX: usize = 64;

impl Store {
    /// One slot: `brings` the character and its basis; `nexon_data` is set
    /// when it shows a NEXON value.
    fn ready_slot(
        &self,
        member: &str,
        brings: Option<(String, &'static str)>,
        bosses: &[BossRef],
        nexon_data: &mut bool,
    ) -> Value {
        let now = now_secs();
        let brings = brings.and_then(|(id, basis)| {
            self.characters
                .iter()
                .find(|c| c.id == id)
                .map(|c| (c, basis))
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
        *nexon_data |= source == "nexon" || power.is_some();
        let bosses: Vec<Value> = bosses
            .iter()
            .map(|b| {
                let entry = entry_level(b.key, b.difficulty);
                let scouter = match brings {
                    Some((c, _)) if self.estimates_on() => {
                        self.estimates.reading(&c.id, &b.token, now)
                    }
                    _ => Value::Null,
                };
                json!({
                    "boss": b.key, "difficulty": b.difficulty, "entry_level": entry,
                    "level": level_verdict(level, entry), "scouter": scouter,
                })
            })
            .collect();
        json!({
            "member_id": member,
            "character": brings.map(|(c, _)| choice(c)),
            "basis": brings.map(|(_, basis)| basis),
            "candidates": self.candidates(member).len(),
            "link": link,
            "level": level.map(|value| json!({ "value": value, "source": source, "as_of": as_of })),
            "power": power.map(|(value, as_of)| json!({ "kind": "nexon_cp", "value": value, "as_of": as_of })),
            "bosses": bosses,
        })
    }

    /// `GET /api/admin/runs/{id}/readiness`: never writes.
    pub fn run_readiness(&self, id: &str) -> Result<Value, MoveError> {
        let run = self.run_rec(id)?;
        let mut nexon_data = false;
        let slots: Vec<Value> = run
            .participants
            .iter()
            .map(|p| self.ready_slot(p.id, self.brings(run, p.id), &run.bosses, &mut nexon_data))
            .collect();
        Ok(json!({
            "run_id": run.id, "slots": slots, "nexon_data": nexon_data, "attribution": ATTRIBUTION,
        }))
    }

    fn timing_readiness(&self, fixed: Option<&str>, party: &[&str], bosses: &[BossRef]) -> Value {
        let mut nexon_data = false;
        let slots: Vec<Value> = party
            .iter()
            .map(|m| self.ready_slot(m, self.timing_brings(fixed, m), bosses, &mut nexon_data))
            .collect();
        json!({
            "fixed_run_id": fixed, "slots": slots, "nexon_data": nexon_data, "attribution": ATTRIBUTION,
        })
    }

    /// `GET /api/admin/fixed/{id}/readiness`: the saved timing.
    pub fn fixed_readiness(&self, id: &str) -> Result<Value, MoveError> {
        let party = self.timing_party(id)?;
        let bosses = self
            .fixed
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.bosses.clone())
            .unwrap_or_default();
        Ok(self.timing_readiness(Some(id), &party, &bosses))
    }

    /// `POST /api/admin/fixed/readiness`: the form as it stands. Picks count
    /// only for members of the saved party; unknown tokens are left out.
    pub fn preview_readiness(&self, req: TimingPreview) -> Result<Value, MoveError> {
        if req.bosses.len() > PREVIEW_MAX || req.participants.len() > PREVIEW_MAX {
            return Err(MoveError::invalid(
                "A preview reads at most 64 bosses and 64 members.",
            ));
        }
        if let Some(id) = &req.fixed_run_id {
            self.timing_party(id)?;
        }
        let bosses: Vec<BossRef> = req
            .bosses
            .iter()
            .filter_map(|token| parse_bosses(token).ok())
            .flatten()
            .collect();
        let party: Vec<&str> = req.participants.iter().map(String::as_str).collect();
        Ok(self.timing_readiness(req.fixed_run_id.as_deref(), &party, &bosses))
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
            json!({"boss": "Carling", "difficulty": "h", "entry_level": 275, "level": "ok", "scouter": null})
        );
        assert_eq!(slot(&ready, "1001")["link"], "pending");
        assert_eq!(slot(&ready, "1001")["basis"], "run_pick");
        assert_eq!(verdicts(&ready, "1002"), ["ok", "under"]);
        let mika = slot(&ready, "1003");
        assert_eq!(
            (&mika["character"]["ign"], &mika["basis"]),
            (&json!("MikaBish"), &json!("timing_pick"))
        );
        assert_eq!(slot(&ready, "1013")["character"], Value::Null);
        assert_eq!(verdicts(&ready, "1013"), ["unknown", "unknown"]);
        let yuzu = slot(&ready, "1004");
        assert_eq!(
            (&yuzu["link"], &yuzu["level"]["source"]),
            (&json!("linked"), &json!("nexon"))
        );
        assert_eq!(yuzu["power"]["kind"], "nexon_cp");
        assert_eq!(verdicts(&ready, "1004"), ["ok", "under"]);
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

    #[test]
    fn timing_readiness_reads_the_saved_timing_or_the_form() {
        let s = store();
        let ready = s.fixed_readiness("f-carling").ok().unwrap();
        assert_eq!(ready["fixed_run_id"], "f-carling");
        assert_eq!(slot(&ready, "1003")["basis"], "timing_pick");
        assert!(s.fixed_readiness("f-gone").is_err());
        let preview = |body: Value| {
            s.preview_readiness(serde_json::from_value(body).unwrap())
                .ok()
                .unwrap()
        };
        let form = preview(json!({
            "fixed_run_id": "f-carling", "bosses": ["HCarling", "HStar", "nope"],
            "participants": ["1008", "1009", "1003"],
        }));
        let sora = slot(&form, "1008");
        assert_eq!(
            (&sora["character"], &sora["candidates"]),
            (&Value::Null, &json!(2)),
            "no main"
        );
        let kaito = slot(&form, "1009");
        assert_eq!(kaito["basis"], "only");
        assert_eq!(verdicts(&form, "1009"), ["unknown", "unknown"]);
        assert_eq!(slot(&form, "1003")["basis"], "timing_pick");
        let new = preview(json!({"bosses": ["HCarling"], "participants": ["1003"]}));
        assert_eq!(
            slot(&new, "1003")["basis"],
            "main",
            "no picks before it is added"
        );
    }

    #[test]
    fn readings_appear_once_a_lookup_is_done_and_never_while_off() {
        let mut s = store();
        let yuzu = |s: &Store| slot(&s.run_readiness("r-carling").ok().unwrap(), "1004").clone();
        assert_eq!(yuzu(&s)["bosses"][0]["scouter"], Value::Null);
        let started = s
            .start_estimate(serde_json::from_value(json!({"run_id": "r-carling"})).unwrap())
            .ok()
            .unwrap();
        let id = started["job"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            yuzu(&s)["bosses"][0]["scouter"],
            Value::Null,
            "still running"
        );
        while s.estimate(&id).ok().unwrap()["state"] == "running" {}
        let reading = &yuzu(&s)["bosses"][0]["scouter"];
        assert_eq!(reading["rating"], "Party-able");
        assert!(reading["percentage"].as_f64().unwrap() > 0.0);
        assert!(reading["fetched_at"].is_string());
        assert_eq!(yuzu(&s)["power"]["kind"], "nexon_cp", "CP is still sent");
        s.set_estimate_scenario(super::super::estimates::Scenario::Off);
        assert_eq!(yuzu(&s)["bosses"][0]["scouter"], Value::Null);
    }
}
