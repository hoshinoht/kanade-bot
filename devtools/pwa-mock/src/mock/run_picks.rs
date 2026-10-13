//! The character each party member brings to a run (module `roster`) or a
//! weekly timing (modules `roster` and `character_data`), as the server's
//! `GET`/`PUT /api/admin/runs/{id}/characters[/{member}]` and
//! `GET`/`PUT /api/admin/fixed/{id}/characters[/{member}]` answer it
//! (`src/api/admin/characters/{runs,fixed}.rs`): candidates are a member's
//! active characters; only explicit picks are stored; what a member brings
//! is resolved on every read: the run's pick, else its timing's pick (read
//! live, while the timing exists and the member is in its party), else the
//! main, else the only active character (`basis`). A warning names another
//! run of this boss week (not cancelled) with the same character for a
//! shared boss, any difficulty. The mock does not model frozen runs
//! (`run_ended`), as it does not for other run writes.
//!
//! Seeded for the boards: Asahi picks AsahiDK on `r-carling`; Mika's pick
//! for the HCarling + HStar timing (`f-carling`) is MikaBish, so its runs
//! bring her from the timing; Ren #1013 has no characters; Sora has two
//! characters and no main, Kaito one alt (`characters::seed`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::characters::{Char, present};
use super::clock::{iso_now, iso_secs, now_secs};
use super::seed::Rec;
use super::{MoveError, Store};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Run,
    Timing,
}

#[derive(Clone)]
pub struct Pick {
    pub scope: Scope,
    /// The run id, or the weekly timing's id.
    pub target: String,
    pub member: String,
    pub character: String,
    pub version: u64,
    pub picked_at: String,
    /// The admin's member id.
    pub picked_by: String,
}

pub fn seed() -> Vec<Pick> {
    let now = now_secs();
    vec![
        Pick {
            scope: Scope::Run,
            target: "r-carling".into(),
            member: "1001".into(),
            // AsahiDK (`characters::seed`).
            character: "ch-s5".into(),
            version: 1,
            picked_at: iso_secs(now - 2 * 86_400),
            picked_by: "1001".into(),
        },
        Pick {
            scope: Scope::Timing,
            target: "f-carling".into(),
            member: "1003".into(),
            // MikaBish.
            character: "ch-s3".into(),
            version: 1,
            picked_at: iso_secs(now - 86_400),
            picked_by: "1001".into(),
        },
    ]
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

pub(super) fn choice(c: &Char) -> Value {
    json!({
        "id": c.id, "ign": c.ign, "world": c.world, "class": c.class, "level": c.level,
        "role": if c.main { "main" } else { "alt" },
    })
}

impl Store {
    pub(super) fn candidates(&self, member: &str) -> Vec<&Char> {
        self.characters
            .iter()
            .filter(|c| c.member_id == member && c.retired_at.is_none())
            .collect()
    }

    fn retired_count(&self, member: &str) -> usize {
        self.characters
            .iter()
            .filter(|c| c.member_id == member && c.retired_at.is_some())
            .count()
    }

    fn pick_of(&self, scope: Scope, target: &str, member: &str) -> Option<&Pick> {
        self.picks
            .iter()
            .find(|p| p.scope == scope && p.target == target && p.member == member)
    }

    /// The timing's pick for `member` while the timing exists and they are
    /// in its saved party.
    fn timing_pick(&self, fixed: &str, member: &str) -> Option<&Pick> {
        self.fixed
            .iter()
            .find(|f| f.id == fixed && !f.retired && f.participants.contains(&member))?;
        self.pick_of(Scope::Timing, fixed, member)
    }

    /// The first of `picks` naming one of `member`'s candidates, else their
    /// main, else their only candidate.
    fn resolve(
        &self,
        member: &str,
        picks: &[(Option<&Pick>, &'static str)],
    ) -> Option<(String, &'static str)> {
        let candidates = self.candidates(member);
        picks
            .iter()
            .find_map(|(pick, basis)| {
                let pick = (*pick)?;
                candidates
                    .iter()
                    .any(|c| c.id == pick.character)
                    .then(|| (pick.character.clone(), *basis))
            })
            .or_else(|| {
                candidates
                    .iter()
                    .find(|c| c.main)
                    .map(|c| (c.id.clone(), "main"))
            })
            .or(match candidates.as_slice() {
                [only] => Some((only.id.clone(), "only")),
                _ => None,
            })
    }

    /// Who `member` brings to `run`, and why.
    pub(super) fn brings(&self, run: &Rec, member: &str) -> Option<(String, &'static str)> {
        if !run.participants.iter().any(|p| p.id == member) {
            return None;
        }
        let timing = run
            .fixed_id
            .as_deref()
            .and_then(|fixed| self.timing_pick(fixed, member));
        self.resolve(
            member,
            &[
                (self.pick_of(Scope::Run, &run.id, member), "run_pick"),
                (timing, "timing_pick"),
            ],
        )
    }

    /// Who `member` brings to the timing `fixed` (`None`: not added yet).
    pub(super) fn timing_brings(
        &self,
        fixed: Option<&str>,
        member: &str,
    ) -> Option<(String, &'static str)> {
        let pick = fixed.and_then(|fixed| self.timing_pick(fixed, member));
        self.resolve(member, &[(pick, "timing_pick")])
    }

    /// A slot without warnings: `own` is the slot's stored row, `live` the
    /// one reads use.
    fn slot_json(
        &self,
        member: &str,
        own: Option<&Pick>,
        live: Option<&Pick>,
        brings: Option<(String, &'static str)>,
        warnings: Vec<Value>,
    ) -> Value {
        let candidates = self.candidates(member);
        let picked = live.filter(|p| candidates.iter().any(|c| c.id == p.character));
        json!({
            "member_id": member,
            "candidates": candidates.into_iter().map(choice).collect::<Vec<_>>(),
            "retired": self.retired_count(member),
            "picked": picked.map(|p| &p.character),
            "picked_at": picked.map(|p| &p.picked_at),
            "picked_by": picked.map(|p| json!({ "kind": "admin", "id": format!("discord:{}", p.picked_by) })),
            "brings": brings.as_ref().map(|(id, _)| id),
            "basis": brings.as_ref().map(|(_, basis)| basis),
            "version": own.map_or(0, |p| p.version),
            "warnings": warnings,
        })
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
                        let (there, basis) = self.brings(o, member)?;
                        (!shared.is_empty() && there == *character).then(|| {
                            json!({
                                "character_id": character, "run_id": o.id,
                                "short_id": o.short_id, "day": o.day, "time": o.time,
                                "bosses": shared, "basis": basis,
                            })
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let own = self.pick_of(Scope::Run, &run.id, member);
        self.slot_json(member, own, own, here, warnings)
    }

    pub(super) fn run_rec(&self, id: &str) -> Result<&Rec, MoveError> {
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

    pub(super) fn timing_party(&self, id: &str) -> Result<Vec<&'static str>, MoveError> {
        self.fixed
            .iter()
            .find(|f| f.id == id && !f.retired)
            .map(|f| f.participants.clone())
            .ok_or_else(|| coded(404, "not_found", "That weekly timing no longer exists."))
    }

    fn timing_slot(&self, id: &str, member: &str) -> Value {
        let own = self.pick_of(Scope::Timing, id, member);
        let live = self.timing_pick(id, member);
        self.slot_json(
            member,
            own,
            live,
            self.timing_brings(Some(id), member),
            Vec::new(),
        )
    }

    /// `GET /api/admin/fixed/{id}/characters`: never writes.
    pub fn timing_characters(&self, id: &str) -> Result<Value, MoveError> {
        let party = self.timing_party(id)?;
        let slots: Vec<Value> = party.iter().map(|m| self.timing_slot(id, m)).collect();
        Ok(json!({ "fixed_run_id": id, "slots": slots }))
    }

    /// One slot write after the target's own checks: the version, then the
    /// character a set names.
    fn write_pick(
        &mut self,
        scope: Scope,
        target: &str,
        member: &str,
        req: CharacterPick,
        loaded: &str,
    ) -> Result<(), MoveError> {
        let version = req.version.ok_or_else(|| {
            coded(
                422,
                "version_required",
                &format!("Send the version the {loaded} characters were loaded at."),
            )
        })?;
        let character = req
            .character_id
            .ok_or_else(|| MoveError::invalid("Send character_id, or null to clear the pick."))?;
        let current = self.pick_of(scope, target, member).cloned();
        if current.as_ref().map_or(0, |p| p.version) != version {
            let on = if scope == Scope::Run {
                "the run"
            } else {
                "this timing"
            };
            return Err(coded(
                409,
                "stale",
                &format!("This member's character on {on} changed since it was loaded."),
            ));
        }
        let here = |p: &Pick| p.scope == scope && p.target == target && p.member == member;
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
                    self.picks.retain(|p| !here(p));
                    self.picks.push(Pick {
                        scope,
                        target: target.into(),
                        member: member.into(),
                        character,
                        version: version + 1,
                        picked_at: iso_now(),
                        picked_by: "1001".into(),
                    });
                }
            }
            None => self.picks.retain(|p| !here(p)),
        }
        Ok(())
    }

    /// `PUT /api/admin/runs/{id}/characters/{member}`: the member's slot.
    pub fn pick_character(
        &mut self,
        id: &str,
        member: &str,
        req: CharacterPick,
    ) -> Result<Value, MoveError> {
        if req.version.is_some() && req.character_id.is_some() {
            let run = self.run_rec(id)?;
            if !run.participants.iter().any(|p| p.id == member) {
                return Err(coded(
                    422,
                    "not_on_run",
                    "That member is not in this run's party.",
                ));
            }
        }
        self.write_pick(Scope::Run, id, member, req, "run's")?;
        let run = self.run_rec(id)?;
        Ok(self.slot(run, member))
    }

    /// `PUT /api/admin/fixed/{id}/characters/{member}`: the member's slot.
    pub fn pick_timing_character(
        &mut self,
        id: &str,
        member: &str,
        req: CharacterPick,
    ) -> Result<Value, MoveError> {
        if req.version.is_some() && req.character_id.is_some() {
            let party = self.timing_party(id)?;
            if !party.contains(&member) {
                return Err(coded(
                    422,
                    "not_on_timing",
                    "That member is not in this timing's party.",
                ));
            }
        }
        self.write_pick(Scope::Timing, id, member, req, "timing's")?;
        Ok(self.timing_slot(id, member))
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
    fn the_seed_draws_the_boards() {
        let s = store();
        let carling = s.run_characters("r-carling").ok().unwrap();
        let asahi = slot(&carling, "1001");
        assert_eq!(
            (&asahi["picked"], &asahi["basis"]),
            (&json!("ch-s5"), &json!("run_pick"))
        );
        let mika = slot(&carling, "1003");
        assert_eq!(mika["candidates"].as_array().unwrap().len(), 3);
        assert_eq!(mika["retired"], 1);
        assert_eq!(
            (&mika["picked"], &mika["brings"], &mika["basis"]),
            (&Value::Null, &json!("ch-s3"), &json!("timing_pick")),
            "MikaBish from the timing"
        );
        let ren = slot(&carling, "1002");
        assert_eq!(
            (&ren["brings"], &ren["basis"]),
            (&json!("ch-s7"), &json!("main"))
        );
        assert_eq!(ren["warnings"][0]["run_id"], "r-nstar");
        assert_eq!(ren["warnings"][0]["bosses"], json!(["NStar"]));
        assert_eq!(ren["warnings"][0]["basis"], "main");
        assert_eq!(slot(&carling, "1013")["candidates"], json!([]));
        assert_eq!(asahi["warnings"], json!([]));
        let next = s.run_characters("n-carling").ok().unwrap();
        assert_eq!(slot(&next, "1002")["warnings"], json!([]));

        let timing = s.timing_characters("f-carling").ok().unwrap();
        let mika = slot(&timing, "1003");
        assert_eq!(
            (&mika["picked"], &mika["basis"], &mika["version"]),
            (&json!("ch-s3"), &json!("timing_pick"), &json!(1))
        );
        assert_eq!(mika["picked_by"]["id"], "discord:1001");
        assert_eq!(
            slot(&timing, "1001")["basis"],
            "main",
            "a run pick stays on its run"
        );
        assert!(s.timing_characters("f-gone").is_err());
    }

    #[test]
    fn runs_read_the_timing_pick_live_under_their_own() {
        let mut s = store();
        let mika =
            |s: &Store, run: &str| slot(&s.run_characters(run).ok().unwrap(), "1003").clone();
        assert_eq!(mika(&s, "n-carling")["brings"], "ch-s3");
        // The timing's pick changes: every run of it with no own pick follows.
        s.pick_timing_character("f-carling", "1003", pick(Some("ch-s4"), 1))
            .ok()
            .unwrap();
        assert_eq!(mika(&s, "r-carling")["brings"], "ch-s4");
        s.pick_character("r-carling", "1003", pick(Some("ch-s2"), 0))
            .ok()
            .unwrap();
        assert_eq!(mika(&s, "r-carling")["basis"], "run_pick");
        assert_eq!(mika(&s, "n-carling")["basis"], "timing_pick");
        // Cleared, the main.
        s.pick_timing_character("f-carling", "1003", pick(None, 2))
            .ok()
            .unwrap();
        assert_eq!(
            (
                &mika(&s, "n-carling")["brings"],
                &mika(&s, "n-carling")["basis"]
            ),
            (&json!("ch-s2"), &json!("main"))
        );
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
        assert!(matches!(
            s.pick_timing_character("f-carling", "1007", pick(None, 0)),
            Err(MoveError::Coded(422, "not_on_timing", _))
        ));
        assert!(matches!(
            s.pick_timing_character("f-carling", "1003", pick(None, 0)),
            Err(MoveError::Coded(409, "stale", _))
        ));
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
        // Deleting a character drops its picks, run and timing alike.
        s.pick_character("r-carling", "1001", pick(Some("ch-s6"), 0))
            .ok()
            .unwrap();
        s.delete_character("ch-s6").ok().unwrap();
        s.delete_character("ch-s3").ok().unwrap();
        assert!(
            s.picks
                .iter()
                .all(|p| p.character != "ch-s6" && p.character != "ch-s3")
        );
    }
}
