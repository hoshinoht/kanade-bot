//! History (A5): the record list, blame, the chain check, and
//! rollbacks (revert, week restore, actor revert) with preview-then-apply.
//! Pages also carry Config section saves (view-only, outside the chain).
//! Reads use the store's readers; rollbacks and their previews go through
//! the one scheduler writer.

pub(super) mod backups;
pub(super) mod parse;
mod rollback;
mod settings;
mod sign_ins;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::Uri,
    response::{IntoResponse, Response},
    routing::{get, post},
};

use super::context::{state, unavailable};
use crate::{
    api::{
        auth::{AdminSession, wire},
        dto::history as dto,
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::history::{ChangeFilter, ChangeRef},
    infrastructure::store::snapshots::HistoryViewQuery,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/history", get(page))
        .route("/api/admin/history/checkpoints", get(checkpoints))
        .route("/api/admin/history/sign-ins", get(sign_ins::page))
        .route("/api/admin/history/revert", post(rollback::revert))
        .route(
            "/api/admin/history/restore-week",
            post(rollback::restore_week),
        )
        .route(
            "/api/admin/history/revert-actor",
            post(rollback::revert_actor),
        )
}

type Reply = Result<Response, ApiError>;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;
/// Stores keep seqs as SQLite integers: a larger one names no record.
pub(super) const MAX_SEQ: u64 = i64::MAX as u64;
const PARAMS: [&str; 5] = ["week", "actor", "run", "before", "limit"];

/// A record the encoder cannot write (an instant out of range) is a server fault.
fn encoded<T, E>(result: Result<T, E>) -> Result<T, ApiError> {
    result.map_err(|_| ApiError::UNAVAILABLE)
}

struct PageQuery {
    filter: ChangeFilter,
    before: Option<u64>,
    limit: usize,
}

/// `week`, `actor`, `run`, `before` and `limit`, each at most once; an
/// unknown key or malformed value is `422 invalid_query`, and so is `run`
/// beside `week` or `actor` (a run's log is the whole of it).
fn page_query(state: &ApiState, uri: &Uri) -> Result<PageQuery, ApiError> {
    // An undecodable pair is refused too.
    let pairs = wire::strict_query_pairs(uri.query()).ok_or(ApiError::INVALID_QUERY)?;
    if pairs.iter().any(|(key, _)| !PARAMS.contains(&key.as_str())) {
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
    let run = value("run")?
        .map(|text| parse::run_id(&text).ok_or(ApiError::INVALID_QUERY))
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
    let filter = match (week, actor, run) {
        (None, None, None) => ChangeFilter::All,
        (Some(week), None, None) => ChangeFilter::Week(week),
        (None, Some(actor), None) => ChangeFilter::Actor(actor),
        (Some(week), Some(actor), None) => ChangeFilter::ActorInWeek(actor, week),
        (None, None, Some(run)) => ChangeFilter::Run(run),
        (_, _, Some(_)) => return Err(ApiError::INVALID_QUERY),
    };
    Ok(PageQuery {
        filter,
        before,
        limit,
    })
}

/// Head, total, records and settings saves come from one store snapshot,
/// so a write landing mid-read never shows in one part and not another.
async fn page(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let query = page_query(state, &uri)?;
    let view = state
        .store
        .history_view(HistoryViewQuery {
            filter: query.filter.clone(),
            before: query.before,
            limit: query.limit,
            settings: settings::query(&query.filter),
        })
        .await
        .map_err(unavailable)?;
    // A run with no row and no record is unknown; a gone run keeps its log.
    if view.total == 0 && !view.run_known {
        return Err(ApiError::INVALID_QUERY);
    }
    let (settings, settings_total) = settings::page(state, &query.filter, query.before, &view)?;
    let records = encoded(
        view.records
            .iter()
            .map(dto::record)
            .collect::<Result<Vec<_>, _>>(),
    )?;
    Ok(Json(dto::HistoryPage {
        records,
        head: (&view.head).into(),
        next_before: view.next_before,
        total: view.total,
        settings,
        settings_total,
    })
    .into_response())
}

/// The chain check and the backup manifests in `KANADE_BACKUP_DIR` (A5-9),
/// both redone on every request; named checkpoints await a schema extension.
async fn checkpoints(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let verification = state.store.verify_history().await.map_err(unavailable)?;
    let found = match state.backups.dir.clone() {
        Some(dir) => backups::list(dir).await,
        None => Vec::new(),
    };
    let mut listed = Vec::with_capacity(found.len());
    for backup in &found {
        let anchored = state
            .store
            .contains_anchor(backup.manifest.history_head.clone())
            .await
            .map_err(unavailable)?;
        listed.push(backups::row(backup, anchored, state.backups.schema_version));
    }
    let head = verification.head.clone().unwrap_or(ChangeRef {
        seq: 0,
        hash: crate::domain::history::GENESIS_PREV_HASH.to_owned(),
    });
    Ok(Json(dto::Checkpoints {
        verified: dto::Verified {
            ok: verification.is_intact(),
            checked: verification.records,
            head: (&head).into(),
            first_broken: verification.first_broken.map(|broken| broken.seq),
        },
        backup_dir_configured: state.backups.dir.is_some(),
        backups: listed,
    })
    .into_response())
}
