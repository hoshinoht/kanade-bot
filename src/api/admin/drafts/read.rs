//! `GET /api/admin/drafts?state=open|closed`, `GET /api/admin/drafts/{id}`
//! and `GET /api/admin/drafts/{id}/preview`: the session admin's own drafts
//! only. Conflicts are analysed only when a draft's preview is opened.

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
    body::query_param,
    refusal::{Doing, draft as refused, not_found},
};
use crate::{
    api::{
        auth::AdminSession,
        dto::drafts::{self, DraftList},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{drafts::LoadedDraft, schedule::ScheduleSnapshot, scheduler::Scope},
};

type Reply = Result<Response, Refusal>;

fn unavailable<T>(_: T) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

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
        .map_err(unavailable)?
    {
        Some(loaded)
            if loaded.draft.kind == crate::domain::drafts::DraftKind::Admin
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
        .map_err(unavailable)?;
    let head = state.store.head().await.map_err(unavailable)?;
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
        .map_err(unavailable)?
        .into_iter()
        .filter(|loaded| loaded.draft.status.is_live() == open)
        .map(|loaded| drafts::row(&ctx, &current, &loaded, head))
        .collect();
    Ok(Json(DraftList { head, drafts }).into_response())
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
    let view = drafts::preview(&ctx, &preview).map_err(unavailable)?;
    Ok(Json(view).into_response())
}
