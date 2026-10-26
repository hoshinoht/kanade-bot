//! `character_data.json` estimates: scouter lookup jobs (module
//! `character_data` with `KANADE_SCOUTER_URL`). Bosses are catalog tokens
//! (`HLimbo`); the scouter's other cards are left out. Every job carries
//! the binding labels "Clear-time estimate" and "For planning only, not
//! eligibility"; a member whose critical rate was assumed says so
//! (`critical_rate_assumed`). A result fetched at least `estimate_days`
//! ago is `stale` ("older than n days").

use std::collections::BTreeMap;

use serde::Serialize;

use super::{history::SettingsActor, iso_instant};
use crate::infrastructure::scouter::{
    BossParty, Card, Combined, Estimates, Failure, FailureKind, Found, Job, Member, Subject,
    Target, catalog_token, saved_party, scouter_keys,
};
use kanade_domain::{catalog::BossTable, characters::PartySlot};

pub const LABEL: &str = "Clear-time estimate";
pub const NOTE: &str = "For planning only, not eligibility";

/// A run's Party total from the saved estimates: the same figures as the
/// reminder card's "Who carries" image (each boss its own party's summed
/// effective %, minutes only from a saved total of that boss's whole
/// party). Never starts a lookup.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunPartyTotal {
    /// When the oldest reading used was fetched; `null` with none saved.
    pub as_of: Option<String>,
    /// `as_of` is at least `fresh_days` ago ("older than n days").
    pub stale: bool,
    /// The saved `estimate_days`.
    pub fresh_days: u32,
    /// One per boss of the run, in run order.
    pub bosses: Vec<PartyTotalBoss>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyTotalBoss {
    /// The boss token as the run lists it (`HCarling`).
    pub boss: String,
    /// The sum of the members' saved effective %; `null`: no estimate yet.
    pub combined_percentage: Option<f64>,
    /// From the saved total of this boss's whole party; `null` while a
    /// member brings no character the scouter can take, or none is saved.
    pub estimated_minutes: Option<f64>,
    /// From the same saved total; `null` without it.
    pub technically_possible: Option<bool>,
    /// Every party member has a saved effective % for this boss.
    pub complete: bool,
    /// Some figure counted assumes a 100% critical rate.
    pub critical_rate_assumed: bool,
    /// Party members with no estimate for this boss (not in the total,
    /// never 0%).
    pub no_estimate: u32,
}

/// `slots` (of a run with `bosses`) as the Party total from `estimates`'
/// saved readings.
pub fn party_total(estimates: &Estimates, bosses: &[String], slots: &[PartySlot]) -> RunPartyTotal {
    let saved = saved_party(
        bosses,
        slots,
        |ign| estimates.cached(ign),
        |igns| estimates.cached_party(igns),
    );
    let party = &saved.party;
    RunPartyTotal {
        as_of: saved.oldest.map(iso_instant),
        stale: saved.oldest.is_some_and(|at| estimates.stale(at)),
        fresh_days: estimates.fresh_days(),
        bosses: bosses
            .iter()
            .zip(&saved.totals)
            .enumerate()
            .map(|(at, (token, total))| PartyTotalBoss {
                boss: token.clone(),
                combined_percentage: party.total(at),
                estimated_minutes: total.as_ref().and_then(|total| total.estimated_minutes),
                technically_possible: total.as_ref().and_then(|total| total.technically_possible),
                complete: party.complete(at),
                critical_rate_assumed: party.crit_at(at),
                no_estimate: count(party.no_estimate_at(at)),
            })
            .collect(),
    }
}

/// `GET /api/admin/estimates?character_ids=|run_id=`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateStatus {
    /// The newest job for exactly this target (queued, running, or
    /// finished in the last 24 hours); `null` without one.
    pub job: Option<EstimateJob>,
    /// The job running now, whatever it looks up (the others wait in line).
    pub running: Option<EstimateJob>,
}

/// `POST /api/admin/estimates` → `202`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateStarted {
    pub job: EstimateJob,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateJob {
    pub id: String,
    #[cfg_attr(test, ts(type = "'queued' | 'running' | 'done' | 'failed'"))]
    pub state: &'static str,
    /// While `queued`: the lookups ahead of it, the running one included
    /// (1 or more); `null` otherwise.
    pub position: Option<u32>,
    /// A run's party; `null` for a character lookup.
    pub run_id: Option<String>,
    /// The characters asked for; empty for a run.
    pub character_ids: Vec<String>,
    pub started_by: SettingsActor,
    /// When it was asked for.
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Who is looked up, in party order (roster IGNs), each with their
    /// outcome as it arrives: every character brought to any boss, once.
    pub characters: Vec<EstimateCharacter>,
    /// Party members not looked up for some boss (listed as missing, never
    /// 0%).
    pub missing: Vec<EstimateMissing>,
    /// A run's distinct boss parties, each with the bosses it plays and
    /// the characters it brings (one when every boss brings the same
    /// characters); empty for a lookup of characters.
    pub parties: Vec<EstimateParty>,
    pub progress: EstimateProgress,
    /// When the result came from the scouter (answered from a stored
    /// result: the oldest one it used); `null` until the job is done.
    pub fetched_at: Option<String>,
    /// `fetched_at` is at least `fresh_days` ago: show the result (members
    /// and party total) marked "older than n days". `false` until done,
    /// and for a job answered by a fresh lookup.
    pub stale: bool,
    /// The saved `estimate_days`.
    pub fresh_days: u32,
    /// The characters found so far (while queued or running too) and,
    /// once done, the party total; `null` with none found.
    pub result: Option<EstimateResult>,
    /// `failed`: why nobody was found. `done`: why the party total is
    /// missing although characters were found (`character_id` null).
    pub error: Option<EstimateError>,
    /// `Clear-time estimate`.
    pub label: &'static str,
    /// `For planning only, not eligibility`.
    pub note: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateCharacter {
    pub character_id: String,
    pub member_id: String,
    pub ign: String,
    /// `pending` until looked up; `ok` (in `result.members`) or `failed`
    /// (`error`). One character's failure never stops the others.
    #[cfg_attr(test, ts(type = "'pending' | 'ok' | 'failed'"))]
    pub outcome: &'static str,
    /// Why this character has no result; `null` unless `failed`.
    pub error: Option<EstimateError>,
}

/// The characters a run brings to some of its bosses; each of those
/// bosses' totals is this party's.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateParty {
    /// Catalog tokens, in run order.
    pub bosses: Vec<String>,
    /// Its characters, in party order.
    pub character_ids: Vec<String>,
    /// How many of its members bring no character the scouter can take.
    pub missing: u32,
}
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateMissing {
    pub member_id: String,
    /// The character they bring, when there is one.
    pub character_id: Option<String>,
    #[cfg_attr(
        test,
        ts(type = "'no_characters' | 'no_character_picked' | 'other_region' | 'unsupported_ign'")
    )]
    pub reason: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateProgress {
    /// Characters finished so far, found or failed.
    pub done: u32,
    pub total: u32,
    /// The character being looked up now; `null` while queued, between
    /// characters, for the party total and once finished.
    pub current: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateResult {
    /// Answered from a stored result younger than `estimate_days` without
    /// calling the scouter.
    pub cache_hit: bool,
    /// Done, no party member missing or failed, and every total complete.
    pub complete: bool,
    /// The characters found, in party order.
    pub members: Vec<EstimateMember>,
    /// The party totals over the characters found (a run's bosses, or
    /// every catalog boss for several characters); `null` for one
    /// character, until the job is done, or when the total failed.
    pub combined: Option<Vec<EstimateCombined>>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateMember {
    pub character_id: String,
    pub ign: String,
    /// The preset the scouter read (`00010`).
    pub preset: String,
    /// The character's critical rate is under 100% and the scouter
    /// assumed exactly 100%: label it "Assumes 100% critical rate (actual
    /// N%)" wherever its numbers show.
    pub critical_rate_assumed: bool,
    /// The character's own critical rate with an assumed result; `null`
    /// otherwise, or when the scouter didn't say.
    pub critical_rate_actual: Option<f64>,
    /// Catalog bosses only, in catalog order.
    pub bosses: Vec<EstimateBoss>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateBoss {
    /// Catalog token, e.g. `HLimbo`.
    pub boss: String,
    /// As the scouter shows it (a `[Party]` card's whole-party figure).
    pub percentage: f64,
    /// This character's share; `null` when the scouter can't tell.
    pub effective_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub rating: String,
    /// A `[Party]` card.
    pub party: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateCombined {
    pub boss: String,
    /// `null` unless every member found has a known share.
    pub combined_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub technically_possible: Option<bool>,
    /// `false` with a missing or failed member or an unknown share.
    pub complete: bool,
    pub issues: Vec<EstimateIssue>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateIssue {
    /// `null` for `not_covered`.
    pub character_id: Option<String>,
    /// `missing_boss`: no card for this boss; `unknown_benchmark_players`:
    /// a `[Party]` card with an unknown party size; `not_covered`:
    /// the scouter has no card for this boss at all.
    #[cfg_attr(
        test,
        ts(type = "'missing_boss' | 'unknown_benchmark_players' | 'not_covered' | 'other'")
    )]
    pub issue: &'static str,
}

/// The drawn error states.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateError {
    /// `busy` (429 past the job's deadline, or a rate limit; retry after
    /// `retry_after_seconds`), `critical_rate` (422), `assumption_failed`
    /// (422: the scouter tried assuming 100% critical rate and couldn't
    /// finish), `manual_check` (503), `failed` (502, 504, timeout, no
    /// connection).
    #[cfg_attr(
        test,
        ts(type = "'busy' | 'critical_rate' | 'assumption_failed' | 'manual_check' | 'failed'")
    )]
    pub code: &'static str,
    pub message: &'static str,
    /// The scouter's HTTP status; `null` without an answer.
    pub status: Option<u16>,
    /// The scouter's error code (`busy`, `upstream_rate_limited`,
    /// `lookup_timeout`, `result_mismatch`, …) or `timeout`, `unreachable`,
    /// `unreadable`.
    pub detail: String,
    pub retry_after_seconds: Option<u32>,
    /// The character it is about, when known.
    pub character_id: Option<String>,
}

fn message(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Busy => "The scouter is busy.",
        FailureKind::CriticalRate => "Set up critical rate on the scouter first.",
        FailureKind::AssumptionFailed => {
            "The estimate couldn't be completed for this character. Try again later."
        }
        FailureKind::ManualCheck => "Scouter needs a manual check.",
        FailureKind::Failed => "Lookup failed.",
    }
}

/// Catalog order: boss, then difficulty.
fn rank(catalog: &BossTable, token: &str) -> (usize, usize) {
    catalog
        .split(token)
        .map_or((usize::MAX, usize::MAX), |(difficulty, boss)| {
            (
                catalog
                    .bosses()
                    .iter()
                    .position(|row| row.short() == boss.short())
                    .unwrap_or(usize::MAX),
                catalog
                    .difficulties()
                    .iter()
                    .position(|row| row.letter() == difficulty.letter())
                    .unwrap_or(usize::MAX),
            )
        })
}

fn subject_of<'a>(subjects: &'a [Subject], ign: &str) -> Option<&'a Subject> {
    let key = ign.to_ascii_lowercase();
    subjects.iter().find(|subject| subject.ign.key() == key)
}

fn boss(token: &str, card: &Card) -> EstimateBoss {
    EstimateBoss {
        boss: token.to_owned(),
        percentage: card.percentage,
        effective_percentage: card.effective_percentage,
        estimated_minutes: card.estimated_minutes,
        rating: card.rating.clone(),
        party: card.party,
    }
}

fn member(subject: &Subject, member: &Member, catalog: &BossTable) -> EstimateMember {
    let mut bosses: Vec<EstimateBoss> = Vec::new();
    for (key, card) in &member.bosses {
        if let Some(token) = catalog_token(key, catalog)
            && !bosses.iter().any(|row| row.boss == token)
        {
            bosses.push(boss(token, card));
        }
    }
    bosses.sort_by_key(|row| rank(catalog, &row.boss));
    let assumed = member.crit_assumed();
    EstimateMember {
        character_id: subject.character_id.clone(),
        ign: subject.ign.as_str().to_owned(),
        preset: member.preset.clone(),
        critical_rate_assumed: assumed,
        critical_rate_actual: member.critical_rate_actual.filter(|_| assumed),
        bosses,
    }
}

fn combined(
    token: &str,
    total: &Combined,
    subjects: &[Subject],
    party_whole: bool,
) -> EstimateCombined {
    EstimateCombined {
        boss: token.to_owned(),
        combined_percentage: total.combined_percentage,
        estimated_minutes: total.estimated_minutes,
        technically_possible: total.technically_possible,
        complete: total.complete && party_whole,
        issues: total
            .issues
            .iter()
            .map(|(ign, issue)| EstimateIssue {
                character_id: subject_of(subjects, ign).map(|subject| subject.character_id.clone()),
                issue: match issue.as_str() {
                    "missing_boss" => "missing_boss",
                    "unknown_benchmark_players" => "unknown_benchmark_players",
                    _ => "other",
                },
            })
            .collect(),
    }
}

/// A run boss with no total: nobody found has the card, or the scouter
/// has none.
fn uncovered(token: &str, found: &[&Subject]) -> EstimateCombined {
    let issues = if scouter_keys(token).next().is_none() {
        vec![EstimateIssue {
            character_id: None,
            issue: "not_covered",
        }]
    } else {
        found
            .iter()
            .map(|subject| EstimateIssue {
                character_id: Some(subject.character_id.clone()),
                issue: "missing_boss",
            })
            .collect()
    };
    EstimateCombined {
        boss: token.to_owned(),
        combined_percentage: None,
        estimated_minutes: None,
        technically_possible: None,
        complete: false,
        issues,
    }
}

/// The party playing `token` (the first for a lookup of characters).
fn party_of<'j>(job: &'j Job, token: &str) -> Option<&'j BossParty> {
    job.parties
        .iter()
        .find(|party| party.bosses.iter().any(|boss| boss == token))
        .or_else(|| job.parties.first())
}

/// `party`'s characters that were found.
fn found_in<'j>(job: &'j Job, party: Option<&BossParty>) -> Vec<&'j Subject> {
    job.subjects
        .iter()
        .zip(&job.results)
        .enumerate()
        .filter(|(at, (_, result))| {
            matches!(result, Some(Ok(_))) && party.is_none_or(|party| party.subjects.contains(at))
        })
        .map(|(_, (subject, _))| subject)
        .collect()
}

/// `party` has no member missing and none of its characters failed; a
/// party of every boss (or none) is the whole job.
fn party_whole(job: &Job, party: Option<&BossParty>) -> bool {
    let failed = |at: &usize| matches!(job.results.get(*at), Some(Some(Err(_))));
    match party.filter(|party| !party.bosses.is_empty()) {
        Some(party) => party.missing == 0 && !party.subjects.iter().any(failed),
        None => whole(job),
    }
}

fn totals(
    job: &Job,
    totals: &BTreeMap<String, Combined>,
    catalog: &BossTable,
) -> Vec<EstimateCombined> {
    let subjects = &job.subjects;
    match &job.target {
        Target::Run { bosses, .. } => {
            let mut tokens: Vec<&str> = Vec::new();
            for token in bosses {
                if catalog.split(token).is_some() && !tokens.contains(&token.as_str()) {
                    tokens.push(token);
                }
            }
            tokens
                .into_iter()
                .map(|token| {
                    let party = party_of(job, token);
                    scouter_keys(token)
                        .find_map(|key| totals.get(key))
                        .map_or_else(
                            || uncovered(token, &found_in(job, party)),
                            |total| combined(token, total, subjects, party_whole(job, party)),
                        )
                })
                .collect()
        }
        Target::Characters(_) => {
            let party_whole = whole(job);
            let mut rows: Vec<EstimateCombined> = Vec::new();
            for (key, total) in totals {
                if let Some(token) = catalog_token(key, catalog)
                    && !rows.iter().any(|row| row.boss == token)
                {
                    rows.push(combined(token, total, subjects, party_whole));
                }
            }
            rows.sort_by_key(|row| rank(catalog, &row.boss));
            rows
        }
    }
}

/// No party member missing and none failed.
fn whole(job: &Job) -> bool {
    job.missing.is_empty()
        && !job
            .results
            .iter()
            .any(|result| matches!(result, Some(Err(_))))
}

/// The characters found so far and, once done, the party total; `None`
/// with nobody found.
fn result(job: &Job, found: Option<&Found>, catalog: &BossTable) -> Option<EstimateResult> {
    let members: Vec<EstimateMember> = job
        .subjects
        .iter()
        .zip(&job.results)
        .filter_map(|(subject, result)| match result {
            Some(Ok(found)) => Some(member(subject, found, catalog)),
            _ => None,
        })
        .collect();
    if members.is_empty() {
        return None;
    }
    let party_whole = whole(job);
    let combined = found
        .and_then(|found| found.combined.as_ref())
        .map(|combined| totals(job, combined, catalog));
    let complete = found.is_some_and(|found| found.total_failure.is_none())
        && party_whole
        && combined
            .as_ref()
            .is_none_or(|rows| rows.iter().all(|row| row.complete));
    Some(EstimateResult {
        cache_hit: found.is_some_and(|found| found.cache_hit),
        complete,
        members,
        combined,
    })
}

fn error(job: &Job, failure: &Failure) -> EstimateError {
    EstimateError {
        code: failure.kind.as_str(),
        message: message(failure.kind),
        status: failure.status,
        detail: failure.detail.clone(),
        retry_after_seconds: failure.retry_after,
        character_id: failure
            .subject
            .and_then(|at| job.subjects.get(at))
            .map(|subject| subject.character_id.clone()),
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// `job` as the API shows it, `stale` against `estimates`' saved
/// `estimate_days`.
pub fn job(job: &Job, catalog: &BossTable, estimates: &Estimates) -> EstimateJob {
    let (run_id, character_ids) = match &job.target {
        Target::Run { run_id, .. } => (Some(run_id.clone()), Vec::new()),
        Target::Characters(ids) => (None, ids.clone()),
    };
    let (fetched_at, found, failure) = match &job.outcome {
        None => (None, None, None),
        Some(Ok(found)) => (
            Some(found.fetched_at),
            Some(found),
            found.total_failure.as_ref(),
        ),
        Some(Err(failure)) => (None, None, Some(failure)),
    };
    EstimateJob {
        id: job.id.clone(),
        state: job.state().as_str(),
        position: job.position.map(count),
        run_id,
        character_ids,
        started_by: SettingsActor {
            kind: job.started_by.kind(),
            id: job.started_by.id().to_owned(),
        },
        started_at: iso_instant(job.started_at),
        finished_at: job.finished_at.map(iso_instant),
        characters: job
            .subjects
            .iter()
            .zip(&job.results)
            .map(|(subject, outcome)| EstimateCharacter {
                character_id: subject.character_id.clone(),
                member_id: subject.member_id.clone(),
                ign: subject.ign.as_str().to_owned(),
                outcome: match outcome {
                    None => "pending",
                    Some(Ok(_)) => "ok",
                    Some(Err(_)) => "failed",
                },
                error: match outcome {
                    Some(Err(failure)) => Some(error(job, failure)),
                    _ => None,
                },
            })
            .collect(),
        missing: job
            .missing
            .iter()
            .map(|missing| EstimateMissing {
                member_id: missing.member_id.clone(),
                character_id: missing.character_id.clone(),
                reason: missing.reason.as_str(),
            })
            .collect(),
        parties: match &job.target {
            Target::Run { .. } => job
                .parties
                .iter()
                .map(|party| EstimateParty {
                    bosses: party.bosses.clone(),
                    character_ids: party
                        .subjects
                        .iter()
                        .filter_map(|at| job.subjects.get(*at))
                        .map(|subject| subject.character_id.clone())
                        .collect(),
                    missing: count(party.missing),
                })
                .collect(),
            Target::Characters(_) => Vec::new(),
        },
        progress: EstimateProgress {
            done: count(job.done()),
            total: count(job.subjects.len()),
            current: job
                .current
                .and_then(|at| job.subjects.get(at))
                .map(|subject| subject.character_id.clone()),
        },
        fetched_at: fetched_at.map(iso_instant),
        stale: fetched_at.is_some_and(|at| estimates.stale(at)),
        fresh_days: estimates.fresh_days(),
        result: result(job, found, catalog),
        error: failure.map(|failure| error(job, failure)),
        label: LABEL,
        note: NOTE,
    }
}
