//! `GET /api/admin/inbox/past?before=&limit=`: closed proposals and member
//! requests, newest closed first, for the Inbox's read-only Past tab. No
//! merge previews: a closed item never applies again.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::Uri,
    response::{IntoResponse, Response},
};

use super::{
    super::context::{context, roster, state, unavailable},
    messages::cited,
};
use crate::api::{
    auth::{AdminSession, wire},
    dto::inbox_past::{self, PastPage},
    error::ApiError,
    listeners::Site,
};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
const PARAMS: [&str; 2] = ["before", "limit"];

struct PageQuery {
    /// The id of the last item already shown.
    before: Option<String>,
    limit: usize,
}

/// `before` and `limit`, each at most once; anything else is `422 invalid_query`.
fn page_query(uri: &Uri) -> Result<PageQuery, ApiError> {
    let pairs = wire::query_pairs(uri.query());
    let sent = uri.query().map_or(0, |query| {
        query.split('&').filter(|pair| !pair.is_empty()).count()
    });
    if pairs.len() != sent || pairs.iter().any(|(key, _)| !PARAMS.contains(&key.as_str())) {
        return Err(ApiError::INVALID_QUERY);
    }
    let value = |key: &str| -> Result<Option<String>, ApiError> {
        let present = pairs.iter().any(|(name, _)| name == key);
        match wire::query_value(&pairs, key) {
            Some(value) if !value.is_empty() => Ok(Some(value)),
            None if !present => Ok(None),
            _ => Err(ApiError::INVALID_QUERY),
        }
    };
    let limit = match value("limit")? {
        None => DEFAULT_LIMIT,
        Some(text) => text
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| text.parse::<usize>().ok())
            .flatten()
            .filter(|limit| (1..=MAX_LIMIT).contains(limit))
            .ok_or(ApiError::INVALID_QUERY)?,
    };
    Ok(PageQuery {
        before: value("before")?,
        limit,
    })
}

pub async fn past(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    uri: Uri,
) -> Result<Response, ApiError> {
    let state = state(&site)?;
    let query = page_query(&uri)?;
    let mut closed = state.store.closed_inbox().await.map_err(unavailable)?;
    closed.sort_by(|a, b| inbox_past::sort_key(b).cmp(&inbox_past::sort_key(a)));
    // Closed items never change, so the cursor item keeps its place; one that
    // names no closed item is refused rather than restarting the list.
    let start = match &query.before {
        None => 0,
        Some(id) => {
            closed
                .iter()
                .position(|item| &item.draft.id == id)
                .ok_or(ApiError::INVALID_QUERY)?
                + 1
        }
    };
    let page = &closed[start..closed.len().min(start + query.limit)];
    let next_before = (start + page.len() < closed.len())
        .then(|| page.last().map(|item| item.draft.id.clone()))
        .flatten();

    // Only this page's cards and cited messages are read, each in one call.
    let proposal_ids: Vec<String> = page
        .iter()
        .filter(|item| item.proposal.is_some())
        .map(|item| item.draft.id.clone())
        .collect();
    let cards = if proposal_ids.is_empty() {
        Vec::new()
    } else {
        state.store.cards(proposal_ids).await.map_err(unavailable)?
    };
    let cited_ids: Vec<String> = cards
        .iter()
        .flat_map(|card| card.details.evidence_message_ids.iter().cloned())
        .collect();
    let cached = if cited_ids.is_empty() {
        Vec::new()
    } else {
        state
            .store
            .messages_by_id(cited_ids)
            .await
            .map_err(unavailable)?
    };
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), state.now());
    let items = page
        .iter()
        .filter_map(|item| {
            let card = cards.iter().find(|card| card.proposal_id == item.draft.id);
            let evidence = card.map_or_else(Vec::new, |card| cited(&ctx, card, &cached));
            inbox_past::item(&ctx, item, card, evidence)
        })
        .collect();
    Ok(Json(PastPage { items, next_before }).into_response())
}
