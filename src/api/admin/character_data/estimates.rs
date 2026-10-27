//! Scouter estimates (module `character_data` with
//! `KANADE_SCOUTER_URL`): admin only, on demand, run as queued jobs looked
//! up one at a time, because a lookup takes up to a few minutes. While the
//! module is off or the URL is unset, every route answers the generic `404`
//! before the session is checked, as an unmounted path does. `POST` takes
//! the session's CSRF token. Jobs live 24 hours in memory; results are kept
//! up to 30 days in memory and in the character-data file, and one younger
//! than the saved `estimate_days` answers a lookup at once. The scouter is
//! sent roster IGNs only.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path as UrlPath, State},
    http::{StatusCode, Uri, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

use super::super::{
    characters::party,
    write::{Body, Refusal, state},
};
use crate::{
    api::{
        auth::{AdminSession, wire},
        dto::estimates::{EstimateStarted, EstimateStatus, job},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    app::read::CharacterRoster,
};
use kanade_chardata::{
    character_data::CharacterData,
    scouter::{
        BossParty, Estimates, JobRequest, MAX_BATCH, MAX_QUEUED, Missing, MissingReason, RosterIgn,
        Start, Subject, Target, party_subjects, push_subject,
    },
};
use kanade_domain::characters::Character;

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

pub(crate) fn roster(state: &ApiState) -> Result<&Arc<dyn CharacterRoster>, Refusal> {
    // The module needs the roster; without it there is nothing to look up.
    state
        .characters
        .as_ref()
        .ok_or_else(|| ApiError::NOT_FOUND.into())
}

fn not_found() -> Refusal {
    Refusal::not_found("not_found", "That character is not on the roster.")
}

/// [`RosterIgn::lookup`] for a character asked for by id: why not is a `422`.
pub(crate) fn estimate_ign(character: &Character) -> Result<RosterIgn, Refusal> {
    RosterIgn::lookup(character).map_err(|reason| match reason {
        MissingReason::OtherRegion => {
            Refusal::unprocessable("region", "Estimates cover MapleSEA characters only.")
        }
        _ => Refusal::unprocessable(
            "unsupported_ign",
            "This IGN can't be looked up for an estimate.",
        ),
    })
}

type Resolved = (Target, Vec<Subject>, Vec<Missing>, Vec<BossParty>);

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
            .map_err(Refusal::unavailable)?
            .ok_or_else(not_found)?;
        let ign = estimate_ign(&character)?;
        push_subject(
            &mut subjects,
            Subject {
                character_id: character.id,
                member_id: character.member_id,
                ign,
            },
        );
    }
    Ok((Target::Characters(ids), subjects, Vec::new(), Vec::new()))
}

/// The run's party: the character each member brings to each boss (as
/// `…/characters` resolves it), and each boss's party; the rest are
/// missing.
async fn run_party(
    state: &ApiState,
    roster: &dyn CharacterRoster,
    run_id: String,
) -> Result<Resolved, Refusal> {
    let (run, slots) = party(state, roster, &run_id).await?;
    let (subjects, missing, parties) = party_subjects(&slots, &run.bosses);
    if subjects.is_empty() {
        return Err(Refusal::unprocessable(
            "nothing_to_look_up",
            "No one in this party has a character to look up.",
        ));
    }
    if parties.iter().any(|party| party.subjects.len() > MAX_BATCH) {
        return Err(Refusal::unprocessable(
            "too_many",
            format!("An estimate looks up at most {MAX_BATCH} characters per boss."),
        ));
    }
    Ok((
        Target::Run {
            run_id: run.id,
            bosses: run.bosses,
        },
        subjects,
        missing,
        parties,
    ))
}

/// `EstimateRequest`: `character_ids` (1 to 6) or `run_id`, and
/// `refresh` to skip the memory ("Look up again").
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

/// `POST /api/admin/estimates` → `202 {job}`: queued (or running at once),
/// already `done` when memory holds a result younger than the saved
/// `estimate_days` for each, or the target's job (also a waiting background
/// refresh, which moves into the queue) already queued or running;
/// `429 estimate_queue_full` past [`MAX_QUEUED`] waiting jobs.
async fn start(
    scouter: Scouter,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    Body(body): Body<EstimateRequest>,
) -> Reply {
    let state = state(&site)?;
    let estimates = scouter.estimates()?;
    let roster = roster(state)?;
    let (target, subjects, missing, parties) = match (body.character_ids, body.run_id) {
        (Some(ids), None) => characters(roster.as_ref(), ids).await?,
        (None, Some(run_id)) => run_party(state, roster.as_ref(), run_id).await?,
        _ => return Err(Refusal::invalid("Send character_ids or run_id.")),
    };
    match estimates.start(JobRequest {
        target,
        subjects,
        missing,
        parties,
        started_by: session.origin().actor,
        refresh: body.refresh,
    }) {
        Start::Started(started) | Start::Joined(started) => Ok((
            StatusCode::ACCEPTED,
            Json(EstimateStarted {
                job: job(&started, &state.catalog, estimates),
            }),
        )
            .into_response()),
        Start::Full => Err(Refusal::too_many(
            "estimate_queue_full",
            format!("{MAX_QUEUED} lookups are waiting already. Try again in a few minutes."),
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
            .map(|found| job(&found, &state.catalog, estimates)),
        running: estimates
            .running()
            .map(|running| job(&running, &state.catalog, estimates)),
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
    let estimates = scouter.estimates()?;
    let found = estimates.job(&id).ok_or_else(|| {
        Refusal::not_found(
            "not_found",
            "That lookup is gone: lookups are kept for 24 hours.",
        )
    })?;
    Ok(Json(job(&found, &state.catalog, estimates)).into_response())
}
