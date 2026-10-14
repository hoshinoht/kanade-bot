//! Readiness (module `character_data`): `GET /api/admin/runs/{id}/readiness`
//! shows each party slot of a run (the slots of `GET …/characters`) with its
//! level and power, and judges its level against the boss knowledge
//! `entry_level` of each boss of the run. A weekly timing's readiness (with
//! module `roster` on too) reads the same way: `GET
//! /api/admin/fixed/{id}/readiness` for the saved timing, and `POST
//! /api/admin/fixed/readiness` for the Fixed editor's form as it stands (a
//! timing not added yet, or an unsaved boss or party change; picks count
//! only for members of the saved party). With estimates on, each boss also
//! carries the character's scouter reading when the estimates' 24-hour
//! memory holds one; readiness never starts a lookup. Computed on read;
//! nothing is stored and nothing is blocked. While a module is off the
//! routes answer the generic `404` before the session is checked.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;

use super::{
    super::{
        characters::{Both, Timing, party},
        write::{Refusal, bad_body, state},
    },
    Module, Reply,
    estimates::lookup_name,
};
use crate::{
    api::{
        auth::AdminSession,
        dto::{
            self, Art, bosses::entry_levels, character_data::link_state, readiness::ReadinessSlot,
        },
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        characters::PartySlot,
        chardata::{Requirement, Subject, readiness},
    },
    infrastructure::character_data::CharacterData,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/runs/{id}/readiness", get(read))
        .route("/api/admin/fixed/{id}/readiness", get(read_timing))
        .route("/api/admin/fixed/readiness", post(preview))
}

/// What each boss asks, in order: tokens as the boss chips project them,
/// entry levels read from the knowledge files.
async fn requirements(state: &ApiState, tokens: Vec<String>) -> Result<Vec<Requirement>, Refusal> {
    let catalog = Arc::clone(&state.catalog);
    let dir = state.knowledge_dir.clone();
    tokio::task::spawn_blocking(move || {
        let mut levels: BTreeMap<String, BTreeMap<String, u16>> = BTreeMap::new();
        tokens
            .iter()
            .filter_map(|token| dto::boss(&catalog, &Art { root: None }, token))
            .map(|boss| {
                let entry_level = dir.as_deref().and_then(|dir| {
                    levels
                        .entry(boss.key.clone())
                        .or_insert_with(|| entry_levels(dir, &catalog, &boss.key))
                        .get(&boss.difficulty)
                        .copied()
                });
                Requirement {
                    entry_level,
                    boss: boss.key,
                    difficulty: boss.difficulty,
                }
            })
            .collect()
    })
    .await
    .map_err(Refusal::unavailable)
}

/// Each slot's readiness against `bosses`, in order, and whether any shows
/// a NEXON value.
async fn answers(
    state: &ApiState,
    module: &CharacterData,
    slots: &[PartySlot],
    bosses: &[Requirement],
) -> Result<(Vec<ReadinessSlot>, bool), Refusal> {
    let mut answers = Vec::with_capacity(slots.len());
    let mut nexon_data = false;
    for slot in slots {
        let brings = slot.brought();
        let (link, ready, scouter) = match brings {
            Some((character, _)) => {
                let view = module
                    .status(&character.id)
                    .await
                    .map_err(Refusal::unavailable)?;
                let subject = Subject {
                    manual_level: character.level,
                    link: view.link.as_ref().map(|link| link.status),
                    snapshot: view.snapshot.as_ref(),
                };
                let scouter = module
                    .estimates()
                    .zip(lookup_name(character).ok())
                    .and_then(|(estimates, ign)| estimates.cached(&ign));
                (
                    Some(link_state(&view)),
                    readiness(&subject, bosses, view.now),
                    scouter,
                )
            }
            None => (
                None,
                readiness(&Subject::default(), bosses, state.now()),
                None,
            ),
        };
        nexon_data |= ready.nexon;
        answers.push(dto::readiness::slot(
            &slot.member_id,
            slot.candidates.len(),
            brings,
            link,
            &ready,
            bosses,
            scouter.as_ref(),
        ));
    }
    Ok((answers, nexon_data))
}

/// `GET /api/admin/runs/{id}/readiness`.
async fn read(
    Module(module): Module,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(run_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    // The module needs the roster; without it there is no party.
    let roster = state
        .characters
        .as_ref()
        .ok_or_else(|| Refusal::from(ApiError::NOT_FOUND))?;
    let (run, slots) = party(state, roster.as_ref(), &run_id).await?;
    let bosses = requirements(state, run.bosses.clone()).await?;
    let (slots, nexon_data) = answers(state, &module, &slots, &bosses).await?;
    Ok(Json(dto::readiness::run(&run.id, slots, nexon_data)).into_response())
}

/// `GET /api/admin/fixed/{id}/readiness`: the saved timing's party and
/// bosses.
async fn read_timing(
    Both(roster): Both,
    Module(module): Module,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(fixed_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let timing = Timing::load(state, roster.as_ref(), Some(&fixed_id), None).await?;
    let fixed = timing
        .fixed
        .as_ref()
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    let slots = timing.slots(&fixed.participants);
    let bosses = requirements(state, fixed.bosses.clone()).await?;
    let (slots, nexon_data) = answers(state, &module, &slots, &bosses).await?;
    Ok(Json(dto::readiness::timing(Some(&fixed.id), slots, nexon_data)).into_response())
}

/// The most bosses or members a preview reads (a form never holds more).
const PREVIEW_MAX: usize = 64;

/// `TimingPreview`: the editor's bosses (tokens) and party (member ids) as
/// they stand, and the timing they edit (absent for a new one).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingPreview {
    #[serde(default)]
    fixed_run_id: Option<String>,
    bosses: Vec<String>,
    participants: Vec<String>,
}

/// `POST /api/admin/fixed/readiness`: readiness of the form as it stands.
/// Nothing is written; unknown boss tokens are left out.
async fn preview(
    Both(roster): Both,
    Module(module): Module,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    body: Result<Json<TimingPreview>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    if body.bosses.len() > PREVIEW_MAX || body.participants.len() > PREVIEW_MAX {
        return Err(Refusal::invalid(format!(
            "A preview reads at most {PREVIEW_MAX} bosses and {PREVIEW_MAX} members."
        )));
    }
    let state = state(&site)?;
    let timing = Timing::load(
        state,
        roster.as_ref(),
        body.fixed_run_id.as_deref(),
        Some(&body.participants),
    )
    .await?;
    let slots = timing.slots(&body.participants);
    // A saved token the catalog dropped still shows by its text; a form's
    // unknown token is no boss at all.
    let tokens = body
        .bosses
        .into_iter()
        .filter(|token| state.catalog.split(token).is_some())
        .collect();
    let bosses = requirements(state, tokens).await?;
    let (slots, nexon_data) = answers(state, &module, &slots, &bosses).await?;
    Ok(Json(dto::readiness::timing(
        body.fixed_run_id.as_deref(),
        slots,
        nexon_data,
    ))
    .into_response())
}
