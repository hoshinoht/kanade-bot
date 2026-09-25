//! `GET /api/admin/chat` and `/api/admin/chat/{id}`.

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
        dto::logs::{chat_row, chat_summary, chat_turn, created_proposals},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::model_log::{ChatFilter, ChatInteraction},
};

async fn every(state: &ApiState, mut filter: ChatFilter) -> Result<Vec<ChatInteraction>, Refusal> {
    let mut rows = Vec::new();
    loop {
        let page = state
            .store
            .chat_logs(filter.clone())
            .await
            .map_err(store_down)?;
        rows.extend(page.items);
        match page.next {
            Some(cursor) => filter.cursor = Some(cursor),
            None => return Ok(rows),
        }
    }
}

/// A withheld question must not be found by its text either: those rows
/// match `q` on the reply alone (the store searches both).
fn searchable(chat: &ChatInteraction, q: Option<&str>) -> bool {
    match q {
        Some(q) if chat.withheld => chat
            .reply
            .to_ascii_lowercase()
            .contains(&q.to_ascii_lowercase()),
        _ => true,
    }
}

pub async fn list(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let filter = filter::chats(&uri, state.policy.zone())?;
    let q = filter.q.clone();
    let facets = state.store.chat_log_facets().await.map_err(store_down)?;
    let rows: Vec<ChatInteraction> = every(state, filter)
        .await?
        .into_iter()
        .filter(|chat| searchable(chat, q.as_deref()))
        .collect();
    let directory = Directory::load(state).await?;
    let names = directory.names();
    Ok(Json(json!({
        "summary": chat_summary(&rows),
        "rows": rows.iter().map(|chat| chat_row(&names, chat)).collect::<Vec<_>>(),
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
    let chat = state
        .store
        .chat_log(id)
        .await
        .map_err(store_down)?
        .ok_or(ApiError::NOT_FOUND)?;
    let cards = state
        .store
        .cards(created_proposals(&chat))
        .await
        .map_err(store_down)?;
    let directory = Directory::load(state).await?;
    Ok(Json(chat_turn(
        &directory.names(),
        &chat,
        &cards,
        state.guild_id.as_deref(),
    ))
    .into_response())
}
