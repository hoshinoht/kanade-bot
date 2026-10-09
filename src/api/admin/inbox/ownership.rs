//! The Inbox Ownership tab: open weekly-timing ownership requests, oldest
//! first, accepted or declined by any admin session acting as staff through
//! `api::ownership` (user decision 2026-10-10). The `Idempotency-Key` is
//! validated, not stored: a retry of this admin's own decision answers as the
//! first did, and accepting pins under the request's own id.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::{
    super::{
        context::{context, roster},
        write::{Refusal, origin, state, write_context},
    },
    refusal,
};
use crate::{
    api::{
        auth::AdminSession,
        dto::inbox::{self, OwnerRequestDto},
        error::ApiError,
        listeners::Site,
        ownership::{self, OwnerDesk},
    },
    domain::{ids::short_id, ownership::OwnerRequestStatus, scheduler::Scope},
};

type Reply = Result<Response, Refusal>;

fn unavailable<T>(_: T) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

/// `GET /api/admin/inbox/ownership`: requests still open now, oldest first
/// (one past its expiry waits for the tick to close it, unlisted).
pub async fn list(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let now = state.now();
    let requests = state
        .store
        .open_owner_requests()
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    // The store lists them oldest first.
    let items: Vec<OwnerRequestDto> = requests
        .iter()
        .filter(|request| request.live(now))
        .filter_map(|request| {
            let fixed = snapshot
                .fixed_runs
                .iter()
                .find(|fixed| fixed.id == request.fixed_run_id)?;
            Some(inbox::owner_request(&ctx, request, fixed))
        })
        .collect();
    Ok(Json(items).into_response())
}

async fn decide(
    site: &Site,
    session: &AdminSession,
    headers: &HeaderMap,
    id: &str,
    accept: bool,
) -> Reply {
    let state = state(site)?;
    let origin = origin(session, headers)?;
    let now = state.now();
    let request = state
        .store
        .owner_request(id.to_owned())
        .await
        .map_err(unavailable)?
        .ok_or_else(refusal::not_found)?;
    let wanted = if accept {
        OwnerRequestStatus::Accepted
    } else {
        OwnerRequestStatus::Declined
    };
    let (write_ctx, profiles) = write_context(state).await?;
    let decided = if request.status == wanted
        && request.decided_by.as_deref() == Some(ownership::actor(&origin).as_str())
    {
        request
    } else {
        // Staff decide regardless of who owns it; the decider only matters
        // to the owner rule, which staff skip.
        let decider = session
            .discord_user()
            .unwrap_or_else(|| session.actor.id())
            .to_owned();
        let desk = OwnerDesk {
            store: &*state.store,
            writer: &*state.writer,
            ctx: &write_ctx,
        };
        desk.decide(origin, id, &decider, true, accept, now)
            .await
            .map_err(refusal::ownership)?
            .0
    };
    let ctx = context(site, state, roster(&profiles), now);
    let who = ctx.name(&decided.requester);
    let timing = short_id(&decided.fixed_run_id);
    let text = if accept {
        format!("{who} now owns weekly timing #{timing}.")
    } else {
        format!("Declined {who}'s request to own weekly timing #{timing}.")
    };
    Ok(Json(json!({ "message": text })).into_response())
}

pub async fn accept(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    decide(&site, &session, &headers, &id, true).await
}

pub async fn decline(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    decide(&site, &session, &headers, &id, false).await
}
