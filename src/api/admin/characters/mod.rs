//! The manual character roster (module `roster`): one member's characters
//! and their create, edit (retire included) and delete, the character each
//! party member brings to a run (`runs.rs`) and, with module
//! `character_data` on too, to a weekly timing (`fixed.rs`). While a
//! route's module is off (`modules.roster`, read at startup) it answers the
//! generic `404`, before the session is even checked, as an unmounted path
//! does. Writes take the session's CSRF token, record History rows
//! (section `characters`, key `character.<id>`, `pick.<run>.<member>` or
//! `fixedpick.<fixed>.<member>`) and replay an `Idempotency-Key` (scope
//! `config`) in the same transaction; roster data never reaches a model.

mod fixed;
mod picks;
mod runs;
mod write;

pub(super) use fixed::{Both, Timing};
pub(crate) use runs::party;
pub(crate) use write::{CharacterCreate, CharacterPatch, patch_request, write_row};

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path as UrlPath, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, patch, put},
};

use super::write::{Refusal, state};
use crate::api::{
    auth::AdminSession,
    dto::characters::{MemberCharacters, row},
    error::ApiError,
    listeners::Site,
    state::{ApiState, CharacterRoster},
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route(
            "/api/admin/members/{id}/characters",
            get(list).post(write::create),
        )
        .route(
            "/api/admin/characters/{id}",
            patch(write::update).delete(write::delete),
        )
        .route("/api/admin/runs/{id}/characters", get(runs::read))
        .route("/api/admin/runs/{id}/characters/{member}", put(runs::pick))
        .route("/api/admin/fixed/{id}/characters", get(fixed::read))
        .route(
            "/api/admin/fixed/{id}/characters/{member}",
            put(fixed::pick),
        )
}

type Reply = Result<Response, Refusal>;

/// The module's store; the generic `404` while the module is off. Taken
/// first, so an off module answers as an unmounted path whoever asks.
struct Roster(Arc<dyn CharacterRoster>);

impl FromRequestParts<Arc<Site>> for Roster {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        site.state
            .as_ref()
            .and_then(|state| state.characters.clone())
            .map(Self)
            .ok_or(ApiError::NOT_FOUND)
    }
}

fn member_not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "That member is not on the roster.",
    )
}

/// Characters belong to members the bot has seen (departed ones included).
async fn require_member(state: &ApiState, member_id: &str) -> Result<(), Refusal> {
    match state.store.member(member_id.to_owned()).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(member_not_found()),
        Err(_) => Err(ApiError::UNAVAILABLE.into()),
    }
}

async fn list(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(member_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    require_member(state, &member_id).await?;
    let characters = roster
        .member_characters(member_id.clone())
        .await
        .map_err(Refusal::unavailable)?;
    Ok(Json(MemberCharacters {
        member_id,
        characters: characters.iter().map(row).collect(),
    })
    .into_response())
}
