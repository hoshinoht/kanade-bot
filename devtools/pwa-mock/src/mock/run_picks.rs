//! The character each party member brings to a run (module `roster`), as
//! the server's `GET /api/admin/runs/{id}/characters` and `PUT
//! /api/admin/runs/{id}/characters/{member}` answer it
//! (`src/api/admin/characters/runs.rs`): candidates are a member's active
//! characters; only explicit picks are stored, and a member with exactly one
//! candidate and no pick reads as `inferred`; a warning names another run
//! of this boss week (not cancelled) with the same character for a shared
//! boss, any difficulty. The mock does not model frozen runs (`run_ended`),
//! as it does not for other run writes.
//!
//! Seeded for the run-sheet boards: Asahi picks AsahiDK on `r-carling`;
//! Mika has three candidates and no pick; RenShade is inferred on both
//! `r-carling` and `r-nstar` (Star twice this boss week); Ren #1013 has no
//! characters.

use serde::Deserialize;
use serde_json::{Value, json};

use super::characters::{Char, present};
use super::seed::Rec;
use super::{MoveError, Store};

#[derive(Clone)]
pub struct Pick {
    pub run: String,
    pub member: String,
    pub character: String,
    pub version: u64,
}

pub fn seed() -> Vec<Pick> {
    vec![Pick {
        run: "r-carling".into(),
        member: "1001".into(),
        // AsahiDK (`characters::seed`).
        character: "ch-s5".into(),
        version: 1,
    }]
}

/// `CharacterPick`: both fields required, `character_id: null` clears.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterPick {
    #[serde(default, deserialize_with = "present")]
    character_id: Option<Option<String>>,
    #[serde(default)]
    version: Option<u64>,
}

fn coded(status: u16, code: &'static str, message: &str) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn choice(c: &Char) -> Value {
    json!({
        "id": c.id, "ign": c.ign, "world": c.world, "class": c.class, "level": c.level,
        "role": if c.main { "main" } else { "alt" },
    })
}

impl Store {
    fn candidates(&self, member: &str) -> Vec<&Char> {
        self.characters
            .iter()
            .filter(|c| c.member_id == member && c.retired_at.is_none())
            .collect()
    }

    fn pick_of(&self, run: &str, member: &str) -> Option<&Pick> {
        self.picks
            .iter()
            .find(|p| p.run == run && p.member == member)
    }

    /// Who `member` brings to `run`: the picked candidate (`false`), else
    /// the only candidate (`true`).
    fn brings(&self, run: &Rec, member: &str) -> Option<(String, bool)> {
        if !run.participants.iter().any(|p| p.id == member) {
            return None;
        }
        let candidates = self.candidates(member);
        let picked = self
            .pick_of(&run.id, member)
            .filter(|p| candidates.iter().any(|c| c.id == p.character));
        match (picked, candidates.as_slice()) {
            (Some(p), _) => Some((p.character.clone(), false)),
            (None, [only]) => Some((only.id.clone(), true)),
            (None, _) => None,
        }
    }

    fn slot(&self, run: &Rec, member: &str) -> Value {
        let here = self.brings(run, member);
        let keys: Vec<&str> = run.bosses.iter().map(|b| b.key).collect();
        let warnings: Vec<Value> = here
            .as_ref()
            .map(|(character, _)| {
                let mut others: Vec<&Rec> = self
                    .runs
                    .iter()
                    .filter(|o| {
                        o.id != run.id && o.next_week == run.next_week && o.status != "cancelled"
                    })
                    .collect();
                others.sort_by_key(|o| (o.day, o.time.clone()));
                others
                    .into_iter()
                    .filter_map(|o| {
                        let shared: Vec<&str> = o
                            .bosses
                            .iter()
                            .filter(|b| keys.contains(&b.key))
                            .map(|b| b.token.as_str())
                            .collect();
                        let (there, inferred) = self.brings(o, member)?;
                        (!shared.is_empty() && there == *character).then(|| {
                            json!({
                                "character_id": character, "run_id": o.id,
                                "short_id": o.short_id, "day": o.day, "time": o.time,
                                "bosses": shared, "inferred": inferred,
                            })
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (picked, inferred) = match here {
            Some((id, false)) => (Some(id), None),
            Some((id, true)) => (None, Some(id)),
            None => (None, None),
        };
        json!({
            "member_id": member,
            "candidates": self.candidates(member).into_iter().map(choice).collect::<Vec<_>>(),
            "picked": picked,
            "inferred": inferred,
            "version": self.pick_of(&run.id, member).map_or(0, |p| p.version),
            "warnings": warnings,
        })
    }

    fn run_rec(&self, id: &str) -> Result<&Rec, MoveError> {
        self.runs
            .iter()
            .find(|r| r.id == id)
            .ok_or(MoveError::NotFound)
    }

    /// `GET /api/admin/runs/{id}/characters`: never writes.
    pub fn run_characters(&self, id: &str) -> Result<Value, MoveError> {
        let run = self.run_rec(id)?;
        let slots: Vec<Value> = run
            .participants
            .iter()
            .map(|p| self.slot(run, p.id))
            .collect();
        Ok(json!({ "run_id": run.id, "slots": slots }))
    }

    /// `PUT /api/admin/runs/{id}/characters/{member}`: the member's slot.
    pub fn pick_character(
        &mut self,
        id: &str,
        member: &str,
        req: CharacterPick,
    ) -> Result<Value, MoveError> {
        let version = req.version.ok_or_else(|| {
            coded(
                422,
                "version_required",
                "Send the version the run's characters were loaded at.",
            )
        })?;
        let character = req
            .character_id
            .ok_or_else(|| MoveError::invalid("Send character_id, or null to clear the pick."))?;
        let run = self.run_rec(id)?;
        if !run.participants.iter().any(|p| p.id == member) {
            return Err(coded(
                422,
                "not_on_run",
                "That member is not in this run's party.",
            ));
        }
        let current = self.pick_of(id, member).cloned();
        if current.as_ref().map_or(0, |p| p.version) != version {
            return Err(coded(
                409,
                "stale",
                "This member's character on the run changed since it was loaded.",
            ));
        }
        match character {
            Some(character) => {
                let row = self
                    .characters
                    .iter()
                    .find(|c| c.id == character)
                    .ok_or_else(|| {
                        coded(404, "not_found", "That character is not on the roster.")
                    })?;
                if row.member_id != member {
                    return Err(coded(
                        422,
                        "not_their_character",
                        "That character belongs to another member.",
                    ));
                }
                if row.retired_at.is_some() {
                    return Err(coded(
                        422,
                        "character_retired",
                        "That character is retired: bring it back on the member's Characters tab first.",
                    ));
                }
                if current.as_ref().is_none_or(|p| p.character != character) {
                    self.picks.retain(|p| !(p.run == id && p.member == member));
                    self.picks.push(Pick {
                        run: id.into(),
                        member: member.into(),
                        character,
                        version: version + 1,
                    });
                }
            }
            None => self.picks.retain(|p| !(p.run == id && p.member == member)),
        }
        let run = self.run_rec(id)?;
        Ok(self.slot(run, member))
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

    fn pick(id: Option<&str>, version: u64) -> CharacterPick {
        CharacterPick {
            character_id: Some(id.map(Into::into)),
            version: Some(version),
        }
    }

    #[test]
    fn the_seed_draws_the_run_sheet_boards() {
        let s = store();
        let carling = s.run_characters("r-carling").ok().unwrap();
        let asahi = slot(&carling, "1001");
        assert_eq!(asahi["picked"], "ch-s5");
        let mika = slot(&carling, "1003");
        assert_eq!(mika["candidates"].as_array().unwrap().len(), 3);
        assert_eq!(
            (&mika["picked"], &mika["inferred"]),
            (&Value::Null, &Value::Null)
        );
        let ren = slot(&carling, "1002");
        assert_eq!(ren["inferred"], "ch-s7", "RenShade");
        assert_eq!(ren["warnings"][0]["run_id"], "r-nstar");
        assert_eq!(ren["warnings"][0]["bosses"], json!(["NStar"]));
        assert_eq!(slot(&carling, "1013")["candidates"], json!([]));
        // Kalos shares no boss with Carling or Star; next week is another week.
        assert_eq!(asahi["warnings"], json!([]));
        let next = s.run_characters("n-carling").ok().unwrap();
        assert_eq!(slot(&next, "1002")["warnings"], json!([]));
    }

    #[test]
    fn picks_follow_the_server_rules() {
        let mut s = store();
        let refused =
            |s: &mut Store, member: &str, req| match s.pick_character("r-carling", member, req) {
                Err(MoveError::Coded(status, code, _)) => (status, code),
                Err(MoveError::NotFound) => (404, "not_found"),
                Err(MoveError::Invalid(_)) => (422, "invalid"),
                _ => panic!("accepted"),
            };
        assert_eq!(
            refused(&mut s, "1003", pick(Some("ch-s5"), 0)),
            (422, "not_their_character")
        );
        assert_eq!(
            refused(&mut s, "1003", pick(Some("ch-s1"), 0)),
            (422, "character_retired")
        );
        assert_eq!(
            refused(&mut s, "1003", pick(Some("ch-x"), 0)),
            (404, "not_found")
        );
        assert_eq!(refused(&mut s, "1003", pick(None, 2)), (409, "stale"));
        assert_eq!(refused(&mut s, "1007", pick(None, 0)), (422, "not_on_run"));
        let picked = s
            .pick_character("r-carling", "1003", pick(Some("ch-s2"), 0))
            .ok()
            .unwrap();
        assert_eq!(
            (&picked["picked"], &picked["version"]),
            (&json!("ch-s2"), &json!(1))
        );
        let cleared = s
            .pick_character("r-carling", "1001", pick(None, 1))
            .ok()
            .unwrap();
        assert_eq!(cleared["version"], 0);
        // Deleting a character drops its picks.
        s.pick_character("r-carling", "1001", pick(Some("ch-s6"), 0))
            .ok()
            .unwrap();
        s.delete_character("ch-s6").ok().unwrap();
        assert!(s.picks.iter().all(|p| p.character != "ch-s6"));
    }
}
