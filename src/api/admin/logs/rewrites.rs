//! `GET /api/admin/rewrites` and `/api/admin/rewrites/{id}`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::Uri,
    response::IntoResponse,
};

use super::{Reply, every, filter, state};
use crate::api::admin::write::Refusal;
use crate::api::{
    auth::AdminSession,
    dto::logs::{RewriteFacets, Rewrites, rewrite, rewrite_row, rewrite_summary},
    error::ApiError,
    listeners::Site,
};
use kanade_domain::model_log::RewriteFilter;

pub async fn list(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let filter = filter::rewrites(&uri, state.policy.zone())?;
    let facets = state
        .store
        .rewrite_log_facets()
        .await
        .map_err(Refusal::unavailable)?;
    let rows = every(filter.cursor.clone(), |cursor| {
        state.store.rewrite_logs(RewriteFilter {
            cursor,
            ..filter.clone()
        })
    })
    .await?;
    Ok(Json(Rewrites {
        summary: rewrite_summary(&rows),
        rows: rows.iter().map(rewrite_row).collect(),
        total: facets.total,
        facets: RewriteFacets::from(&facets),
    })
    .into_response())
}

pub async fn detail(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let log = state
        .store
        .rewrite_log(id)
        .await
        .map_err(Refusal::unavailable)?
        .ok_or(ApiError::NOT_FOUND)?;
    Ok(Json(rewrite(&log)).into_response())
}
