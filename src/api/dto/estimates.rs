//! `character_data.json` estimates: scouter lookup jobs (module
//! `character_data` with `KANADE_SCOUTER_URL`). Bosses are catalog tokens
//! (`HLimbo`); the scouter's other cards are left out. Every job carries
//! the binding labels "Clear-time estimate" and "For planning only, not
//! eligibility"; a member whose critical rate was assumed says so
//! (`critical_rate_assumed`).

use std::collections::BTreeMap;

use serde::Serialize;

use super::{history::SettingsActor, iso_instant};
use crate::{
    domain::catalog::BossTable,
    infrastructure::scouter::{
        Card, Combined, Failure, FailureKind, Found, Job, Member, Subject, Target, catalog_token,
        scouter_keys,
    },
};

pub const LABEL: &str = "Clear-time estimate";
pub const NOTE: &str = "For planning only, not eligibility";

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
    /// outcome as it arrives.
    pub characters: Vec<EstimateCharacter>,
    /// Party members not looked up (listed as missing, never 0%).
    pub missing: Vec<EstimateMissing>,
    pub progress: EstimateProgress,
    /// When the result came from the scouter (a cached result keeps its
    /// first time); `null` until the job is done.
    pub fetched_at: Option<String>,
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
    /// Answered from kanade's 24-hour memory without calling the scouter.
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
    /// `retry_after_seconds`), `critical_rate` (422), `manual_check` (503),
    /// `failed` (502, 504, timeout, no connection).
    #[cfg_attr(
        test,
        ts(type = "'busy' | 'critical_rate' | 'manual_check' | 'failed'")
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

fn totals(
    job: &Job,
    totals: &BTreeMap<String, Combined>,
    party_whole: bool,
    catalog: &BossTable,
) -> Vec<EstimateCombined> {
    let subjects = &job.subjects;
    match &job.target {
        Target::Run { bosses, .. } => {
            let found: Vec<&Subject> = subjects
                .iter()
                .zip(&job.results)
                .filter(|(_, result)| matches!(result, Some(Ok(_))))
                .map(|(subject, _)| subject)
                .collect();
            let mut tokens: Vec<&str> = Vec::new();
            for token in bosses {
                if catalog.split(token).is_some() && !tokens.contains(&token.as_str()) {
                    tokens.push(token);
                }
            }
            tokens
                .into_iter()
                .map(|token| {
                    scouter_keys(token)
                        .find_map(|key| totals.get(key))
                        .map_or_else(
                            || uncovered(token, &found),
                            |total| combined(token, total, subjects, party_whole),
                        )
                })
                .collect()
        }
        Target::Characters(_) => {
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
fn party_whole(job: &Job) -> bool {
    job.missing.is_empty()
        && !job
            .results
            .iter()
            .any(|result| matches!(result, Some(Err(_))))
}

/// A finished run lookup's party totals per run boss, and when they came
/// from the scouter; `None` until it is done, or when the total failed.
pub fn run_totals(job: &Job, catalog: &BossTable) -> Option<(String, Vec<EstimateCombined>)> {
    let Some(Ok(found)) = &job.outcome else {
        return None;
    };
    let combined = found.combined.as_ref()?;
    Some((
        iso_instant(found.fetched_at),
        totals(job, combined, party_whole(job), catalog),
    ))
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
    let party_whole = party_whole(job);
    let combined = found
        .and_then(|found| found.combined.as_ref())
        .map(|combined| totals(job, combined, party_whole, catalog));
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

pub fn job(job: &Job, catalog: &BossTable) -> EstimateJob {
    let (run_id, character_ids) = match &job.target {
        Target::Run { run_id, .. } => (Some(run_id.clone()), Vec::new()),
        Target::Characters(ids) => (None, ids.clone()),
    };
    let (fetched_at, found, failure) = match &job.outcome {
        None => (None, None, None),
        Some(Ok(found)) => (
            Some(iso_instant(found.fetched_at)),
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
        progress: EstimateProgress {
            done: count(job.done()),
            total: count(job.subjects.len()),
            current: job
                .current
                .and_then(|at| job.subjects.get(at))
                .map(|subject| subject.character_id.clone()),
        },
        fetched_at,
        result: result(job, found, catalog),
        error: failure.map(|failure| error(job, failure)),
        label: LABEL,
        note: NOTE,
    }
}
