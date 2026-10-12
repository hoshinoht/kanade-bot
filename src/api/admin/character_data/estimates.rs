//! MapleScouter estimates (module `character_data` with
//! `KANADE_SCOUTER_URL`): admin only, on demand, one lookup at a time, run
//! as a job because a lookup takes up to two minutes. While the module is
//! off or the URL is unset, every route answers the generic `404` before
//! the session is checked, as an unmounted path does. `POST` takes the
//! session's CSRF token. Nothing is stored: jobs and results live 24 hours
//! in memory. MapleScouter is sent roster IGNs only.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path as UrlPath, State, rejection::JsonRejection},
    http::{StatusCode, Uri, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

use super::super::{
    characters::party,
    write::{Refusal, bad_body, state},
};
use crate::{
    api::{
        auth::{AdminSession, wire},
        dto::estimates::{EstimateStarted, EstimateStatus, job},
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::characters::{Character, DEFAULT_WORLD},
    infrastructure::{
        character_data::CharacterData,
        scouter::{
            Estimates, JobRequest, MAX_BATCH, Missing, MissingReason, RosterIgn, Start, Subject,
            Target,
        },
    },
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/estimates", get(status).post(start))
        .route("/api/admin/estimates/{job}", get(read))
}

type Reply = Result<Response, Refusal>;

/// The module with estimates on; the generic `404` otherwise. Taken first,
/// so an off module or an unset URL answers as an unmounted path.
struct Scouter(Arc<CharacterData>);

impl FromRequestParts<Arc<Site>> for Scouter {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        site.state
            .as_ref()
            .and_then(|state| state.character_data.clone())
            .filter(|module| module.estimates().is_some())
            .map(Self)
            .ok_or(ApiError::NOT_FOUND)
    }
}

impl Scouter {
    fn estimates(&self) -> Result<&Estimates, Refusal> {
        self.0.estimates().ok_or_else(|| ApiError::NOT_FOUND.into())
    }
}

fn unavailable<E>(_: E) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

fn roster(state: &ApiState) -> Result<&Arc<dyn CharacterRoster>, Refusal> {
    // The module needs the roster; without it there is nothing to look up.
    state
        .characters
        .as_ref()
        .ok_or_else(|| ApiError::NOT_FOUND.into())
}

fn not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "That character is not on the roster.",
    )
}

/// What one character is to MapleScouter: an IGN to look up, or why not.
/// Readiness reads the cache under the same IGN.
pub(super) fn lookup_name(character: &Character) -> Result<RosterIgn, MissingReason> {
    if character.world != DEFAULT_WORLD {
        return Err(MissingReason::OtherRegion);
    }
    RosterIgn::of(character).ok_or(MissingReason::UnsupportedIgn)
}

/// Unique by IGN (MapleScouter refuses duplicates).
fn push(subjects: &mut Vec<Subject>, subject: Subject) {
    if !subjects
        .iter()
        .any(|seen| seen.ign.key() == subject.ign.key())
    {
        subjects.push(subject);
    }
}

type Resolved = (Target, Vec<Subject>, Vec<Missing>);

/// The characters asked for, sorted (the target), each looked up.
async fn characters(roster: &dyn CharacterRoster, ids: Vec<String>) -> Result<Resolved, Refusal> {
    let mut ids: Vec<String> = ids.into_iter().map(|id| id.trim().to_owned()).collect();
    ids.sort_unstable();
    ids.dedup();
    if !(1..=MAX_BATCH).contains(&ids.len()) || ids.iter().any(String::is_empty) {
        return Err(Refusal::invalid(format!(
            "Send 1 to {MAX_BATCH} character ids."
        )));
    }
    let mut subjects = Vec::new();
    for id in &ids {
        let character = roster
            .character(id.clone())
            .await
            .map_err(unavailable)?
            .ok_or_else(not_found)?;
        let ign = lookup_name(&character).map_err(|reason| match reason {
            MissingReason::OtherRegion => Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "region",
                "MapleScouter covers MapleSEA characters only.",
            ),
            _ => Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_ign",
                "MapleScouter can't look up this IGN.",
            ),
        })?;
        push(
            &mut subjects,
            Subject {
                character_id: character.id,
                member_id: character.member_id,
                ign,
            },
        );
    }
    Ok((Target::Characters(ids), subjects, Vec::new()))
}

/// The run's party: each member's picked (else inferred) character; the
/// rest are missing.
async fn run_party(
    state: &ApiState,
    roster: &dyn CharacterRoster,
    run_id: String,
) -> Result<Resolved, Refusal> {
    let (run, slots) = party(state, roster, &run_id).await?;
    let mut subjects = Vec::new();
    let mut missing = Vec::new();
    for slot in &slots {
        let brought = slot
            .bringing()
            .and_then(|id| slot.candidates.iter().find(|row| row.id == id));
        let Some(character) = brought else {
            missing.push(Missing {
                member_id: slot.member_id.clone(),
                character_id: None,
                reason: if slot.candidates.is_empty() {
                    MissingReason::NoCharacters
                } else {
                    MissingReason::NoCharacterPicked
                },
            });
            continue;
        };
        match lookup_name(character) {
            Ok(ign) => push(
                &mut subjects,
                Subject {
                    character_id: character.id.clone(),
                    member_id: character.member_id.clone(),
                    ign,
                },
            ),
            Err(reason) => missing.push(Missing {
                member_id: slot.member_id.clone(),
                character_id: Some(character.id.clone()),
                reason,
            }),
        }
    }
    if subjects.is_empty() {
        return Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "nothing_to_look_up",
            "No one in this party has a character to look up.",
        ));
    }
    if subjects.len() > MAX_BATCH {
        return Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "too_many",
            format!("MapleScouter looks up at most {MAX_BATCH} characters at once."),
        ));
    }
    Ok((
        Target::Run {
            run_id: run.id,
            bosses: run.bosses,
        },
        subjects,
        missing,
    ))
}

/// `EstimateRequest`: `character_ids` (1 to 6) or `run_id`, and
/// `refresh` to skip the 24-hour memory ("Look up again").
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

/// `POST /api/admin/estimates` → `202 {job}` (already `done` when the
/// 24-hour memory answers it); `409 estimate_running` while a lookup runs.
async fn start(
    scouter: Scouter,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    body: Result<Json<EstimateRequest>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let estimates = scouter.estimates()?;
    let roster = roster(state)?;
    let (target, subjects, missing) = match (body.character_ids, body.run_id) {
        (Some(ids), None) => characters(roster.as_ref(), ids).await?,
        (None, Some(run_id)) => run_party(state, roster.as_ref(), run_id).await?,
        _ => return Err(Refusal::invalid("Send character_ids or run_id.")),
    };
    match estimates.start(JobRequest {
        target,
        subjects,
        missing,
        started_by: session.origin().actor,
        refresh: body.refresh,
    }) {
        Start::Started(started) => Ok((
            StatusCode::ACCEPTED,
            Json(EstimateStarted {
                job: job(&started, &state.catalog),
            }),
        )
            .into_response()),
        Start::Running(_) => Err(Refusal::new(
            StatusCode::CONFLICT,
            "estimate_running",
            "Another lookup is running.",
        )),
    }
}

/// `GET /api/admin/estimates?character_ids=<a>,<b>` or `?run_id=<id>`:
/// the newest job for that target and the running one.
async fn status(
    scouter: Scouter,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    uri: Uri,
) -> Reply {
    let state = state(&site)?;
    let estimates = scouter.estimates()?;
    let pairs = wire::query_pairs(uri.query());
    let known = pairs
        .iter()
        .all(|(key, _)| key == "character_ids" || key == "run_id");
    let target = match (
        wire::query_value(&pairs, "character_ids"),
        wire::query_value(&pairs, "run_id"),
    ) {
        (Some(ids), None) if known && pairs.len() == 1 => {
            let mut ids: Vec<String> = ids
                .split(',')
                .map(|id| id.trim().to_owned())
                .filter(|id| !id.is_empty())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            Target::Characters(ids)
        }
        (None, Some(run_id)) if known && pairs.len() == 1 => Target::Run {
            run_id,
            bosses: Vec::new(),
        },
        _ => {
            return Err(Refusal::invalid(
                "Ask with character_ids=<id>,<id> or run_id=<id>.",
            ));
        }
    };
    Ok(Json(EstimateStatus {
        job: estimates
            .latest(&target)
            .map(|found| job(&found, &state.catalog)),
        running: estimates
            .running()
            .map(|running| job(&running, &state.catalog)),
    })
    .into_response())
}

/// `GET /api/admin/estimates/{job}`.
async fn read(
    scouter: Scouter,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let found = scouter.estimates()?.job(&id).ok_or_else(|| {
        Refusal::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "That lookup is gone: results are kept for 24 hours.",
        )
    })?;
    Ok(Json(job(&found, &state.catalog)).into_response())
}
