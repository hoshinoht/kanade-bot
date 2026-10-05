//! Admin reads (A3): week, stats, summary, fixed timings, reminders, members,
//! channels, personas, bosses, event bosses and knowledge. Every handler takes
//! [`AdminSession`]; reads go through the shared store's reader connections.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::Uri,
    response::{IntoResponse, Response},
    routing::get,
};

use super::context::{context, frames, next_week, roster, state, unavailable};
use crate::{
    api::{
        auth::AdminSession,
        dto::{self, Art, Named},
        error::ApiError,
        listeners::Site,
    },
    domain::scheduler::Scope,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/week", get(week))
        .route("/api/admin/stats", get(stats))
        .route("/api/admin/summary", get(summary))
        .route("/api/admin/fixed", get(fixed))
        .route("/api/admin/reminders", get(reminders))
        .route("/api/admin/members", get(members))
        .route("/api/admin/channels", get(channels))
        .route("/api/admin/roles", get(roles))
        .route("/api/admin/personas", get(personas))
        .route("/api/admin/bosses", get(bosses))
        .route("/api/admin/bosses/events", get(events))
        .route("/api/admin/bosses/{key}/knowledge", get(knowledge))
}

type Reply = Result<Response, ApiError>;

async fn week(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let next = next_week(&uri)?;
    let now = state.now();
    let [this, following] = frames(state, now)?;
    let frame = if next { following } else { this };
    // Head first: a commit landing in between makes `version` lag the data
    // (a spurious 409 later), never lead it (a lost update under API-1).
    let version = state.store.head().await.map_err(unavailable)?.seq;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![frame.start]))
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    let run_lengths = match &state.config {
        Some(desk) => desk.settings().await.run_lengths,
        None => Default::default(),
    };
    Ok(Json(dto::week::week(
        &ctx,
        &snapshot,
        &frame,
        version,
        &run_lengths,
    ))
    .into_response())
}

async fn stats(State(site): State<Arc<Site>>, _: AdminSession, uri: Uri) -> Reply {
    let state = state(&site)?;
    let next = next_week(&uri)?;
    let now = state.now();
    let [this, following] = frames(state, now)?;
    let frame = if next { following } else { this };
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![frame.start]))
        .await
        .map_err(unavailable)?;
    let ctx = context(&site, state, roster(&[]), now);
    Ok(Json(dto::week::stats(&ctx, &snapshot, &frame)).into_response())
}

async fn summary(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let now = state.now();
    let [this, next] = frames(state, now)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start, next.start]))
        .await
        .map_err(unavailable)?;
    let inbox = state.store.inbox_count().await.map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let members = dto::members::bossers(&profiles, &state.access);
    // The config desk's running settings, so a save shows on the next read.
    let quiet_mode = match &state.config {
        Some(desk) => desk.settings().await.notifications.quiet_mode,
        None => false,
    };
    let model = state
        .model_limits
        .as_ref()
        .map_or_else(Vec::new, |limits| limits(now));
    let live = dto::week::Live {
        quiet_mode,
        model: dto::week::Model::from_groups(&model),
        rescan_off: super::logs::rescan::off_note(state),
    };
    let ctx = context(&site, state, roster(&[]), now);
    Ok(Json(dto::week::summary(&ctx, &snapshot, inbox, members, live)).into_response())
}

async fn fixed(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let now = state.now();
    let [this, next] = frames(state, now)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start, next.start]))
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    Ok(Json(dto::fixed::rows(&ctx, &snapshot, [this.start, next.start])).into_response())
}

async fn reminders(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let now = state.now();
    let [this, next] = frames(state, now)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start, next.start]))
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    Ok(Json(dto::reminders::reminders(&ctx, &snapshot)).into_response())
}

async fn members(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let [this, _] = frames(state, state.now())?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start]))
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let personas = state.profile_options();
    Ok(Json(dto::members::rows(
        &profiles,
        &state.access,
        &personas,
        &snapshot,
        this.start,
    ))
    .into_response())
}

async fn channels(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let channels: Vec<Named> = state
        .channels
        .channels()
        .into_iter()
        .map(|channel| Named {
            id: channel.id,
            name: channel.name,
        })
        .collect();
    Ok(Json(channels).into_response())
}

async fn roles(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    if !state.channels.connected() {
        return Err(ApiError::UNAVAILABLE);
    }
    Ok(Json(dto::roles(&state.channels.roles())).into_response())
}

async fn personas(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let personas = state.profile_options();
    Ok(Json(dto::members::personas(&personas)).into_response())
}

async fn bosses(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    // Weekly timings are always loaded; no runs are needed.
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(unavailable)?;
    let art = Art {
        root: site.boss_dir.as_deref(),
    };
    Ok(Json(dto::bosses::rows(
        &state.catalog,
        &art,
        &snapshot.fixed_runs,
    ))
    .into_response())
}

async fn events(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let art = Art {
        root: site.boss_dir.as_deref(),
    };
    let events = state
        .knowledge_dir
        .as_deref()
        .map(|dir| dto::bosses::events(dir, &art))
        .unwrap_or_default();
    Ok(Json(events).into_response())
}

async fn knowledge(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    key: Result<UrlPath<String>, PathRejection>,
) -> Reply {
    let state = state(&site)?;
    let UrlPath(key) = key.map_err(|_| ApiError::NOT_FOUND)?;
    let dir = state.knowledge_dir.as_deref().ok_or(ApiError::NOT_FOUND)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(unavailable)?;
    let art = Art {
        root: site.boss_dir.as_deref(),
    };
    dto::bosses::knowledge(dir, &state.catalog, &art, &snapshot.fixed_runs, &key)
        .map(|knowledge| Json(knowledge).into_response())
        .ok_or(ApiError::NOT_FOUND)
}
