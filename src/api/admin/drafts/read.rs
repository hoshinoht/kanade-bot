//! `GET /api/admin/drafts?state=open|closed`, `GET /api/admin/drafts/graph`,
//! `GET /api/admin/drafts/{id}` and `GET /api/admin/drafts/{id}/preview`: the
//! session admin's own drafts only. Conflicts are analysed only when a
//! draft's preview is opened.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::Uri,
    response::{IntoResponse, Response},
};

use super::{
    super::{
        context::{context, roster},
        write::{Refusal, state, write_context},
    },
    body::{query_param, seq_range},
    refusal::{Doing, draft as refused, not_found},
};
use crate::api::{
    auth::AdminSession,
    dto::{
        drafts::{self, DraftGraph, DraftList, GraphDraft},
        iso_instant,
    },
    error::ApiError,
    listeners::Site,
    state::ApiState,
};
use kanade_domain::{
    drafts::{DraftStatus, LoadedDraft, closed_at},
    schedule::ScheduleSnapshot,
    scheduler::Scope,
};

type Reply = Result<Response, Refusal>;

/// The session admin's own administrator draft, else `404`.
pub async fn own(
    state: &ApiState,
    session: &AdminSession,
    id: &str,
) -> Result<LoadedDraft, Refusal> {
    match state
        .store
        .draft(id.to_owned())
        .await
        .map_err(Refusal::unavailable)?
    {
        Some(loaded)
            if loaded.draft.kind == kanade_domain::drafts::DraftKind::Admin
                && loaded.draft.author == session.actor =>
        {
            Ok(loaded)
        }
        _ => Err(not_found()),
    }
}

async fn current(state: &ApiState) -> Result<(ScheduleSnapshot, u64), Refusal> {
    let current = state
        .store
        .snapshot(Scope::All)
        .await
        .map_err(Refusal::unavailable)?;
    let head = state.store.head().await.map_err(Refusal::unavailable)?;
    Ok((current, head.seq))
}

pub async fn list(State(site): State<Arc<Site>>, session: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let open = match query_param(&uri, "state")?.as_deref() {
        None | Some("open") => true,
        Some("closed") => false,
        Some(_) => return Err(ApiError::INVALID_QUERY.into()),
    };
    let (_, profiles) = write_context(state).await?;
    let ctx = context(&site, state, roster(&profiles), state.now());
    let (current, head) = current(state).await?;
    let drafts = state
        .store
        .author_drafts(session.actor.clone())
        .await
        .map_err(Refusal::unavailable)?
        .into_iter()
        .filter(|loaded| loaded.draft.status.is_live() == open)
        .map(|loaded| drafts::row(&ctx, &current, &loaded, head))
        .collect();
    Ok(Json(DraftList { head, drafts }).into_response())
}

/// The History graph's lanes for records `from..=to`: every own draft whose
/// span touches them. A draft spans its base to its merge record, to the
/// last record before it was discarded or expired, or (open) to the head;
/// the close time is its closing event's, so no column records it.
pub async fn graph(State(site): State<Arc<Site>>, session: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let (from, to) = seq_range(&uri)?;
    let (_, profiles) = write_context(state).await?;
    let ctx = context(&site, state, roster(&profiles), state.now());
    let (current, head) = current(state).await?;
    // A draft closed before record `from` was written ends below the range.
    let from_at = match from {
        0 => None,
        seq => state
            .store
            .change(seq)
            .await
            .map_err(Refusal::unavailable)?
            .map(|record| record.at),
    };
    let mut lanes = Vec::new();
    for loaded in state
        .store
        .author_drafts(session.actor.clone())
        .await
        .map_err(Refusal::unavailable)?
    {
        let draft = &loaded.draft;
        let closed = if draft.status.is_live() {
            None
        } else {
            let events = state
                .store
                .draft_events(draft.id.clone())
                .await
                .map_err(Refusal::unavailable)?;
            Some(closed_at(&events).unwrap_or(draft.updated_at))
        };
        let reaches = match (draft.status, closed) {
            (_, None) => head >= from,
            (DraftStatus::Merged, _) => draft.merged_seq.is_some_and(|seq| seq >= from),
            (_, Some(at)) => from_at.is_none_or(|first| at >= first),
        };
        if draft.base.seq <= to && reaches {
            lanes.push(GraphDraft {
                draft: drafts::row(&ctx, &current, &loaded, head),
                closed_at: closed.map(iso_instant),
            });
        }
    }
    Ok(Json(DraftGraph {
        head,
        drafts: lanes,
    })
    .into_response())
}

pub async fn detail(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let loaded = own(state, &session, &id).await?;
    let (_, profiles) = write_context(state).await?;
    let ctx = context(&site, state, roster(&profiles), state.now());
    let (current, head) = current(state).await?;
    Ok(Json(drafts::detail(&ctx, &current, &loaded, head)).into_response())
}

pub async fn preview(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    own(state, &session, &id).await?;
    let now = state.now();
    let (write_ctx, profiles) = write_context(state).await?;
    let ctx = context(&site, state, roster(&profiles), now);
    let preview = state
        .writer
        .preview_draft(&id, &write_ctx, now)
        .await
        .map_err(|error| refused(error, Doing::Landing))?;
    let view = drafts::preview(&ctx, &preview).map_err(Refusal::unavailable)?;
    Ok(Json(view).into_response())
}
