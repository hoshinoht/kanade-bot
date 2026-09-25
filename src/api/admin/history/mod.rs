//! History (A5): the record list and records, blame, the chain check, and
//! rollbacks (revert, week restore, actor revert) with preview-then-apply.
//! Reads use the store's readers; rollbacks and their previews go through
//! the one scheduler writer.

pub(super) mod parse;
mod rollback;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::Uri,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;

use super::context::{state, unavailable};
use crate::{
    api::{
        auth::{AdminSession, wire},
        dto::history as dto,
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        history::{BlameTarget, ChangeFilter, ChangeRef},
        scheduler::Scope,
    },
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/history", get(page))
        .route("/api/admin/history/checkpoints", get(checkpoints))
        .route("/api/admin/history/revert", post(rollback::revert))
        .route(
            "/api/admin/history/restore-week",
            post(rollback::restore_week),
        )
        .route(
            "/api/admin/history/revert-actor",
            post(rollback::revert_actor),
        )
        .route("/api/admin/history/{seq}", get(record))
        .route("/api/admin/runs/{id}/blame", get(blame))
}

type Reply = Result<Response, ApiError>;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;
/// Stores keep seqs as SQLite integers: a larger one names no record.
pub(super) const MAX_SEQ: u64 = i64::MAX as u64;
const PARAMS: [&str; 4] = ["week", "actor", "before", "limit"];

/// A record the encoder cannot write (an instant out of range) is a server fault.
fn encoded<T, E>(result: Result<T, E>) -> Result<T, ApiError> {
    result.map_err(|_| ApiError::UNAVAILABLE)
}

fn head_json(head: &ChangeRef) -> serde_json::Value {
    json!({"seq": head.seq, "hash": head.hash})
}

struct PageQuery {
    filter: ChangeFilter,
    before: Option<u64>,
    limit: usize,
}

/// `week`, `actor`, `before` and `limit`, each at most once; an unknown key or
/// malformed value is `422 invalid_query`.
fn page_query(state: &ApiState, uri: &Uri) -> Result<PageQuery, ApiError> {
    let pairs = wire::query_pairs(uri.query());
    // `query_pairs` drops pairs it cannot decode; those are refused too.
    let sent = uri.query().map_or(0, |query| {
        query.split('&').filter(|pair| !pair.is_empty()).count()
    });
    if pairs.len() != sent || pairs.iter().any(|(key, _)| !PARAMS.contains(&key.as_str())) {
        return Err(ApiError::INVALID_QUERY);
    }
    let value = |key: &str| -> Result<Option<String>, ApiError> {
        let present = pairs.iter().any(|(name, _)| name == key);
        match wire::query_value(&pairs, key) {
            Some(value) => Ok(Some(value)),
            None if present => Err(ApiError::INVALID_QUERY),
            None => Ok(None),
        }
    };
    let week = value("week")?
        .map(|text| parse::week(&state.policy, &text).ok_or(ApiError::INVALID_QUERY))
        .transpose()?;
    let actor = value("actor")?
        .map(|text| parse::actor(&text).ok_or(ApiError::INVALID_QUERY))
        .transpose()?;
    let number = |key: &str| -> Result<Option<u64>, ApiError> {
        value(key)?
            .map(|text| {
                text.bytes()
                    .all(|byte| byte.is_ascii_digit())
                    .then(|| text.parse::<u64>().ok())
                    .flatten()
                    .ok_or(ApiError::INVALID_QUERY)
            })
            .transpose()
    };
    let before = number("before")?
        .map(|before| {
            (before <= MAX_SEQ)
                .then_some(before)
                .ok_or(ApiError::INVALID_QUERY)
        })
        .transpose()?;
    let limit = match number("limit")? {
        None => DEFAULT_LIMIT,
        Some(limit @ 1..=100) => usize::try_from(limit).unwrap_or(MAX_LIMIT),
        Some(_) => return Err(ApiError::INVALID_QUERY),
    };
    let filter = match (week, actor) {
        (None, None) => ChangeFilter::All,
        (Some(week), None) => ChangeFilter::Week(week),
        (None, Some(actor)) => ChangeFilter::Actor(actor),
        (Some(week), Some(actor)) => ChangeFilter::ActorInWeek(actor, week),
    };
    Ok(PageQuery {
        filter,
        before,
        limit,
    })
}

async fn page(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let query = page_query(state, &uri)?;
    // Head first, as the Week read: it never runs ahead of the page.
    let head = state.store.head().await.map_err(unavailable)?;
    let total = state
        .store
        .history_total(query.filter.clone())
        .await
        .map_err(unavailable)?;
    let slice = state
        .store
        .history_page(query.filter, query.before, query.limit)
        .await
        .map_err(unavailable)?;
    let records = encoded(
        slice
            .records
            .iter()
            .map(dto::record)
            .collect::<Result<Vec<_>, _>>(),
    )?;
    Ok(Json(json!({
        "records": records,
        "head": head_json(&head),
        "next_before": slice.next_before,
        "total": total,
    }))
    .into_response())
}

async fn record(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    seq: Result<UrlPath<u64>, PathRejection>,
) -> Reply {
    let state = state(&site)?;
    // A non-numeric seq names no record.
    let UrlPath(seq) = seq.map_err(|_| ApiError::NOT_FOUND)?;
    if seq > MAX_SEQ {
        return Err(ApiError::NOT_FOUND);
    }
    let record = state
        .store
        .change(seq)
        .await
        .map_err(unavailable)?
        .ok_or(ApiError::NOT_FOUND)?;
    Ok(Json(encoded(dto::record(&record))?).into_response())
}

/// The chain check and the backup manifests. No backup directory is
/// configured yet, so `backups` is empty (A5-9); named checkpoints await a
/// schema extension.
async fn checkpoints(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let verification = state.store.verify_history().await.map_err(unavailable)?;
    let head = verification.head.clone().unwrap_or(ChangeRef {
        seq: 0,
        hash: crate::domain::history::GENESIS_PREV_HASH.to_owned(),
    });
    Ok(Json(json!({
        "verified": {
            "ok": verification.is_intact(),
            "checked": verification.records,
            "head": head_json(&head),
        },
        "backups": [],
    }))
    .into_response())
}

async fn blame(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(run_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let blame = state
        .store
        .blame(BlameTarget::Run(run_id.clone()))
        .await
        .map_err(unavailable)?
        .ok_or(ApiError::NOT_FOUND)?;
    let snapshot = state
        .store
        .snapshot(Scope::Run(run_id.clone()))
        .await
        .map_err(unavailable)?;
    Ok(Json(encoded(dto::blame(&blame, &run_id, &snapshot))?).into_response())
}
