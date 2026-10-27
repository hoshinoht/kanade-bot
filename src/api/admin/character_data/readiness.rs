//! Readiness (module `character_data`): `GET /api/admin/runs/{id}/readiness`
//! shows each party slot of a run (the slots of `GET …/characters`) with its
//! level and power, and judges the level of the character brought to each
//! boss of the run (per-boss picks) against that boss's knowledge
//! `entry_level`. A weekly timing's readiness (with module `roster` on too)
//! reads the same way: `GET /api/admin/fixed/{id}/readiness` for the saved
//! timing, and `POST /api/admin/fixed/readiness` for the Fixed editor's form
//! as it stands (a timing not added yet, or an unsaved boss or party
//! change; picks count only for members of the saved party). With
//! estimates on, each boss also carries its character's stored scouter
//! reading, of any age, marked `stale` once at least `estimate_days` old;
//! readiness never starts a lookup. Computed on read; nothing is stored and
//! nothing is blocked. While a module is off the routes answer the generic
//! `404` before the session is checked.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;

use super::{
    super::{
        characters::{Both, Timing, party},
        write::{Body, Refusal, state},
    },
    Module, Reply,
};
use crate::api::{
    auth::AdminSession,
    dto::{
        self, Art,
        bosses::entry_levels,
        character_data::link_state,
        readiness::{Cached, Judged, ReadinessSlot},
    },
    error::ApiError,
    listeners::Site,
    state::ApiState,
};
use kanade_chardata::{
    character_data::CharacterData,
    scouter::{Estimates, RosterIgn},
};
use kanade_domain::{
    catalog::BossTable,
    characters::{Character, PartySlot},
    chardata::{Requirement, Subject, readiness},
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

/// `character`'s values against `bosses`: its link, readiness and stored
/// scouter result (`None`: nothing brought).
async fn judge<'c>(
    state: &ApiState,
    module: &CharacterData,
    character: Option<&'c Character>,
    bosses: &[Requirement],
) -> Result<Judged<'c>, Refusal> {
    let Some(character) = character else {
        return Ok(Judged {
            character: None,
            link: None,
            readiness: readiness(&Subject::default(), bosses, state.now()),
            scouter: None,
        });
    };
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
        .zip(RosterIgn::lookup(character).ok())
        .and_then(|(estimates, ign)| Cached::of(estimates, &ign));
    Ok(Judged {
        character: Some(character),
        link: Some(link_state(&view)),
        readiness: readiness(&subject, bosses, view.now),
        scouter,
    })
}

/// The character brought to each of `bosses`: the slot's bosses whose
/// token the catalog knows (as [`requirements`] keeps them), in order.
fn brought_to<'s>(
    catalog: &BossTable,
    slot: &'s PartySlot,
    bosses: &[Requirement],
) -> Vec<Option<&'s Character>> {
    let known: Vec<usize> = (0..slot.bosses.len())
        .filter(|at| catalog.split(&slot.bosses[*at].token).is_some())
        .collect();
    (0..bosses.len())
        .map(|at| match known.get(at) {
            Some(boss) => slot.brought_to(*boss).map(|(character, _)| character),
            None => slot.brought().map(|(character, _)| character),
        })
        .collect()
}

/// Each slot's readiness against `bosses`, in order, whether any shows
/// a NEXON value, and the saved `estimate_days` while estimates are on.
async fn answers(
    state: &ApiState,
    module: &CharacterData,
    slots: &[PartySlot],
    bosses: &[Requirement],
) -> Result<(Vec<ReadinessSlot>, bool, Option<u32>), Refusal> {
    let mut answers = Vec::with_capacity(slots.len());
    let mut nexon_data = false;
    for slot in slots {
        let brings = slot.brought();
        let first = judge(
            state,
            module,
            brings.map(|(character, _)| character),
            bosses,
        )
        .await?;
        // Each other character brought, judged once.
        let mut others: Vec<Judged<'_>> = Vec::new();
        let per_boss = brought_to(&state.catalog, slot, bosses);
        for character in per_boss.iter().flatten() {
            let seen = first.character.map(|row| row.id.as_str()) == Some(character.id.as_str())
                || others
                    .iter()
                    .any(|judged| judged.character.is_some_and(|row| row.id == character.id));
            if !seen {
                others.push(judge(state, module, Some(character), bosses).await?);
            }
        }
        let judged: Vec<&Judged<'_>> = per_boss
            .iter()
            .map(|character| {
                character
                    .and_then(|character| {
                        others.iter().find(|judged| {
                            judged.character.is_some_and(|row| row.id == character.id)
                        })
                    })
                    .unwrap_or(&first)
            })
            .collect();
        nexon_data |= first.readiness.nexon || others.iter().any(|judged| judged.readiness.nexon);
        let others: Vec<&Judged<'_>> = others.iter().collect();
        answers.push(dto::readiness::slot(
            &slot.member_id,
            slot.candidates.len(),
            brings,
            &first,
            &judged,
            bosses,
            &others,
        ));
    }
    Ok((
        answers,
        nexon_data,
        module.estimates().map(Estimates::fresh_days),
    ))
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
    let (slots, nexon_data, fresh_days) = answers(state, &module, &slots, &bosses).await?;
    Ok(Json(dto::readiness::run(&run.id, slots, nexon_data, fresh_days)).into_response())
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
    let slots = timing.slots(&state.catalog, &fixed.participants, &fixed.bosses);
    let bosses = requirements(state, fixed.bosses.clone()).await?;
    let (slots, nexon_data, fresh_days) = answers(state, &module, &slots, &bosses).await?;
    Ok(Json(dto::readiness::timing(
        Some(&fixed.id),
        slots,
        nexon_data,
        fresh_days,
    ))
    .into_response())
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
    Body(body): Body<TimingPreview>,
) -> Reply {
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
    // A saved token the catalog dropped still shows by its text; a form's
    // unknown token is no boss at all. Per-boss picks read against the
    // form's bosses, as they will once it is saved.
    let tokens: Vec<String> = body
        .bosses
        .into_iter()
        .filter(|token| state.catalog.split(token).is_some())
        .collect();
    let slots = timing.slots(&state.catalog, &body.participants, &tokens);
    let bosses = requirements(state, tokens).await?;
    let (slots, nexon_data, fresh_days) = answers(state, &module, &slots, &bosses).await?;
    Ok(Json(dto::readiness::timing(
        body.fixed_run_id.as_deref(),
        slots,
        nexon_data,
        fresh_days,
    ))
    .into_response())
}
