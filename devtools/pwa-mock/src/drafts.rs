//! `/api/admin/drafts` (drafts.json), mirroring the Rust routes. CSRF and
//! `Idempotency-Key` replay are the shared write layer's (`writes.rs`).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    App,
    api::outcome,
    mock::drafts::{AddOp, Rebase},
};

#[derive(Deserialize, Default)]
pub struct ListQuery {
    state: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct VersionQuery {
    version: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetitleBody {
    version: Option<u64>,
    title: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MergeBody {
    version: Option<u64>,
}

pub async fn list(State(app): State<App>, Query(q): Query<ListQuery>) -> Response {
    outcome(app.store.lock().await.draft_list(q.state.as_deref()))
}

pub async fn create(State(app): State<App>, body: Option<Json<CreateBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let written = app.store.lock().await.draft_create(body.title.as_deref());
    (StatusCode::CREATED, Json(written)).into_response()
}

pub async fn detail(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.draft_detail(&id))
}

pub async fn retitle(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<RetitleBody>,
) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .draft_retitle(&id, body.version, &body.title),
    )
}

pub async fn discard(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<VersionQuery>,
) -> Response {
    outcome(app.store.lock().await.draft_discard(&id, q.version))
}

pub async fn add_op(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<AddOp>,
) -> Response {
    outcome(app.store.lock().await.draft_add(&id, body))
}

pub async fn remove_op(
    State(app): State<App>,
    Path((id, op)): Path<(String, String)>,
    Query(q): Query<VersionQuery>,
) -> Response {
    outcome(app.store.lock().await.draft_remove(&id, &op, q.version))
}

pub async fn preview(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.draft_preview(&id))
}

pub async fn rebase(
    State(app): State<App>,
    Path(id): Path<String>,
    body: Option<Json<Rebase>>,
) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    outcome(app.store.lock().await.draft_rebase(&id, body))
}

pub async fn merge(
    State(app): State<App>,
    Path(id): Path<String>,
    body: Option<Json<MergeBody>>,
) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    outcome(app.store.lock().await.draft_merge(&id, body.version))
}

/// `POST /__mock/inbox/kalos-v4`: the member changes `p-kalos-fixed`.
pub async fn kalos_v4(State(app): State<App>) -> Response {
    if app.store.lock().await.kalos_v4() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}
