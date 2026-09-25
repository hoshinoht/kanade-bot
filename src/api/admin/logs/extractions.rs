//! `GET /api/admin/extractions` and `/api/admin/extractions/{id}`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::Uri,
    response::IntoResponse,
};
use serde_json::json;

use super::{Directory, Reply, filter, state, store_down};
use crate::api::admin::write::Refusal;
use crate::{
    api::{
        auth::AdminSession,
        dto::logs::{Proposed, extraction, extraction_row},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::model_log::{ExtractionFilter, ExtractionLog},
};

async fn every(
    state: &ApiState,
    mut filter: ExtractionFilter,
) -> Result<Vec<ExtractionLog>, Refusal> {
    let mut rows = Vec::new();
    loop {
        let page = state
            .store
            .extraction_logs(filter.clone())
            .await
            .map_err(store_down)?;
        rows.extend(page.items);
        match page.next {
            Some(cursor) => filter.cursor = Some(cursor),
            None => return Ok(rows),
        }
    }
}

/// The newest call's alias until runtime settings name the configured one.
async fn current_model(state: &ApiState) -> Result<String, Refusal> {
    let newest = state
        .store
        .extraction_logs(ExtractionFilter {
            limit: 1,
            omit_bodies: true,
            ..ExtractionFilter::default()
        })
        .await
        .map_err(store_down)?;
    Ok(newest
        .items
        .first()
        .map(|log| log.model.clone())
        .unwrap_or_default())
}

pub async fn list(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let filter = filter::extractions(&uri, state.policy.zone())?;
    let facets = state
        .store
        .extraction_log_facets()
        .await
        .map_err(store_down)?;
    let rows = every(state, filter).await?;
    let model = current_model(state).await?;
    let directory = Directory::load(state).await?;
    let names = directory.names();
    Ok(Json(json!({
        "model": model,
        "rows": rows.iter().map(|log| extraction_row(&names, log)).collect::<Vec<_>>(),
        "total": facets.total,
        "facets": names.facets(&facets),
    }))
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
        .extraction_log(id)
        .await
        .map_err(store_down)?
        .ok_or(ApiError::NOT_FOUND)?;
    let cards = state
        .store
        .cards(log.proposal_ids.clone())
        .await
        .map_err(store_down)?;
    let mut drafts = Vec::with_capacity(log.proposal_ids.len());
    for proposal in &log.proposal_ids {
        drafts.push(
            state
                .store
                .draft(proposal.clone())
                .await
                .map_err(store_down)?,
        );
    }
    let proposed: Vec<Proposed<'_>> = log
        .proposal_ids
        .iter()
        .zip(&drafts)
        .map(|(proposal, draft)| Proposed {
            card: cards.iter().find(|card| &card.proposal_id == proposal),
            draft: draft.as_ref(),
        })
        .collect();
    let messages = state
        .store
        .messages_by_id(log.message_ids.clone())
        .await
        .map_err(store_down)?;
    let directory = Directory::load(state).await?;
    Ok(Json(extraction(
        &directory.names(),
        state.policy.zone(),
        &log,
        &proposed,
        &messages,
    ))
    .into_response())
}
