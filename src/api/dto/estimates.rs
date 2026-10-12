//! `character_data.json` estimates: MapleScouter lookup jobs (module
//! `character_data` with `KANADE_SCOUTER_URL`). Bosses are catalog tokens
//! (`HLimbo`); MapleScouter's other cards are left out. Every job carries
//! the binding labels "Estimate from MapleScouter" and "Estimate for
//! planning only, not eligibility".

use serde::Serialize;

use super::{history::SettingsActor, iso_instant};
use crate::{
    domain::catalog::BossTable,
    infrastructure::scouter::{
        Card, Combined, Estimate, Failure, FailureKind, Job, Subject, Target, catalog_token,
        scouter_keys,
    },
};

pub const LABEL: &str = "Estimate from MapleScouter";
pub const NOTE: &str = "Estimate for planning only, not eligibility";

/// `GET /api/admin/estimates?character_ids=|run_id=`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateStatus {
    /// The newest job for exactly this target (running, or finished in the
    /// last 24 hours); `null` without one.
    pub job: Option<EstimateJob>,
    /// The job running now, whatever it looks up (one runs at a time).
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
    #[cfg_attr(test, ts(type = "'running' | 'done' | 'failed'"))]
    pub state: &'static str,
    /// A run's party; `null` for a character lookup.
    pub run_id: Option<String>,
    /// The characters asked for; empty for a run.
    pub character_ids: Vec<String>,
    pub started_by: SettingsActor,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Who is looked up, in party order (roster IGNs).
    pub characters: Vec<EstimateCharacter>,
    /// Party members not looked up (listed as missing, never 0%).
    pub missing: Vec<EstimateMissing>,
    pub progress: EstimateProgress,
    /// When the result came from MapleScouter (a cached result keeps its
    /// first time); `null` without a result.
    pub fetched_at: Option<String>,
    pub result: Option<EstimateResult>,
    pub error: Option<EstimateError>,
    /// `Estimate from MapleScouter`.
    pub label: &'static str,
    /// `Estimate for planning only, not eligibility`.
    pub note: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateCharacter {
    pub character_id: String,
    pub member_id: String,
    pub ign: String,
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
    /// Characters looked up so far.
    pub done: u32,
    pub total: u32,
    /// The character being looked up now; `null` between characters, for
    /// the party total and once finished.
    pub current: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateResult {
    /// Answered from kanade's 24-hour memory without calling MapleScouter.
    pub cache_hit: bool,
    /// No party member missing and every total complete.
    pub complete: bool,
    pub members: Vec<EstimateMember>,
    /// The party totals (a run's bosses, or every catalog boss for several
    /// characters); `null` for one character.
    pub combined: Option<Vec<EstimateCombined>>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateMember {
    pub character_id: String,
    pub ign: String,
    /// The preset MapleScouter read (`00010` unless it fell back for
    /// critical rate).
    pub preset: String,
    /// Catalog bosses only, in catalog order.
    pub bosses: Vec<EstimateBoss>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EstimateBoss {
    /// Catalog token, e.g. `HLimbo`.
    pub boss: String,
    /// As MapleScouter shows it (a `[Party]` card's whole-party figure).
    pub percentage: f64,
    /// This character's share; `null` when MapleScouter can't tell.
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
    /// `null` unless every looked-up member has a known share.
    pub combined_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub technically_possible: Option<bool>,
    /// `false` with a missing member or an unknown share.
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
    /// MapleScouter has no card for this boss at all.
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
    /// `busy` (429; retry after `retry_after_seconds`), `critical_rate`
    /// (422), `manual_check` (503), `failed` (502, 504, timeout, no
    /// connection).
    #[cfg_attr(
        test,
        ts(type = "'busy' | 'critical_rate' | 'manual_check' | 'failed'")
    )]
    pub code: &'static str,
    pub message: &'static str,
    /// MapleScouter's HTTP status; `null` without an answer.
    pub status: Option<u16>,
    /// MapleScouter's error code (`busy`, `upstream_rate_limited`,
    /// `lookup_timeout`, `result_mismatch`, …) or `timeout`, `unreachable`,
    /// `unreadable`.
    pub detail: String,
    pub retry_after_seconds: Option<u32>,
    /// The character it stopped at, when known.
    pub character_id: Option<String>,
}

fn message(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Busy => "MapleScouter is busy.",
        FailureKind::CriticalRate => "Set up critical rate on MapleScouter first.",
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

/// A run boss with no total: nobody has the card, or MapleScouter has none.
fn uncovered(token: &str, subjects: &[Subject]) -> EstimateCombined {
    let issues = if scouter_keys(token).next().is_none() {
        vec![EstimateIssue {
            character_id: None,
            issue: "not_covered",
        }]
    } else {
        subjects
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

fn result(job: &Job, estimate: &Estimate, cache_hit: bool, catalog: &BossTable) -> EstimateResult {
    let subjects = &job.subjects;
    let party_whole = job.missing.is_empty();
    let members = subjects
        .iter()
        .filter_map(|subject| {
            let member = estimate
                .members
                .iter()
                .find(|member| member.ign.eq_ignore_ascii_case(subject.ign.as_str()))?;
            let mut bosses: Vec<EstimateBoss> = Vec::new();
            for (key, card) in &member.bosses {
                if let Some(token) = catalog_token(key, catalog)
                    && !bosses.iter().any(|row| row.boss == token)
                {
                    bosses.push(boss(token, card));
                }
            }
            bosses.sort_by_key(|row| rank(catalog, &row.boss));
            Some(EstimateMember {
                character_id: subject.character_id.clone(),
                ign: subject.ign.as_str().to_owned(),
                preset: member.preset.clone(),
                bosses,
            })
        })
        .collect();
    let combined = estimate.combined.as_ref().map(|totals| match &job.target {
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
                    scouter_keys(token)
                        .find_map(|key| totals.get(key))
                        .map_or_else(
                            || uncovered(token, subjects),
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
    });
    EstimateResult {
        cache_hit,
        complete: party_whole
            && combined
                .as_ref()
                .is_none_or(|rows: &Vec<EstimateCombined>| rows.iter().all(|row| row.complete)),
        members,
        combined,
    }
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
    let (fetched_at, result, error) = match &job.outcome {
        None => (None, None, None),
        Some(Ok(found)) => (
            Some(iso_instant(found.fetched_at)),
            Some(result(job, &found.estimate, found.cache_hit, catalog)),
            None,
        ),
        Some(Err(failure)) => (None, None, Some(error(job, failure))),
    };
    EstimateJob {
        id: job.id.clone(),
        state: job.state().as_str(),
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
            .map(|subject| EstimateCharacter {
                character_id: subject.character_id.clone(),
                member_id: subject.member_id.clone(),
                ign: subject.ign.as_str().to_owned(),
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
            done: count(job.done),
            total: count(job.subjects.len()),
            current: job
                .current
                .and_then(|at| job.subjects.get(at))
                .map(|subject| subject.character_id.clone()),
        },
        fetched_at,
        result,
        error,
        label: LABEL,
        note: NOTE,
    }
}
