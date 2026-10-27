//! Scenario controls (both origins), each over the live generation and
//! answering 204 once done (409 with the reason when the real API refused
//! the change, 503 between generations):
//!
//! - `POST /__e2e/arrive` `{kind}`: a change made outside the portal
//!   ([`seed::p6::arrive`]): `reaction`, `move`, `run`, `proposal`, `chat`,
//!   `extraction` or `member`.
//! - `POST /__e2e/public/hint` `{topic, member?}`: a change the member
//!   stream turns into `schedule`, `mine` or `allowance` for `member`
//!   (default Asahi; [`seed::p6::member_hint`]).
//! - `POST /__e2e/public/remove` `{run}`: Ren takes Asahi off `run`.
//! - `POST /__e2e/inbox/kalos-v4`: `p-kalos-fixed` changes to Sunday under
//!   an admin editing it; answers `{version}`, the request's new version.
//! - `POST /__e2e/presence` `{at, id, name, editing?, seen_s?, held?,
//!   leave?}`: another admin (`id`, e.g. `discord:<snowflake>`) is at `at`
//!   (`request:<id>` or `run:<id>`), seen `seen_s` ago and aging from there
//!   unless `held`, or (`leave`) gone from it; `DELETE /__e2e/presence`:
//!   nobody else anywhere; `POST /__e2e/presence/board`: the presence
//!   boards' Ren (editing `p-kalos-fixed` and `r-carling`) and Kaito
//!   (viewing `p-kalos-fixed`, 40 s ago), held. Each sends the presence
//!   hint, as a real check-in that changed what others see does.

use std::{fmt::Display, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use kanade::api::{
    dto::{events::Topic, presence::Mode},
    presence::{Place, Visitor},
    state::ApiState,
};
use serde::Deserialize;

use crate::{
    Fail,
    controls::Shared,
    seed::{ids::*, p6},
};

pub fn routes<S: Clone + Send + Sync + 'static>(shared: &Arc<Shared>) -> Router<S> {
    Router::new()
        .route("/__e2e/arrive", post(arrive))
        .route("/__e2e/public/hint", post(member_hint))
        .route("/__e2e/public/remove", post(remove))
        .route("/__e2e/inbox/kalos-v4", post(kalos_v4))
        .route("/__e2e/presence", post(presence).delete(nobody))
        .route("/__e2e/presence/board", post(presence_board))
        .with_state(Arc::clone(shared))
}

fn done(result: Result<(), Fail>) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refused(error),
    }
}

fn refused(error: impl Display) -> Response {
    (StatusCode::CONFLICT, error.to_string()).into_response()
}

fn between() -> Response {
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

/// Runs `$scenario` (an expression over `seed`) on the live generation.
macro_rules! over_live {
    ($shared:expr, |$seed:ident| $scenario:expr) => {{
        let Some(live) = $shared.live().await else {
            return between();
        };
        let $seed = match p6::live(Arc::clone(live.store()), &live.live().state) {
            Ok(seed) => seed,
            Err(error) => return refused(error),
        };
        $scenario
    }};
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arrival {
    kind: String,
}

async fn arrive(
    State(shared): State<Arc<Shared>>,
    body: Result<Json<Arrival>, JsonRejection>,
) -> Response {
    let Ok(Json(Arrival { kind })) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    over_live!(shared, |seed| match p6::arrive(&seed, &kind).await {
        Some(result) => done(result),
        None => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    topic: String,
    member: Option<String>,
}

async fn member_hint(
    State(shared): State<Arc<Shared>>,
    body: Result<Json<Hint>, JsonRejection>,
) -> Response {
    let Ok(Json(Hint { topic, member })) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let member = member.as_deref().unwrap_or(ASAHI);
    over_live!(
        shared,
        |seed| match p6::member_hint(&seed, &topic, member).await {
            Some(result) => done(result),
            None => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
        }
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRef {
    run: String,
}

async fn remove(
    State(shared): State<Arc<Shared>>,
    body: Result<Json<RunRef>, JsonRejection>,
) -> Response {
    let Ok(Json(RunRef { run })) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    over_live!(shared, |seed| done(p6::remove_asahi(&seed, &run).await))
}

async fn kalos_v4(State(shared): State<Arc<Shared>>) -> Response {
    over_live!(shared, |seed| match p6::kalos_v4(&seed).await {
        Ok(version) => Json(serde_json::json!({ "version": version })).into_response(),
        Err(error) => refused(error),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Simulated {
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

/// A simulated admin's one tab.
fn tab(admin: &str) -> String {
    format!("e2e:{admin}")
}

/// Puts `admin` at `place` on `state`'s board.
fn put(
    state: &ApiState,
    place: Place,
    admin: &str,
    name: &str,
    mode: Mode,
    seen_s: u64,
    held: bool,
) {
    let tab = tab(admin);
    let visitor = Visitor {
        tab: &tab,
        admin,
        name,
    };
    state
        .presence
        .simulate(place, visitor, mode, Duration::from_secs(seen_s), held);
}

async fn presence(
    State(shared): State<Arc<Shared>>,
    body: Result<Json<Simulated>, JsonRejection>,
) -> Response {
    let Ok(Json(who)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(place) = Place::parse(&who.at) else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    let Some(live) = shared.live().await else {
        return between();
    };
    let state = &live.live().state;
    if who.leave {
        state.presence.simulate_leave(&place, &tab(&who.id));
    } else {
        let mode = if who.editing {
            Mode::Editing
        } else {
            Mode::Viewing
        };
        put(state, place, &who.id, &who.name, mode, who.seen_s, who.held);
    }
    state.events.notify(Topic::Presence);
    StatusCode::NO_CONTENT.into_response()
}

async fn nobody(State(shared): State<Arc<Shared>>) -> Response {
    let Some(live) = shared.live().await else {
        return between();
    };
    let state = &live.live().state;
    state.presence.clear();
    state.events.notify(Topic::Presence);
    StatusCode::NO_CONTENT.into_response()
}

async fn presence_board(State(shared): State<Arc<Shared>>) -> Response {
    let Some(live) = shared.live().await else {
        return between();
    };
    let state = &live.live().state;
    state.presence.clear();
    let ren = format!("discord:{REN}");
    let kaito = format!("discord:{KAITO}");
    let board = [
        ("request:p-kalos-fixed", &ren, "Ren", Mode::Editing, 0),
        ("request:p-kalos-fixed", &kaito, "Kaito", Mode::Viewing, 40),
        ("run:r-carling", &ren, "Ren", Mode::Editing, 0),
    ];
    for (at, admin, name, mode, seen_s) in board {
        let Some(place) = Place::parse(at) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        put(state, place, admin, name, mode, seen_s, true);
    }
    state.events.notify(Topic::Presence);
    StatusCode::NO_CONTENT.into_response()
}
