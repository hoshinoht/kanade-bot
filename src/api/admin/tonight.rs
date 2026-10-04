//! `GET /api/admin/auth/tonight`: the one schedule read the signed-out sign-in
//! page makes (owner decision 2026-10-04). It takes no session, so it answers
//! only today's next run as time, boss names and the aggregate tally.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};

use super::context::{context, frames, roster, state, unavailable};
use crate::{
    api::{dto, error::ApiError, listeners::Site},
    domain::scheduler::Scope,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/auth/tonight", get(tonight))
}

async fn tonight(State(site): State<Arc<Site>>) -> Result<Response, ApiError> {
    let state = state(&site)?;
    let now = state.now();
    // Both weeks: a reset later today puts tonight's run in the next one.
    let [this, next] = frames(state, now)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start, next.start]))
        .await
        .map_err(unavailable)?;
    let ctx = context(&site, state, roster(&[]), now);
    Ok(Json(dto::week::tonight(&ctx, &snapshot)).into_response())
}
