//! The member writes on the public origin (`member-writes-contract`): own
//! answer and move, the Discord deep-link view, and requests (list, submit,
//! withdraw), each behind `require_session` and, for writes, the server's
//! `admit` (token, required `Idempotency-Key`, a fresh sign-in for all but a
//! withdrawal). `POST /__mock/public/{unfresh,remove,end-week}` stand in for
//! an aged sign-in, an admin taking the member off a run, and a boss-week
//! reset while a page is open.

use crate::{
    App,
    api::error,
    mock::{
        member_runs::{AnswerBody, MoveBody},
        portal::MEMBER_SEED_ID,
        requests::{Refused, RequestBody},
    },
    public::{admit, carry, member, written},
};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

fn invalid_body() -> Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_body",
        "The request body is not valid.",
    )
}

/// `GET /api/public/runs/{id}`: `MemberRunLink`.
pub async fn link(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let found = app.store.lock().await.member_link(MEMBER_SEED_ID, &id);
    carry(crate::api::outcome(found), &current)
}

/// `PUT /api/public/runs/{id}/answer`.
pub async fn answer(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<AnswerBody>, JsonRejection>,
) -> Response {
    let (current, key) = match admit(&app, &method, &headers, true).await {
        Ok(admitted) => admitted,
        Err(refused) => return *refused,
    };
    let Ok(Json(body)) = body else {
        return carry(invalid_body(), &current);
    };
    let result = app
        .store
        .lock()
        .await
        .member_answer(MEMBER_SEED_ID, &key, &id, body);
    written(&app, &current, StatusCode::OK, result)
}

/// `POST /api/public/runs/{id}/move`.
pub async fn move_run(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<MoveBody>, JsonRejection>,
) -> Response {
    let (current, key) = match admit(&app, &method, &headers, true).await {
        Ok(admitted) => admitted,
        Err(refused) => return *refused,
    };
    let Ok(Json(body)) = body else {
        return carry(invalid_body(), &current);
    };
    let result = app
        .store
        .lock()
        .await
        .member_move(MEMBER_SEED_ID, &key, &id, body);
    written(&app, &current, StatusCode::OK, result)
}

/// `GET /api/public/requests/mine`: `MemberRequests`.
pub async fn requests(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    let current = match member(&app, &method, &headers).await {
        Ok(current) => current,
        Err(refused) => return *refused,
    };
    let body = app.store.lock().await.member_requests(MEMBER_SEED_ID);
    carry(Json(body).into_response(), &current)
}

/// `POST /api/public/requests`: `201` with the new request; an exact retry
/// `200` with it as it now is; a limit `429 request_limit` with `limit`.
pub async fn submit(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    body: Result<Json<RequestBody>, JsonRejection>,
) -> Response {
    let (current, key) = match admit(&app, &method, &headers, true).await {
        Ok(admitted) => admitted,
        Err(refused) => return *refused,
    };
    let Ok(Json(body)) = body else {
        return carry(invalid_body(), &current);
    };
    let result = app
        .store
        .lock()
        .await
        .member_request(MEMBER_SEED_ID, &key, body);
    match result {
        Ok((created, request)) => {
            let status = if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            written(&app, &current, status, Ok(request))
        }
        Err(Refused::Coded(refusal)) => written::<()>(&app, &current, StatusCode::OK, Err(refusal)),
        Err(Refused::Limit(limit)) => {
            let message = if limit == "open" {
                "You already have the most requests waiting; withdraw one or wait for a decision."
            } else {
                "You've sent the most requests allowed in a day; try again tomorrow."
            };
            let body = json!({ "error": "request_limit", "message": message, "limit": limit });
            carry(
                (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response(),
                &current,
            )
        }
    }
}

/// `POST /api/public/requests/{id}/withdraw`: no fresh sign-in.
pub async fn withdraw(
    State(app): State<App>,
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (current, key) = match admit(&app, &method, &headers, false).await {
        Ok(admitted) => admitted,
        Err(refused) => return *refused,
    };
    let result = app
        .store
        .lock()
        .await
        .member_withdraw_request(MEMBER_SEED_ID, &key, &id);
    written(&app, &current, StatusCode::OK, result)
}

/// `POST /__mock/public/unfresh`: the member's sign-ins age past the
/// fresh-write window; signing in again starts a fresh one.
pub async fn mock_unfresh(State(app): State<App>) -> StatusCode {
    app.store.lock().await.public_sessions().age_sign_ins();
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
pub struct RunRef {
    run: String,
}

/// `POST /__mock/public/remove {run}`: an admin (Ren) takes the member off a run.
pub async fn mock_remove(State(app): State<App>, Json(req): Json<RunRef>) -> Response {
    let removed = app
        .store
        .lock()
        .await
        .remove_member(MEMBER_SEED_ID, &req.run);
    match removed {
        Ok(()) => {
            app.hints.emit("schedule");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(refusal) => crate::api::outcome::<()>(Err(refusal)),
    }
}

/// `POST /__mock/public/end-week`: this boss week reset while pages were open.
pub async fn mock_end_week(State(app): State<App>) -> StatusCode {
    app.store.lock().await.end_week();
    StatusCode::NO_CONTENT
}
