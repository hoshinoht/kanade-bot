//! Character data (module `character_data`): one roster character's link
//! and Nexon data, the Add character lookup (`lookup.rs`, never stored) and
//! a run's readiness (`readiness.rs`). While the module
//! is off (`modules.character_data`, read at startup) every route answers
//! the generic `404` before the session is checked, as an unmounted path
//! does. Writes take the session's CSRF token, replay an `Idempotency-Key`
//! (scope `config`) and record History rows (section `characters`, key
//! `link.<id>`, status words only). An admin request is `pending` and calls
//! nothing until the member confirms in Discord; the refresh shares the
//! batch's 24-hour cap and daily budget.

pub(crate) mod estimates;
mod lookup;
mod readiness;

pub(crate) use lookup::answer as lookup_answer;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    replay::recall,
    write::{Refusal, bad_body, state},
};
use crate::{
    api::{
        auth::AdminSession, dto::character_data::link, error::ApiError, listeners::Site,
        state::ApiState,
    },
    domain::chardata::UnlinkReason,
    infrastructure::character_data::{CharacterData, LinkView, RefreshOutcome, RequestOutcome},
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route(
            "/api/admin/characters/{id}/link",
            get(read).post(request).delete(unlink),
        )
        .route("/api/admin/characters/{id}/refresh", post(refresh))
        .merge(estimates::routes())
        .merge(lookup::routes())
        .merge(readiness::routes())
}

type Reply = Result<Response, Refusal>;

/// The module's service; the generic `404` while the module is off. Taken
/// first, so an off module answers as an unmounted path whoever asks.
struct Module(Arc<CharacterData>);

impl FromRequestParts<Arc<Site>> for Module {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        site.state
            .as_ref()
            .and_then(|state| state.character_data.clone())
            .map(Self)
            .ok_or(ApiError::NOT_FOUND)
    }
}

fn not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "That character is not on the roster.",
    )
}

pub(crate) fn not_connected() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "not_connected",
        "Character data isn't connected yet.",
    )
}

fn not_linked() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "not_linked",
        "That character is not linked.",
    )
}

/// The roster character must exist (its link may not).
async fn require_character(state: &ApiState, id: &str) -> Result<(), Refusal> {
    let Some(roster) = &state.characters else {
        return Err(not_found());
    };
    match roster.character(id.to_owned()).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(not_found()),
        Err(error) => Err(Refusal::unavailable(error)),
    }
}

async fn view(module: &CharacterData, id: &str) -> Result<Value, Refusal> {
    let view = module.view(id).await.map_err(Refusal::unavailable)?;
    Ok(json!(link(id, &view, module.privacy_url())))
}

/// `GET`: status, fresh data and the attribution.
async fn read(
    Module(module): Module,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    require_character(state, &id).await?;
    Ok(Json(view(&module, &id).await?).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    /// The link version as loaded (0 while there is none).
    version: Option<u32>,
}

/// `POST`: ask the member to link (pending; no Nexon call).
async fn request(
    Module(module): Module,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Result<Json<LinkRequest>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let version = body.version.ok_or_else(|| {
        Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "version_required",
            "Send the link version as loaded (0 when not linked).",
        )
    })?;
    let request = format!("character.link\u{1f}{id}\u{1f}{version}");
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let origin = session.origin();
    let now = state.now();
    let mut answer = None;
    let outcome = module
        .request_link(&id, version, &origin.actor, origin.surface, |new| {
            let body = json!(link(
                &id,
                &LinkView {
                    connected: true,
                    link: Some(new.clone()),
                    snapshot: None,
                    refresh: None,
                    now,
                },
                module.privacy_url(),
            ));
            let replay = keyed
                .as_ref()
                .map(|keyed| keyed.answered(StatusCode::CREATED, &body, now));
            answer = Some(body);
            replay
        })
        .await
        .map_err(Refusal::unavailable)?;
    match outcome {
        RequestOutcome::Requested(_) => {
            Ok((StatusCode::CREATED, Json(answer.unwrap_or(Value::Null))).into_response())
        }
        RequestOutcome::NotConnected => Err(not_connected()),
        RequestOutcome::Missing => Err(not_found()),
        RequestOutcome::Retired => Err(Refusal::new(
            StatusCode::CONFLICT,
            "retired",
            "A retired character can't be linked.",
        )),
        RequestOutcome::Region => Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "region",
            "Only MapleSEA characters can be linked.",
        )),
        RequestOutcome::AlreadyLinked(_) => Err(Refusal::new(
            StatusCode::CONFLICT,
            "already_linked",
            "That character is already linked or waiting for its member.",
        )),
        RequestOutcome::Stale(_) => Err(Refusal::new(
            StatusCode::CONFLICT,
            "stale",
            "The link changed since it was loaded.",
        )),
    }
}

/// `DELETE`: unlink now; the data is deleted in the same write.
async fn unlink(
    Module(module): Module,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Reply {
    let state = state(&site)?;
    let request = format!("character.unlink\u{1f}{id}");
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let body = json!({ "message": "Unlinked: its character data is deleted." });
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::OK, &body, state.now()));
    let origin = session.origin();
    match module
        .unlink(
            &id,
            &origin.actor,
            origin.surface,
            UnlinkReason::Requested,
            replay,
        )
        .await
        .map_err(Refusal::unavailable)?
    {
        Some(_) => Ok(Json(body).into_response()),
        None => Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "not_linked",
            "That character is not linked.",
        )),
    }
}

/// `POST …/refresh`: fetch now, from the daily budget (no per-character cap).
async fn refresh(
    Module(module): Module,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Reply {
    let state = state(&site)?;
    let request = format!("character.refresh\u{1f}{id}");
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    require_character(state, &id).await?;
    match module
        .refresh_now(&id)
        .await
        .map_err(Refusal::unavailable)?
    {
        RefreshOutcome::Done(_) => {}
        RefreshOutcome::NotConnected => return Err(not_connected()),
        RefreshOutcome::NotLinked => return Err(not_linked()),
        RefreshOutcome::Unavailable(reason) => {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "nexon_unavailable",
                format!("The Open API can't be called right now ({reason})."),
            ));
        }
    }
    let body = view(&module, &id).await?;
    if let Some(keyed) = keyed {
        state
            .store
            .put_replay(keyed.answered(StatusCode::OK, &body, state.now()))
            .await
            .map_err(Refusal::unavailable)?;
    }
    Ok(Json(body).into_response())
}
