//! `/api/admin/presence` (presence.json), mirroring the Rust routes: `PUT`
//! checks in and answers the others, `GET` reads, `DELETE` leaves. CSRF is the
//! shared write layer's (`writes.rs`). The mock's one admin is never listed;
//! the others are simulated (`mock/presence.rs`), driven by `/__mock/presence`.

use axum::{
    Json,
    extract::{
        Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::{App, api::error, mock::presence};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    kind: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckIn {
    at: Place,
    mode: String,
    /// Required as on the server, which orders a tab's calls by it; the
    /// mock never lists its one admin, so there is nothing here to order.
    #[serde(rename = "seq")]
    _seq: u64,
}

fn invalid_body() -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "invalid_body",
        "The request body is not valid.",
    )
}

fn invalid_query() -> Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_query",
        "A query parameter is not valid.",
    )
}

type Pairs = Result<Query<Vec<(String, String)>>, QueryRejection>;

/// `?at=<kind>:<id>` (percent-decoded, as the client encodes it), plus
/// `&seq=<n>` exactly when `seq` is wanted (the leave); nothing else.
fn at(query: Pairs, seq: bool) -> Option<String> {
    let Ok(Query(pairs)) = query else {
        return None;
    };
    match pairs.as_slice() {
        [(key, value)] if key == "at" && !seq => presence::parse(value),
        [(key, value), (n, number)] if key == "at" && n == "seq" && seq => {
            number.parse::<u64>().ok()?;
            presence::parse(value)
        }
        _ => None,
    }
}

pub async fn check_in(
    State(app): State<App>,
    body: Result<Json<CheckIn>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return invalid_body();
    };
    let Some(place) = presence::place(&body.at.kind, &body.at.id) else {
        return invalid_body();
    };
    if !matches!(body.mode.as_str(), "viewing" | "editing") {
        return invalid_body();
    }
    let here = app.store.lock().await.presence().here(&place);
    Json(json!({ "heartbeat_s": 15, "ttl_s": 60, "here": here })).into_response()
}

pub async fn read(State(app): State<App>, query: Pairs) -> Response {
    let Some(place) = at(query, false) else {
        return invalid_query();
    };
    let here = app.store.lock().await.presence().here(&place);
    Json(json!({ "here": here })).into_response()
}

pub async fn leave(query: Pairs) -> Response {
    match at(query, true) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => invalid_query(),
    }
}

/// `POST /__mock/presence`: another admin arrives at (or changes mode at) a
/// place, seen `seen_s` ago; `{"at", "id", "leave": true}` takes them away.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Simulated {
    at: String,
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    editing: bool,
    #[serde(default)]
    seen_s: u64,
    #[serde(default)]
    held: bool,
    #[serde(default)]
    leave: bool,
}

pub async fn simulate(State(app): State<App>, Json(who): Json<Simulated>) -> StatusCode {
    let Some(place) = presence::parse(&who.at) else {
        return StatusCode::UNPROCESSABLE_ENTITY;
    };
    {
        let mut store = app.store.lock().await;
        let board = store.presence();
        if who.leave {
            board.take(&place, &who.id);
        } else {
            board.put(place, who.id, who.name, who.editing, who.seen_s, who.held);
        }
    }
    app.hints.emit("presence");
    StatusCode::NO_CONTENT
}

/// `POST /__mock/presence/board`: the presence boards' Ren and Kaito (dev seed).
pub async fn board(State(app): State<App>) -> StatusCode {
    app.store.lock().await.presence().board();
    app.hints.emit("presence");
    StatusCode::NO_CONTENT
}

/// `DELETE /__mock/presence`: nobody else anywhere.
pub async fn clear(State(app): State<App>) -> StatusCode {
    app.store.lock().await.presence().clear();
    app.hints.emit("presence");
    StatusCode::NO_CONTENT
}
