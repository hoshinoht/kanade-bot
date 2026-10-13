//! MapleScouter estimates (module `character_data` with `KANADE_SCOUTER_URL`),
//! as the server's admin API answers them (`src/api/admin/character_data/
//! estimates.rs`): `POST /api/admin/estimates {character_ids | run_id,
//! refresh?}` → `202 {job}`, `GET /api/admin/estimates/{job}` and
//! `GET /api/admin/estimates?character_ids=|run_id=` → `{job, running}`.
//! Jobs wait in one FIFO line and run one at a time, as the server's do; a
//! target already queued or running answers that job; 16 waiting refuse
//! more (429). There is no sidecar: each read of a running job finishes one
//! character (a party then takes one more read for its total), and each
//! read of a job waiting behind invented lookups moves it one place up, so
//! e2e gets the same steps every time. Wholly invented numbers.
//!
//! `POST /__mock/estimates {scenario}` sets what later lookups do, until the
//! next reset: `ok` (default), `queued` (waits behind two invented lookups),
//! `assumed` (the first character's critical rate, 89%, assumed at 100%),
//! `busy` (429, 40 s), `critical_rate` (422), `manual_check` (503), `failed`
//! (502), `timeout` (504), `hold` (never finishes: the running state, and
//! later lookups queue behind it), `off` (no scouter URL: every route
//! answers the generic 404). An error fails the last character only: the
//! others are found and the party total covers them. Seeded: a done lookup
//! of MikaNL (`ch-s2`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::clock::{iso_secs, now_secs};
use super::{MoveError, Store};

pub const LABEL: &str = "Estimate from MapleScouter";
pub const NOTE: &str = "Estimate for planning only, not eligibility";
const MAX_BATCH: usize = 6;
const MAX_QUEUED: usize = 16;
/// Lookups a `queued` scenario job waits behind.
const INVENTED_AHEAD: usize = 2;
/// The assumed character's own critical rate.
const ACTUAL_CRIT: f64 = 89.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Ok,
    Queued,
    Assumed,
    Busy,
    CriticalRate,
    ManualCheck,
    Failed,
    Timeout,
    Hold,
    Off,
}

impl Scenario {
    /// The scenarios whose last character fails.
    fn fails(self) -> bool {
        matches!(
            self,
            Self::Busy | Self::CriticalRate | Self::ManualCheck | Self::Failed | Self::Timeout
        )
    }
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
    /// Out of the line and looked up.
    started: bool,
    /// Invented lookups still ahead of it.
    wait: usize,
    /// Reads while running.
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

    /// Reads it takes once running: one per character, plus the party total.
    fn steps(&self) -> usize {
        self.subjects.len() + usize::from(self.party())
    }

    fn finished(&self) -> bool {
        self.cache_hit || (self.started && self.reads >= self.steps())
    }

    /// Queued or running.
    fn active(&self) -> bool {
        !self.finished()
    }

    /// `pending`, `ok` or `failed` for the character at `at`.
    fn outcome(&self, at: usize) -> &'static str {
        if !self.cache_hit && (!self.started || at >= self.reads) {
            "pending"
        } else if self.scenario.fails() && !self.cache_hit && at + 1 == self.subjects.len() {
            "failed"
        } else {
            "ok"
        }
    }

    fn found(&self) -> bool {
        (0..self.subjects.len()).any(|at| self.outcome(at) == "ok")
    }

    fn state(&self) -> &'static str {
        match () {
            _ if self.finished() && self.found() => "done",
            _ if self.finished() => "failed",
            _ if self.started => "running",
            _ => "queued",
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

    /// The character at `at`'s result (whether or not they are found yet).
    fn member(&self, at: usize) -> Value {
        let subject = &self.subjects[at];
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
        let assumed = self.scenario == Scenario::Assumed && at == 0;
        json!({
            "character_id": subject.character_id,
            "ign": subject.ign,
            "preset": "00010",
            "critical_rate_assumed": assumed,
            "critical_rate_actual": assumed.then_some(ACTUAL_CRIT),
            "bosses": bosses,
        })
    }

    /// The characters found so far, with their places in the party.
    fn members(&self) -> Vec<(usize, Value)> {
        (0..self.subjects.len())
            .filter(|at| self.outcome(*at) == "ok")
            .map(|at| (at, self.member(at)))
            .collect()
    }

    fn combined(&self, members: &[(usize, Value)]) -> Option<Vec<Value>> {
        if !self.party() || self.state() != "done" {
            return None;
        }
        let tokens: Vec<String> = if self.run_id.is_some() {
            self.bosses.clone()
        } else {
            CARDS.iter().map(|card| card.0.to_owned()).collect()
        };
        let whole = self.whole();
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
                    for (at, member) in members {
                        match member["bosses"]
                            .as_array()
                            .and_then(|rows| rows.iter().find(|row| row["boss"] == *token))
                        {
                            Some(row) => sum += row["effective_percentage"].as_f64().unwrap_or(0.0),
                            None => issues.push(json!({
                                "character_id": self.subjects[*at].character_id, "issue": "missing_boss",
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

    /// Every character asked for was found (the server then keeps the IGN set).
    fn all_found(&self) -> bool {
        (0..self.subjects.len()).all(|at| self.outcome(at) == "ok")
    }

    /// No member missing and none failed.
    fn whole(&self) -> bool {
        self.missing.is_empty() && self.all_found()
    }

    fn error(&self, at: usize) -> Value {
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
            "character_id": self.subjects[at].character_id,
        })
    }

    /// `position` is the lookups ahead while queued.
    fn json(&self, position: Option<usize>) -> Value {
        let state = self.state();
        let finished = self.finished().then(|| self.started_at + 20);
        let members = self.members();
        let result = (!members.is_empty()).then(|| {
            let combined = self.combined(&members);
            let complete = state == "done"
                && self.whole()
                && combined
                    .as_ref()
                    .is_none_or(|rows| rows.iter().all(|row| row["complete"] == true));
            json!({
                "cache_hit": self.cache_hit,
                "complete": complete,
                "members": members.iter().map(|(_, member)| member).collect::<Vec<_>>(),
                "combined": combined,
            })
        });
        let current = (state == "running" && self.reads < self.subjects.len())
            .then(|| self.subjects[self.reads].character_id.clone());
        let done = (0..self.subjects.len())
            .filter(|at| self.outcome(*at) != "pending")
            .count();
        json!({
            "id": self.id,
            "state": state,
            "position": position,
            "run_id": self.run_id,
            "character_ids": self.character_ids,
            "started_by": {"kind": "admin", "id": "discord:1001"},
            "started_at": iso_secs(self.started_at),
            "finished_at": finished.map(iso_secs),
            "characters": self.subjects.iter().enumerate().map(|(at, s)| {
                let outcome = self.outcome(at);
                json!({
                    "character_id": s.character_id, "member_id": s.member_id, "ign": s.ign,
                    "outcome": outcome,
                    "error": (outcome == "failed").then(|| self.error(at)),
                })
            }).collect::<Vec<_>>(),
            "missing": self.missing,
            "progress": {"done": done, "total": self.subjects.len(), "current": current},
            "fetched_at": (state == "done").then(|| iso_secs(self.fetched_at.unwrap_or(self.started_at + 20))),
            "result": result,
            "error": (state == "failed").then(|| self.error(self.subjects.len() - 1)),
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
                started: true,
                wait: 0,
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
        self.jobs.iter().find(|job| job.started && job.active())
    }

    /// Lookups ahead of a queued job: invented ones, then the active jobs
    /// asked for before it.
    fn position(&self, at: usize) -> Option<usize> {
        let job = &self.jobs[at];
        (job.state() == "queued")
            .then(|| job.wait + self.jobs[..at].iter().filter(|job| job.active()).count())
    }

    fn json(&self, at: usize) -> Value {
        self.jobs[at].json(self.position(at))
    }

    /// With nothing running, the head of the line starts once no invented
    /// lookup is ahead of it.
    fn promote(&mut self) {
        if self.running().is_some() {
            return;
        }
        if let Some(head) = self
            .jobs
            .iter_mut()
            .find(|job| job.active() && !job.started)
            && head.wait == 0
        {
            head.started = true;
        }
    }

    /// `character_id`'s reading for boss `token` from the newest lookup
    /// that found them in the last 24 hours (`null` otherwise), as
    /// readiness reads the server's memory: never starting a lookup.
    pub fn reading(&self, character_id: &str, token: &str, now: i64) -> Value {
        self.jobs
            .iter()
            .filter_map(|job| {
                let at = job.subjects.iter().position(|s| s.character_id == character_id)?;
                let fetched_at = job.fetched_at.unwrap_or(job.started_at + 20);
                (job.outcome(at) == "ok" && now - fetched_at < 24 * 3_600)
                    .then_some((fetched_at, job, at))
            })
            .max_by_key(|(fetched_at, ..)| *fetched_at)
            .and_then(|(fetched_at, job, at)| {
                let member = job.member(at);
                let card = member["bosses"]
                    .as_array()?
                    .iter()
                    .find(|card| card["boss"] == token)?
                    .clone();
                let share = if card["effective_percentage"].is_null() {
                    card["percentage"].clone()
                } else {
                    card["effective_percentage"].clone()
                };
                Some(json!({
                    "percentage": share, "rating": card["rating"],
                    "estimated_minutes": card["estimated_minutes"], "fetched_at": iso_secs(fetched_at),
                    "critical_rate_assumed": member["critical_rate_assumed"],
                    "critical_rate_actual": member["critical_rate_actual"],
                }))
            })
            .unwrap_or(Value::Null)
    }
}

impl Store {
    /// `POST /__mock/estimates {scenario}`.
    pub fn set_estimate_scenario(&mut self, scenario: Scenario) {
        self.estimates.scenario = scenario;
    }

    /// Estimate routes answer the generic 404 while off (or the module is).
    pub fn estimates_on(&self) -> bool {
        self.character_data_on() && self.estimates.on()
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
                    let brought = slot["brings"].as_str();
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
        ids.sort();
        let estimates = &mut self.estimates;
        // The target's queued or running job answers.
        if let Some(at) = estimates
            .jobs
            .iter()
            .position(|job| job.active() && job.same_target(&run_id, &ids))
        {
            return Ok(json!({ "job": estimates.json(at) }));
        }
        let now = now_secs();
        let cached = (!req.refresh)
            .then(|| {
                estimates.jobs.iter().rev().find(|job| {
                    job.same_target(&run_id, &ids) && job.state() == "done" && job.all_found()
                })
            })
            .flatten()
            .map(|job| job.fetched_at.unwrap_or(job.started_at + 20));
        let waiting = estimates
            .jobs
            .iter()
            .filter(|job| job.state() == "queued")
            .count();
        if cached.is_none() && waiting >= MAX_QUEUED {
            return Err(coded(
                429,
                "estimate_queue_full",
                "16 lookups are waiting already. Try again in a few minutes.",
            ));
        }
        // One job per target, as the server keeps them.
        estimates.jobs.retain(|job| !job.same_target(&run_id, &ids));
        let id = format!("est-{}", estimates.next);
        estimates.next += 1;
        let scenario = estimates.scenario;
        estimates.jobs.push(Job {
            id,
            run_id,
            character_ids: ids,
            bosses,
            subjects,
            missing,
            scenario: if cached.is_some() {
                Scenario::Ok
            } else {
                scenario
            },
            started_at: now,
            started: false,
            wait: if scenario == Scenario::Queued && cached.is_none() {
                INVENTED_AHEAD
            } else {
                0
            },
            reads: 0,
            cache_hit: cached.is_some(),
            fetched_at: cached,
        });
        estimates.promote();
        Ok(json!({ "job": estimates.json(estimates.jobs.len() - 1) }))
    }

    /// `GET /api/admin/estimates/{job}`: a running job finishes one more
    /// step, a job behind invented lookups moves one place up.
    pub fn estimate(&mut self, id: &str) -> Result<Value, MoveError> {
        let estimates = &mut self.estimates;
        let at = estimates
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| {
                coded(
                    404,
                    "not_found",
                    "That lookup is gone: results are kept for 24 hours.",
                )
            })?;
        let job = &mut estimates.jobs[at];
        if job.active() && !job.started {
            job.wait = job.wait.saturating_sub(1);
        } else if job.active() && job.scenario != Scenario::Hold {
            job.reads += 1;
        }
        estimates.promote();
        Ok(estimates.json(at))
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
        let estimates = &self.estimates;
        let latest = estimates
            .jobs
            .iter()
            .rposition(|job| job.same_target(&run_id, &ids));
        let running = estimates
            .jobs
            .iter()
            .position(|job| job.started && job.active());
        Ok(json!({
            "job": latest.map(|at| estimates.json(at)),
            "running": running.map(|at| estimates.json(at)),
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

    fn start(s: &mut Store, body: Value) -> Value {
        s.start_estimate(request(body)).ok().unwrap()["job"].clone()
    }

    fn read(s: &mut Store, job: &Value) -> Value {
        s.estimate(job["id"].as_str().unwrap()).ok().unwrap()
    }

    #[test]
    fn a_party_fills_in_one_character_per_read_then_the_total() {
        let mut s = store();
        let started = start(&mut s, json!({"run_id": "r-carling"}));
        assert_eq!(started["state"], "running", "nothing ahead");
        assert_eq!(started["result"], Value::Null, "nobody found yet");
        let total = started["progress"]["total"].as_u64().unwrap() as usize;
        assert!(total >= 2);
        for done in 1..=total {
            let job = read(&mut s, &started);
            assert_eq!(job["state"], "running", "read {done}");
            assert_eq!(job["progress"]["done"], done);
            assert_eq!(job["result"]["members"].as_array().unwrap().len(), done);
            assert_eq!(job["result"]["combined"], Value::Null, "no total yet");
            assert_eq!(job["characters"][done - 1]["outcome"], "ok");
        }
        let done = read(&mut s, &started);
        assert_eq!(done["state"], "done");
        assert_eq!(done["result"]["complete"], false, "Mika has no pick");
        assert!(done["result"]["combined"].is_array());
        // A second lookup answers from memory at once.
        let again = start(&mut s, json!({"run_id": "r-carling"}));
        assert_eq!(again["state"], "done");
        assert_eq!(again["result"]["cache_hit"], true);
    }

    #[test]
    fn lookups_queue_behind_the_running_one_and_dedupe() {
        let mut s = store();
        s.set_estimate_scenario(Scenario::Hold);
        let held = start(&mut s, json!({"character_ids": ["ch-s3"]}));
        s.set_estimate_scenario(Scenario::Ok);
        let first = start(&mut s, json!({"character_ids": ["ch-s7"]}));
        let second = start(&mut s, json!({"run_id": "r-carling"}));
        assert_eq!(held["state"], "running");
        assert_eq!(
            (&first["state"], &first["position"]),
            (&json!("queued"), &json!(1))
        );
        assert_eq!(
            (&second["state"], &second["position"]),
            (&json!("queued"), &json!(2))
        );
        assert_eq!(
            start(&mut s, json!({"character_ids": ["ch-s7"]}))["id"],
            first["id"]
        );
        assert_eq!(read(&mut s, &held)["state"], "running", "held");
        assert_eq!(read(&mut s, &first)["position"], 1, "still behind it");
        let status = s.estimate_status(Some("ch-s7"), None).ok().unwrap();
        assert_eq!(status["running"]["id"], held["id"]);
        assert_eq!(status["job"]["position"], 1);
    }

    #[test]
    fn the_queued_scenario_waits_behind_two_then_runs() {
        let mut s = store();
        s.set_estimate_scenario(Scenario::Queued);
        let job = start(&mut s, json!({"character_ids": ["ch-s3"]}));
        assert_eq!(
            (&job["state"], &job["position"]),
            (&json!("queued"), &json!(2))
        );
        assert_eq!(read(&mut s, &job)["position"], 1);
        assert_eq!(read(&mut s, &job)["state"], "running");
        assert_eq!(read(&mut s, &job)["state"], "done");
    }

    #[test]
    fn the_line_holds_sixteen() {
        let mut s = store();
        s.set_estimate_scenario(Scenario::Queued);
        let ids: Vec<String> = s
            .characters
            .iter()
            .filter(|c| c.world == "msea")
            .map(|c| c.id.clone())
            .collect();
        // Distinct targets: single characters, then pairs.
        let mut targets: Vec<Vec<&String>> = ids.iter().map(|id| vec![id]).collect();
        for (at, a) in ids.iter().enumerate() {
            targets.extend(ids[at + 1..].iter().map(|b| vec![a, b]));
        }
        assert!(targets.len() > MAX_QUEUED, "enough seeded characters");
        for target in &targets[..MAX_QUEUED] {
            // `refresh`: the seeded MikaNL lookup would answer from memory.
            let job = start(&mut s, json!({"character_ids": target, "refresh": true}));
            assert_eq!(job["state"], "queued");
        }
        let refused = s.start_estimate(request(
            json!({"character_ids": targets[MAX_QUEUED], "refresh": true}),
        ));
        assert!(matches!(
            refused,
            Err(MoveError::Coded(429, "estimate_queue_full", _))
        ));
    }

    #[test]
    fn an_error_fails_the_last_character_only() {
        for (scenario, code) in [
            (Scenario::Busy, "busy"),
            (Scenario::CriticalRate, "critical_rate"),
            (Scenario::ManualCheck, "manual_check"),
            (Scenario::Failed, "failed"),
            (Scenario::Timeout, "failed"),
        ] {
            let mut s = store();
            s.set_estimate_scenario(scenario);
            let one = start(&mut s, json!({"character_ids": ["ch-s3"], "refresh": true}));
            let failed = read(&mut s, &one);
            assert_eq!(
                (&failed["state"], &failed["error"]["code"]),
                (&json!("failed"), &json!(code))
            );
            let party = start(&mut s, json!({"run_id": "r-carling"}));
            let mut job = party.clone();
            while job["state"] != "done" && job["state"] != "failed" {
                job = read(&mut s, &party);
            }
            assert_eq!(job["state"], "done", "{scenario:?}: the others are found");
            let rows = job["characters"].as_array().unwrap();
            assert_eq!(rows.last().unwrap()["outcome"], "failed");
            assert_eq!(rows.last().unwrap()["error"]["code"], code);
            assert!(
                rows[..rows.len() - 1]
                    .iter()
                    .all(|row| row["outcome"] == "ok")
            );
            assert_eq!(
                job["result"]["members"].as_array().unwrap().len(),
                rows.len() - 1
            );
            assert_eq!(job["result"]["complete"], false);
            assert_eq!(job["error"], Value::Null);
        }
        let mut s = store();
        s.set_estimate_scenario(Scenario::Off);
        assert!(!s.estimates_on());
    }

    #[test]
    fn the_assumed_scenario_labels_the_first_character() {
        let mut s = store();
        s.set_estimate_scenario(Scenario::Assumed);
        let job = start(&mut s, json!({"run_id": "r-carling"}));
        let first = read(&mut s, &job);
        let member = &first["result"]["members"][0];
        assert_eq!(
            (
                &member["critical_rate_assumed"],
                &member["critical_rate_actual"]
            ),
            (&json!(true), &json!(89.0))
        );
        let second = read(&mut s, &job);
        assert_eq!(
            second["result"]["members"][1]["critical_rate_assumed"],
            false
        );
    }
}
