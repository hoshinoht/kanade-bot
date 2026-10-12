//! MapleScouter estimates (module `character_data` with `KANADE_SCOUTER_URL`),
//! as the server's admin API answers them (`src/api/admin/character_data/
//! estimates.rs`): `POST /api/admin/estimates {character_ids | run_id,
//! refresh?}` → `202 {job}`, `GET /api/admin/estimates/{job}` and
//! `GET /api/admin/estimates?character_ids=|run_id=` → `{job, running}`.
//! There is no sidecar: each read of a running job advances it one
//! character (a party then takes one more read for its total), so e2e gets
//! the same steps every time. Wholly invented numbers.
//!
//! `POST /__mock/estimates {scenario}` sets what later lookups do, until the
//! next reset: `ok` (default), `busy` (429, 40 s), `critical_rate` (422),
//! `manual_check` (503), `failed` (502), `timeout` (504), `hold` (never
//! finishes: the running state and the 409), `off` (no scouter URL: every
//! route answers the generic 404). A party's error stops at its last
//! character (no total). Seeded: a done lookup of MikaNL (`ch-s2`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::clock::{iso_secs, now_secs};
use super::{MoveError, Store};

pub const LABEL: &str = "Estimate from MapleScouter";
pub const NOTE: &str = "Estimate for planning only, not eligibility";
const MAX_BATCH: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Ok,
    Busy,
    CriticalRate,
    ManualCheck,
    Failed,
    Timeout,
    Hold,
    Off,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioRequest {
    pub scenario: Scenario,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EstimateRequest {
    #[serde(default)]
    character_ids: Option<Vec<String>>,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    refresh: bool,
}

#[derive(Clone)]
struct Subject {
    character_id: String,
    member_id: String,
    ign: String,
}

#[derive(Clone)]
pub struct Job {
    id: String,
    run_id: Option<String>,
    character_ids: Vec<String>,
    /// The run's boss tokens (a run's totals).
    bosses: Vec<String>,
    subjects: Vec<Subject>,
    missing: Vec<Value>,
    scenario: Scenario,
    started_at: i64,
    /// Reads so far.
    reads: usize,
    cache_hit: bool,
    fetched_at: Option<i64>,
}

/// The mock's estimate state.
#[derive(Clone)]
pub struct Estimates {
    scenario: Scenario,
    jobs: Vec<Job>,
    next: u32,
}

/// `(token, percentage, effective share, rating, [Party])`, in catalog
/// order; a character's figures scale with its place in the party.
const CARDS: [(&str, f64, Option<f64>, &str, bool); 4] = [
    ("XKalos", 54.0, Some(9.0), "6p Min Cut", true),
    ("HCarling", 18.0, Some(18.0), "Party-able", false),
    ("HStar", 64.5, Some(21.5), "3p Min Cut", true),
    ("HLimbo", 72.4, Some(72.4), "Party-able", false),
];

fn coded(status: u16, code: &'static str, message: &str) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

impl Job {
    fn party(&self) -> bool {
        self.run_id.is_some() || self.subjects.len() > 1
    }

    /// Reads it takes to finish: one per character, plus the party total.
    fn steps(&self) -> usize {
        if self.party() {
            self.subjects.len() + 1
        } else {
            1
        }
    }

    /// Where an error stops it: the last character.
    fn stop_at(&self) -> usize {
        self.subjects.len() - 1
    }

    fn state(&self) -> &'static str {
        match self.scenario {
            _ if self.cache_hit => "done",
            Scenario::Hold => "running",
            Scenario::Ok if self.reads >= self.steps() => "done",
            Scenario::Ok => "running",
            _ if self.reads > self.stop_at() => "failed",
            _ => "running",
        }
    }

    fn done(&self) -> usize {
        match self.state() {
            "done" => self.subjects.len(),
            "failed" => self.stop_at(),
            _ => self.reads.min(self.subjects.len()),
        }
    }

    fn same_target(&self, run_id: &Option<String>, ids: &[String]) -> bool {
        match (&self.run_id, run_id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.character_ids == ids,
            _ => false,
        }
    }

    /// Who lacks a card: the last of a party has no Star card (the boards'
    /// TsuKain).
    fn has_card(&self, at: usize, token: &str) -> bool {
        !(token == "HStar" && self.party() && at + 1 == self.subjects.len() && at > 0)
    }

    fn members(&self) -> Vec<Value> {
        self.subjects
            .iter()
            .enumerate()
            .map(|(at, subject)| {
                let scale = 1.0 + 0.15 * at as f64;
                let bosses: Vec<Value> = CARDS
                    .iter()
                    .filter(|(token, ..)| self.has_card(at, token))
                    .map(|(token, percentage, share, rating, party)| {
                        let share = share.map(|share| round(share * scale));
                        json!({
                            "boss": token,
                            "percentage": round(percentage * scale),
                            "effective_percentage": share,
                            "estimated_minutes": share.map(|share| round(3000.0 / share)),
                            "rating": rating,
                            "party": party,
                        })
                    })
                    .collect();
                json!({
                    "character_id": subject.character_id,
                    "ign": subject.ign,
                    "preset": "00010",
                    "bosses": bosses,
                })
            })
            .collect()
    }

    fn combined(&self, members: &[Value]) -> Option<Vec<Value>> {
        if !self.party() {
            return None;
        }
        let tokens: Vec<String> = if self.run_id.is_some() {
            self.bosses.clone()
        } else {
            CARDS.iter().map(|card| card.0.to_owned()).collect()
        };
        let whole = self.missing.is_empty();
        Some(
            tokens
                .iter()
                .map(|token| {
                    if !CARDS.iter().any(|card| card.0 == token) {
                        return json!({
                            "boss": token, "combined_percentage": null, "estimated_minutes": null,
                            "technically_possible": null, "complete": false,
                            "issues": [{"character_id": null, "issue": "not_covered"}],
                        });
                    }
                    let mut sum = 0.0;
                    let mut issues = Vec::new();
                    for (member, subject) in members.iter().zip(&self.subjects) {
                        match member["bosses"]
                            .as_array()
                            .and_then(|rows| rows.iter().find(|row| row["boss"] == *token))
                        {
                            Some(row) => sum += row["effective_percentage"].as_f64().unwrap_or(0.0),
                            None => issues.push(json!({
                                "character_id": subject.character_id, "issue": "missing_boss",
                            })),
                        }
                    }
                    if issues.is_empty() {
                        let sum = round(sum);
                        json!({
                            "boss": token, "combined_percentage": sum,
                            "estimated_minutes": round(3000.0 / sum),
                            "technically_possible": sum >= 95.0, "complete": whole,
                            "issues": [],
                        })
                    } else {
                        json!({
                            "boss": token, "combined_percentage": null, "estimated_minutes": null,
                            "technically_possible": null, "complete": false, "issues": issues,
                        })
                    }
                })
                .collect(),
        )
    }

    fn error(&self) -> Value {
        let (code, message, status, detail, retry) = match self.scenario {
            Scenario::Busy => ("busy", "MapleScouter is busy.", 429, "busy", Some(40)),
            Scenario::CriticalRate => (
                "critical_rate",
                "Set up critical rate on MapleScouter first.",
                422,
                "critical_rate_setup_required",
                None,
            ),
            Scenario::ManualCheck => (
                "manual_check",
                "Scouter needs a manual check.",
                503,
                "verification_required",
                None,
            ),
            Scenario::Timeout => ("failed", "Lookup failed.", 504, "lookup_timeout", None),
            _ => ("failed", "Lookup failed.", 502, "lookup_failed", None),
        };
        json!({
            "code": code, "message": message, "status": status, "detail": detail,
            "retry_after_seconds": retry,
            "character_id": self.subjects[self.stop_at()].character_id,
        })
    }

    fn json(&self) -> Value {
        let state = self.state();
        let finished = (state != "running").then(|| self.started_at + 20);
        let result = (state == "done").then(|| {
            let members = self.members();
            let combined = self.combined(&members);
            let complete = self.missing.is_empty()
                && combined
                    .as_ref()
                    .is_none_or(|rows| rows.iter().all(|row| row["complete"] == true));
            json!({
                "cache_hit": self.cache_hit,
                "complete": complete,
                "members": members,
                "combined": combined,
            })
        });
        let current = (state == "running" && self.party() && self.reads < self.subjects.len())
            .then(|| self.subjects[self.reads].character_id.clone());
        json!({
            "id": self.id,
            "state": state,
            "run_id": self.run_id,
            "character_ids": self.character_ids,
            "started_by": {"kind": "admin", "id": "discord:1001"},
            "started_at": iso_secs(self.started_at),
            "finished_at": finished.map(iso_secs),
            "characters": self.subjects.iter().map(|s| json!({
                "character_id": s.character_id, "member_id": s.member_id, "ign": s.ign,
            })).collect::<Vec<_>>(),
            "missing": self.missing,
            "progress": {"done": self.done(), "total": self.subjects.len(), "current": current},
            "fetched_at": (state == "done").then(|| iso_secs(self.fetched_at.unwrap_or(self.started_at + 20))),
            "result": result,
            "error": (state == "failed").then(|| self.error()),
            "label": LABEL,
            "note": NOTE,
        })
    }
}

impl Estimates {
    /// MikaNL (`ch-s2`, linked) looked up two hours ago.
    pub fn seed() -> Self {
        let at = now_secs() - 2 * 3_600;
        Self {
            scenario: Scenario::Ok,
            jobs: vec![Job {
                id: "est-seed-1".into(),
                run_id: None,
                character_ids: vec!["ch-s2".into()],
                bosses: Vec::new(),
                subjects: vec![Subject {
                    character_id: "ch-s2".into(),
                    member_id: "1003".into(),
                    ign: "MikaNL".into(),
                }],
                missing: Vec::new(),
                scenario: Scenario::Ok,
                started_at: at,
                reads: 1,
                cache_hit: false,
                fetched_at: Some(at + 20),
            }],
            next: 1,
        }
    }

    pub fn on(&self) -> bool {
        self.scenario != Scenario::Off
    }

    fn running(&self) -> Option<&Job> {
        self.jobs.iter().find(|job| job.state() == "running")
    }
}

impl Store {
    /// `POST /__mock/estimates {scenario}`.
    pub fn set_estimate_scenario(&mut self, scenario: Scenario) {
        self.estimates.scenario = scenario;
    }

    /// Estimate routes answer the generic 404 while off (or the roster is).
    pub fn estimates_on(&self) -> bool {
        self.roster_on() && self.estimates.on()
    }

    fn subject_of(&self, id: &str) -> Result<Subject, MoveError> {
        let c = self
            .characters
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| coded(404, "not_found", "That character is not on the roster."))?;
        if c.world != "msea" {
            return Err(coded(
                422,
                "region",
                "MapleScouter covers MapleSEA characters only.",
            ));
        }
        Ok(Subject {
            character_id: c.id.clone(),
            member_id: c.member_id.clone(),
            ign: c.ign.clone(),
        })
    }

    /// `POST /api/admin/estimates`: `202 {job}`.
    pub fn start_estimate(&mut self, req: EstimateRequest) -> Result<Value, MoveError> {
        let (run_id, mut ids, subjects, missing, bosses) = match (req.character_ids, req.run_id) {
            (Some(mut ids), None) => {
                ids.sort();
                ids.dedup();
                if !(1..=MAX_BATCH).contains(&ids.len()) {
                    return Err(MoveError::invalid("Send 1 to 6 character ids."));
                }
                let subjects = ids
                    .iter()
                    .map(|id| self.subject_of(id))
                    .collect::<Result<Vec<_>, _>>()?;
                (None, ids, subjects, Vec::new(), Vec::new())
            }
            (None, Some(run_id)) => {
                let slots = self.run_characters(&run_id)?;
                let bosses = self
                    .runs
                    .iter()
                    .find(|run| run.id == run_id)
                    .map(|run| run.bosses.iter().map(|b| b.token.clone()).collect())
                    .unwrap_or_default();
                let mut subjects = Vec::new();
                let mut missing = Vec::new();
                for slot in slots["slots"].as_array().into_iter().flatten() {
                    let member = slot["member_id"].as_str().unwrap_or_default();
                    let brought = slot["picked"].as_str().or(slot["inferred"].as_str());
                    match brought.map(|id| self.subject_of(id)) {
                        Some(Ok(subject)) => subjects.push(subject),
                        Some(Err(_)) => missing.push(json!({
                            "member_id": member, "character_id": brought, "reason": "other_region",
                        })),
                        None => missing.push(json!({
                            "member_id": member, "character_id": null,
                            "reason": if slot["candidates"].as_array().is_some_and(Vec::is_empty) {
                                "no_characters"
                            } else {
                                "no_character_picked"
                            },
                        })),
                    }
                }
                if subjects.is_empty() {
                    return Err(coded(
                        422,
                        "nothing_to_look_up",
                        "No one in this party has a character to look up.",
                    ));
                }
                if subjects.len() > MAX_BATCH {
                    return Err(coded(
                        422,
                        "too_many",
                        "MapleScouter looks up at most 6 characters at once.",
                    ));
                }
                (Some(run_id), Vec::new(), subjects, missing, bosses)
            }
            _ => return Err(MoveError::invalid("Send character_ids or run_id.")),
        };
        if self.estimates.running().is_some() {
            return Err(coded(409, "estimate_running", "Another lookup is running."));
        }
        ids.sort();
        let now = now_secs();
        let cached = (!req.refresh)
            .then(|| {
                self.estimates
                    .jobs
                    .iter()
                    .rev()
                    .find(|job| job.same_target(&run_id, &ids) && job.state() == "done")
            })
            .flatten()
            .map(|job| job.fetched_at.unwrap_or(job.started_at + 20));
        // One job per target, as the server keeps them.
        self.estimates
            .jobs
            .retain(|job| !job.same_target(&run_id, &ids));
        let id = format!("est-{}", self.estimates.next);
        self.estimates.next += 1;
        let job = Job {
            id,
            run_id,
            character_ids: ids,
            bosses,
            subjects,
            missing,
            scenario: self.estimates.scenario,
            started_at: now,
            reads: 0,
            cache_hit: cached.is_some(),
            fetched_at: cached,
        };
        let answer = json!({ "job": job.json() });
        self.estimates.jobs.push(job);
        Ok(answer)
    }

    /// `GET /api/admin/estimates/{job}`: advances a running job one step.
    pub fn estimate(&mut self, id: &str) -> Result<Value, MoveError> {
        let job = self
            .estimates
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| {
                coded(
                    404,
                    "not_found",
                    "That lookup is gone: results are kept for 24 hours.",
                )
            })?;
        if job.state() == "running" {
            job.reads += 1;
        }
        Ok(job.json())
    }

    /// `GET /api/admin/estimates?character_ids=|run_id=`.
    pub fn estimate_status(
        &self,
        character_ids: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<Value, MoveError> {
        let (run_id, ids) = match (character_ids, run_id) {
            (Some(ids), None) => {
                let mut ids: Vec<String> = ids
                    .split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .collect();
                ids.sort();
                ids.dedup();
                (None, ids)
            }
            (None, Some(run_id)) => (Some(run_id.to_owned()), Vec::new()),
            _ => {
                return Err(MoveError::invalid(
                    "Ask with character_ids=<id>,<id> or run_id=<id>.",
                ));
            }
        };
        Ok(json!({
            "job": self.estimates.jobs.iter().rev().find(|job| job.same_target(&run_id, &ids)).map(Job::json),
            "running": self.estimates.running().map(Job::json),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Store::new(super::super::catalog::Catalog::new(
            root.join("web/e2e/fixtures/boss"),
        ))
    }

    fn request(body: Value) -> EstimateRequest {
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn a_party_steps_one_character_per_read_then_the_total() {
        let mut s = store();
        let started = s
            .start_estimate(request(json!({"run_id": "r-carling"})))
            .ok()
            .unwrap();
        let id = started["job"]["id"].as_str().unwrap().to_owned();
        let total = started["job"]["progress"]["total"].as_u64().unwrap() as usize;
        assert!(total >= 1);
        for read in 1..=total {
            let job = s.estimate(&id).ok().unwrap();
            assert_eq!(job["state"], "running", "read {read}");
            assert_eq!(job["progress"]["done"], read);
        }
        let done = s.estimate(&id).ok().unwrap();
        assert_eq!(done["state"], "done");
        assert_eq!(done["result"]["complete"], false, "Mika has no pick");
        // A second lookup answers from memory at once.
        let again = s
            .start_estimate(request(json!({"run_id": "r-carling"})))
            .ok()
            .unwrap();
        assert_eq!(again["job"]["state"], "done");
        assert_eq!(again["job"]["result"]["cache_hit"], true);
    }

    #[test]
    fn scenarios_draw_every_error_state_and_the_409() {
        let mut s = store();
        s.set_estimate_scenario(Scenario::Hold);
        let held = s
            .start_estimate(request(json!({"character_ids": ["ch-s3"]})))
            .ok()
            .unwrap();
        let refused = s.start_estimate(request(json!({"character_ids": ["ch-s7"]})));
        assert!(matches!(
            refused,
            Err(MoveError::Coded(409, "estimate_running", _))
        ));
        let id = held["job"]["id"].as_str().unwrap();
        assert_eq!(s.estimate(id).ok().unwrap()["state"], "running");
        for (scenario, code) in [
            (Scenario::Busy, "busy"),
            (Scenario::CriticalRate, "critical_rate"),
            (Scenario::ManualCheck, "manual_check"),
            (Scenario::Failed, "failed"),
            (Scenario::Timeout, "failed"),
        ] {
            let mut s = store();
            s.set_estimate_scenario(scenario);
            let job = s
                .start_estimate(request(
                    json!({"character_ids": ["ch-s3"], "refresh": true}),
                ))
                .ok()
                .unwrap();
            let failed = s.estimate(job["job"]["id"].as_str().unwrap()).ok().unwrap();
            assert_eq!(
                (&failed["state"], &failed["error"]["code"]),
                (&json!("failed"), &json!(code))
            );
        }
        let mut s = store();
        s.set_estimate_scenario(Scenario::Off);
        assert!(!s.estimates_on());
    }
}
