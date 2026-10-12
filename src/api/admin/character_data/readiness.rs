//! Run readiness (module `character_data`): `GET /api/admin/runs/{id}/readiness`
//! shows each party slot (the slots of `GET …/characters`, inferred ones
//! recomputed) with its level and power, and judges its level against the
//! boss knowledge `entry_level` of each boss of the run. Computed on read;
//! nothing is stored and nothing is blocked. While the module is off the
//! route answers the generic `404` before the session is checked.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    response::IntoResponse,
    routing::get,
};

use super::{
    super::{
        characters::party,
        write::{Refusal, state},
    },
    Module, Reply, unavailable,
};
use crate::{
    api::{
        auth::AdminSession,
        dto::{self, Art, bosses::entry_levels, character_data::link_state},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::chardata::{Requirement, Subject, readiness},
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/runs/{id}/readiness", get(read))
}

/// What each boss of the run asks, in run order: tokens as the run's boss
/// chips project them, entry levels read from the knowledge files.
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
    .map_err(unavailable)
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
    let mut answers = Vec::with_capacity(slots.len());
    let mut nexon_data = false;
    for slot in &slots {
        let brings = slot
            .bringing()
            .and_then(|id| slot.candidates.iter().find(|character| character.id == id))
            .map(|character| (character, slot.picked.is_none()));
        let (link, ready) = match brings {
            Some((character, _)) => {
                let view = module.status(&character.id).await.map_err(unavailable)?;
                let subject = Subject {
                    manual_level: character.level,
                    link: view.link.as_ref().map(|link| link.status),
                    snapshot: view.snapshot.as_ref(),
                };
                (
                    Some(link_state(&view)),
                    readiness(&subject, &bosses, view.now),
                )
            }
            None => (None, readiness(&Subject::default(), &bosses, state.now())),
        };
        nexon_data |= ready.nexon;
        answers.push(dto::readiness::slot(
            &slot.member_id,
            brings,
            link,
            &ready,
            &bosses,
        ));
    }
    Ok(Json(dto::readiness::run(&run.id, answers, nexon_data)).into_response())
}
