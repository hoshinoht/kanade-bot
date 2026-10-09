//! The signed-in member's reads (`member-reads`): the boss week as members
//! see it, their own chat allowance and boss art. The week and allowance go
//! through the admin reads' own state, frames and projection context, so
//! days, times and boss-week boundaries match `GET /api/admin/week` exactly;
//! art is the admin origin's handler.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::{HeaderMap, Uri},
    response::{IntoResponse, Response},
};

use crate::{
    api::{
        admin::{
            context::{context, frames, next_week, roster, state, unavailable},
            limits::{allowance_row, allowance_snapshot},
        },
        assets::art_of,
        auth::member::MemberSession,
        dto::public::{MemberAllowance, MemberWeek, member_allowance, member_week},
        error::ApiError,
        listeners::Site,
    },
    domain::scheduler::Scope,
};

/// `GET /art/{kind}/{key}`; every other `/art/…` shape is a plain 404, but
/// only after the session check, so signed-out callers learn nothing.
pub(super) async fn art(
    State(site): State<Arc<Site>>,
    _: MemberSession,
    rest: Result<UrlPath<String>, PathRejection>,
    request: HeaderMap,
) -> Response {
    match rest.as_deref().map(|rest| rest.split_once('/')) {
        Ok(Some((kind, key))) if !key.contains('/') => {
            art_of(&site, kind, key.to_owned(), &request).await
        }
        _ => ApiError::NOT_FOUND.into_response(),
    }
}

/// `GET /api/public/week?week=` (absent or `this`, `next`).
pub(super) async fn week(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    uri: Uri,
) -> Result<Json<MemberWeek>, ApiError> {
    let state = state(&site)?;
    let next = next_week(&uri)?;
    let now = state.now();
    let [this, following] = frames(state, now)?;
    let frame = if next { following } else { this };
    // Head first, as the admin week: `version` may lag the data, never lead it.
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
    Ok(Json(member_week(
        &ctx,
        &snapshot,
        &frame,
        version,
        &run_lengths,
        &session.user_id,
    )))
}

/// `GET /api/public/me/allowance`: the caller's Limits row, their own queued
/// chat call and whether the model is busy; nobody else's figures.
pub(super) async fn allowance(
    State(site): State<Arc<Site>>,
    session: MemberSession,
) -> Result<Json<MemberAllowance>, ApiError> {
    let state = state(&site)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let (snapshot, now) = allowance_snapshot(state);
    let row = profiles
        .iter()
        .find(|profile| profile.member.user_id == session.user_id)
        .and_then(|profile| allowance_row(state, &snapshot, now, profile));
    let groups = state
        .model_limits
        .as_ref()
        .map(|limits| limits(now))
        .unwrap_or_default();
    Ok(Json(member_allowance(
        row,
        snapshot.member_default.1,
        &session.user_id,
        &groups,
        now,
    )))
}
