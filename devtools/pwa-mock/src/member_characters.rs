//! The member's own characters on the public origin (`mock/member_characters.rs`,
//! as `src/api/public/characters/`): each route answers `closed` while the
//! portal is, then the generic 404 while its module is off (`roster`;
//! `character_data` for the NEXON routes; the scouter for the estimates),
//! both before the session. Add, Edit and Link need the session's token, an
//! `Idempotency-Key` and the fresh window; Unlink and Unlink all the token
//! and key; the lookup preview, Cancel and a lookup start the token only.
//! `POST /__mock/public/characters {roster}` redraws Asahi's characters.

use crate::{
    App,
    api::{Answer, Refusal, closed, error, not_found, outcome_with},
    mock::{
        MoveError, Store,
        member_characters::{
            EstimateRefused, LinkConfirm, MyCreate, MyEstimate, MyLookup, MyPatch, RosterRequest,
        },
        portal::{Current, MEMBER_SEED_ID},
    },
    public::{admit, carry, member, member_body},
};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;

/// `closed`, then the generic 404 while the module (`on`) is off, before the session.
async fn gate(app: &App, on: fn(&Store) -> bool) -> Result<(), Refusal> {
    let store = app.store.lock().await;
    if !store.public_portal() {
        return Err(closed().into());
    }
    let on = on(&store);
    drop(store);
    if on {
        Ok(())
    } else {
        Err(not_found().await.into())
    }
}

async fn session(
    app: &App,
    on: fn(&Store) -> bool,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Current, Refusal> {
    gate(app, on).await?;
    member(app, method, headers).await
}

async fn admitted(
    app: &App,
    on: fn(&Store) -> bool,
    method: &Method,
    headers: &HeaderMap,
    fresh: bool,
) -> Result<(Current, String), Refusal> {
    gate(app, on).await?;
    admit(app, method, headers, fresh).await
}

/// `result` with `status` on success.
fn answer<T: serde::Serialize>(
    current: &Current,
    status: StatusCode,
    result: Result<T, MoveError>,
) -> Answer {
    Ok(carry(outcome_with(status, result), current))
}

/// `GET /api/public/me/characters`: `MyCharacters`.
pub async fn list(State(app): State<App>, method: Method, headers: HeaderMap) -> Answer {
    let current = session(&app, Store::roster_on, &method, &headers).await?;
    let body = app.store.lock().await.my_characters(MEMBER_SEED_ID);
    Ok(carry(Json(body).into_response(), &current))
}

/// `POST /api/public/me/characters`: `201 MyCharacterCreated` (a retry
/// with the same key `201` with the first answer).
pub async fn create(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    body: Result<Json<MyCreate>, JsonRejection>,
) -> Answer {
    let (current, key) = admitted(&app, Store::roster_on, &method, &headers, true).await?;
    let body = member_body(body, &current)?;
    let result = app.store.lock().await.my_create(MEMBER_SEED_ID, &key, body);
    answer(&current, StatusCode::CREATED, result.map(|(_, row)| row))
}

/// `PATCH /api/public/me/characters/{id}`: the row.
pub async fn update(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<MyPatch>, JsonRejection>,
) -> Answer {
    let (current, key) = admitted(&app, Store::roster_on, &method, &headers, true).await?;
    let body = member_body(body, &current)?;
    let result = app
        .store
        .lock()
        .await
        .my_patch(MEMBER_SEED_ID, &key, &id, body);
    answer(&current, StatusCode::OK, result)
}

/// `POST /api/public/me/characters/lookup`: `CharacterLookup`, never kept.
pub async fn look_up(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    body: Result<Json<MyLookup>, JsonRejection>,
) -> Answer {
    let current = session(&app, Store::character_data_on, &method, &headers).await?;
    let body = member_body(body, &current)?;
    let result = app.store.lock().await.my_look_up(body);
    answer(&current, StatusCode::OK, result)
}

/// `GET /api/public/me/characters/{id}/link`: `CharacterLink`.
pub async fn link(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer {
    let current = session(&app, Store::character_data_on, &method, &headers).await?;
    let result = app.store.lock().await.my_link(MEMBER_SEED_ID, &id);
    answer(&current, StatusCode::OK, result)
}

/// `GET /api/public/me/characters/{id}/link/notice`: `LinkNotice`.
pub async fn notice(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer {
    let current = session(&app, Store::character_data_on, &method, &headers).await?;
    let result = app.store.lock().await.my_notice(MEMBER_SEED_ID, &id);
    answer(&current, StatusCode::OK, result)
}

/// `POST /api/public/me/characters/{id}/link`: `LinkOutcome`.
pub async fn confirm(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<LinkConfirm>, JsonRejection>,
) -> Answer {
    let on = Store::character_data_on;
    let (current, _) = admitted(&app, on, &method, &headers, true).await?;
    let body = member_body(body, &current)?;
    let result = app.store.lock().await.my_confirm(MEMBER_SEED_ID, &id, body);
    answer(&current, StatusCode::OK, result)
}

/// `POST /api/public/me/characters/{id}/link/cancel`: `LinkCancelled`.
pub async fn cancel(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer {
    let current = session(&app, Store::character_data_on, &method, &headers).await?;
    let result = app.store.lock().await.my_cancel(MEMBER_SEED_ID, &id);
    answer(&current, StatusCode::OK, result)
}

/// `DELETE /api/public/me/characters/{id}/link`: `Unlinked` (no fresh sign-in).
pub async fn unlink(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer {
    let on = Store::character_data_on;
    let (current, key) = admitted(&app, on, &method, &headers, false).await?;
    let result = app.store.lock().await.my_unlink(MEMBER_SEED_ID, &key, &id);
    answer(&current, StatusCode::OK, result)
}

/// `DELETE /api/public/me/characters/links`: `UnlinkedAll`.
pub async fn unlink_all(State(app): State<App>, method: Method, headers: HeaderMap) -> Answer {
    let on = Store::character_data_on;
    let (current, key) = admitted(&app, on, &method, &headers, false).await?;
    let result = app.store.lock().await.my_unlink_all(MEMBER_SEED_ID, &key);
    answer(&current, StatusCode::OK, result)
}

/// `GET /api/public/me/data`: `MyData`.
pub async fn data(State(app): State<App>, method: Method, headers: HeaderMap) -> Answer {
    let current = session(&app, Store::character_data_on, &method, &headers).await?;
    let body = app.store.lock().await.my_data(MEMBER_SEED_ID);
    Ok(carry(Json(body).into_response(), &current))
}

/// `POST /api/public/me/estimates`: `202 {job}`, or `429 member_limited`
/// with `next_at`.
pub async fn start_estimate(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    body: Result<Json<MyEstimate>, JsonRejection>,
) -> Answer {
    let current = session(&app, Store::estimates_on, &method, &headers).await?;
    let body = member_body(body, &current)?;
    let result = app
        .store
        .lock()
        .await
        .my_estimate_start(MEMBER_SEED_ID, body);
    match result {
        Ok(started) => answer(&current, StatusCode::ACCEPTED, Ok(started)),
        Err(EstimateRefused::Coded(refused)) => {
            answer::<()>(&current, StatusCode::OK, Err(refused))
        }
        Err(EstimateRefused::Limited { next_at }) => {
            let body = json!({
                "error": "member_limited",
                "message": "You can start one lookup every 30 minutes.",
                "next_at": next_at,
            });
            let limited = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
            Ok(carry(limited, &current))
        }
    }
}

#[derive(Deserialize)]
pub struct EstimateQuery {
    character_id: Option<String>,
}

/// `GET /api/public/me/estimates?character_id=`: `EstimateStatus`.
pub async fn estimate_status(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Query(q): Query<EstimateQuery>,
) -> Answer {
    let current = session(&app, Store::estimates_on, &method, &headers).await?;
    let Some(id) = q.character_id else {
        let refused = error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "Ask with character_id=<id>.",
        );
        return Ok(carry(refused, &current));
    };
    let result = app
        .store
        .lock()
        .await
        .my_estimate_status(MEMBER_SEED_ID, &id);
    answer(&current, StatusCode::OK, result)
}

/// `GET /api/public/me/estimates/{job}`: `EstimateJob`.
pub async fn estimate(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(job): Path<String>,
) -> Answer {
    let current = session(&app, Store::estimates_on, &method, &headers).await?;
    let result = app.store.lock().await.my_estimate(MEMBER_SEED_ID, &job);
    answer(&current, StatusCode::OK, result)
}

/// `GET /api/public/runs/{id}/characters`: `RunPartyCharacters`.
pub async fn party(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer {
    let current = session(&app, Store::roster_on, &method, &headers).await?;
    let result = app.store.lock().await.party_characters(MEMBER_SEED_ID, &id);
    answer(&current, StatusCode::OK, result)
}

/// `POST /__mock/public/characters {roster}`: Asahi's characters as the
/// member-characters boards draw them (`board`), or none (`empty`).
pub async fn mock_roster(State(app): State<App>, Json(req): Json<RosterRequest>) -> StatusCode {
    app.store
        .lock()
        .await
        .seed_member_characters(MEMBER_SEED_ID, req.roster);
    StatusCode::NO_CONTENT
}
