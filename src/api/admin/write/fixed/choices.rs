//! `POST /api/admin/fixed/{id}/choices`: the runs a save moving the timing to
//! this day and time would ask update or keep about, as the save itself
//! decides them (`preview_fixed_edit`). Nothing is written.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use super::{Body, Refusal, Reply, scheduler, slot, state};
use crate::api::{
    admin::context::{context, frames, roster, run_ends, weeks},
    auth::AdminSession,
    dto,
    listeners::Site,
};
use kanade_domain::schedule::{Draft, FixedEdit, preview_fixed_edit};

/// `FixedChoicesRequest`: the day and time the editor is about to save.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoicesRequest {
    weekday: u8,
    time: String,
}

/// The runs the save takes a decision for (none: send no `decisions`).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct FixedChoices {
    runs: Vec<dto::fixed::FixedRunLink>,
}

pub async fn choices(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(fixed_id): UrlPath<String>,
    Body(request): Body<ChoicesRequest>,
) -> Reply {
    let state = state(&site)?;
    let (weekday, time) = slot(request.weekday, &request.time)?;
    let gone = || Refusal::not_found("not_found", "That weekly timing no longer exists.");
    let now = state.now();
    let [this, next] = frames(state, now).map_err(Refusal::from)?;
    let snapshot = weeks(state, vec![this.start, next.start]).await?;
    let current = snapshot
        .fixed_runs
        .iter()
        .find(|fixed| fixed.id == fixed_id)
        .ok_or_else(gone)?;
    // As the save builds its edit: only a field that differs is changed.
    let edit = FixedEdit {
        weekday: (current.weekday != weekday).then_some(weekday),
        time: (current.time != time).then_some(time),
        ..FixedEdit::default()
    };
    let ends = Arc::new(run_ends(state).await);
    let draft = Draft::new(snapshot.clone()).with_run_ends(Some(Arc::clone(&ends)));
    let listed = preview_fixed_edit(&draft, &fixed_id, &edit, &state.policy, now)
        .map_err(|error| scheduler(error.into()))?;
    let profiles = state.store.members().await.map_err(Refusal::unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    let row = dto::fixed::rows(&ctx, &snapshot, [this.start, next.start], &ends)
        .into_iter()
        .find(|row| row.id == fixed_id)
        .ok_or_else(gone)?;
    let runs = row
        .runs
        .into_iter()
        .filter(|run| listed.iter().any(|listed| listed.run_id == run.run_id))
        .collect();
    Ok(Json(FixedChoices { runs }).into_response())
}
