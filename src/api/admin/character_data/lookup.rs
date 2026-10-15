//! The Add character dialog's lookup (module `character_data`): one typed
//! MapleSEA IGN asked of NEXON (`id` + `basic`, never `stat`) and answered
//! once. Nothing is stored and no History row is written: the values are
//! NEXON data, kept only after the member links the character. `POST`
//! takes the session's CSRF token; the shared 24-hour budget, the holds and
//! a per-admin spacing limit it.

use std::sync::Arc;

use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

use super::super::write::{Body, Refusal};
use super::{Module, not_connected};
use crate::{
    api::{
        auth::AdminSession,
        dto::character_data::{LookupStatus, lookup},
        listeners::Site,
    },
    domain::{
        characters::{DEFAULT_WORLD, parse_ign, parse_world},
        chardata::LINK_REGION,
    },
    infrastructure::character_data::LookupOutcome,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/characters/lookup", get(status).post(look_up))
}

type Reply = Result<Response, Refusal>;

/// `GET`: whether lookups (and the link a MapleSEA create asks for) can be made.
async fn status(Module(module): Module, _: AdminSession) -> Reply {
    Ok(Json(LookupStatus {
        connected: module.connected(),
    })
    .into_response())
}

/// `CharacterLookupRequest`: the IGN as typed and its region (`msea` when
/// absent; only MapleSEA can be asked).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupRequest {
    ign: String,
    #[serde(default)]
    world: Option<String>,
}

/// `POST`: `200 CharacterLookup`, `404 not_found` when NEXON knows no such
/// name, `429 lookup_cooldown`, `503 nexon_unavailable` while calls are held.
async fn look_up(
    Module(module): Module,
    session: AdminSession,
    Body(body): Body<LookupRequest>,
) -> Reply {
    let ign = parse_ign(&body.ign).map_err(|error| Refusal::invalid(error.to_string()))?;
    let world = parse_world(body.world.as_deref().unwrap_or(DEFAULT_WORLD))
        .map_err(|error| Refusal::invalid(error.to_string()))?;
    if world != LINK_REGION {
        return Err(Refusal::unprocessable(
            "region",
            "Only MapleSEA characters can be looked up.",
        ));
    }
    answer(
        module
            .look_up(&session.origin().actor, &ign)
            .await
            .map_err(Refusal::unavailable)?,
    )
}

/// A lookup's answer, the same on both origins.
pub(crate) fn answer(outcome: LookupOutcome) -> Reply {
    match outcome {
        LookupOutcome::Found(found) => Ok(Json(lookup(found)).into_response()),
        LookupOutcome::NotFound => Err(Refusal::not_found(
            "not_found",
            "NEXON has no MapleSEA character by that name.",
        )),
        LookupOutcome::NotConnected => Err(not_connected()),
        LookupOutcome::TooSoon => Err(Refusal::too_many(
            "lookup_cooldown",
            "Wait a few seconds before looking up again.",
        )),
        LookupOutcome::Unavailable(reason) => Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "nexon_unavailable",
            format!("The Open API can't be called right now ({reason})."),
        )),
    }
}
