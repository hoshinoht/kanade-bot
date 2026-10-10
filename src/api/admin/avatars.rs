//! Portrait images for the admin app: `GET /api/admin/members/{id}/avatar`
//! (a stored member's gateway avatar) and `GET /api/admin/me/avatar` (the
//! signed-in admin). Admin session required; monogram when there is none.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::HeaderMap,
    response::Response,
    routing::get,
};

use super::context::{state, unavailable};
use crate::api::{
    auth::AdminSession,
    avatars::{AvatarRef, Portrait, respond},
    error::ApiError,
    listeners::Site,
    state::ApiState,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/members/{id}/avatar", get(member))
        .route("/api/admin/me/avatar", get(me))
}

async fn portrait(state: &ApiState, avatar: Option<&AvatarRef>) -> Option<Portrait> {
    state.avatars.as_ref()?.portrait(avatar).await
}

async fn member(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let state = state(&site)?;
    let Ok(UrlPath(id)) = path else {
        return Err(ApiError::NOT_FOUND);
    };
    if !(1..=20).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::NOT_FOUND);
    }
    let profile = state
        .store
        .member(id.clone())
        .await
        .map_err(unavailable)?
        .ok_or(ApiError::NOT_FOUND)?;
    let name = profile.member.name().unwrap_or(&id).to_owned();
    let found = portrait(state, state.channels.member_avatar(&id).as_ref()).await;
    Ok(respond(found, &name, &headers))
}

/// The gateway's view of the admin's member avatar when it has one (so
/// Account matches Members), else the user avatar stored at sign-in.
async fn me(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let state = state(&site)?;
    let avatar = session.discord_user().and_then(|id| {
        state.channels.member_avatar(id).or_else(|| {
            session
                .avatar_hash
                .as_deref()
                .and_then(|hash| AvatarRef::user(id, hash))
        })
    });
    let found = portrait(state, avatar.as_ref()).await;
    Ok(respond(found, &session.display, &headers))
}
