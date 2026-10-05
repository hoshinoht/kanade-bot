//! `GET /api/admin/me`: the signed-in admin's Account view. Token and
//! Tailscale sessions stay neutral (no member); a Discord session adds the
//! member's access, named guild roles, the same allowance row as Limits and
//! their reply style. `/api/admin/me/sessions` lists and ends the caller's
//! own sessions (`sessions.rs`).

mod reply;
mod sessions;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    response::IntoResponse,
    routing::{delete, get, post},
};

use super::{
    context::{state, unavailable},
    limits::{allowance_row, allowance_snapshot},
};
use crate::api::{
    auth::AdminSession,
    dto::{
        RoleRow,
        account::{Me, MeMember},
        iso_instant, roles,
    },
    error::ApiError,
    listeners::Site,
    state::ApiState,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/me", get(me))
        .route("/api/admin/me/sessions", get(sessions::list))
        .route("/api/admin/me/sessions/{handle}", delete(sessions::end_one))
        .route(
            "/api/admin/me/sessions/sign-out-others",
            post(sessions::end_others),
        )
}

async fn me(
    State(site): State<Arc<Site>>,
    session: AdminSession,
) -> Result<axum::response::Response, ApiError> {
    let state = state(&site)?;
    let member = match session.discord_user() {
        Some(id) => match state
            .store
            .member(id.to_owned())
            .await
            .map_err(unavailable)?
            .filter(|profile| !profile.member.is_bot)
        {
            Some(profile) => {
                let snapshot = allowance_snapshot(state);
                Some(MeMember {
                    id: profile.member.user_id.clone(),
                    name: profile
                        .member
                        .name()
                        .unwrap_or(&profile.member.user_id)
                        .to_owned(),
                    access: state.access.access(&profile),
                    bossing: profile.member.has_role,
                    roles: named_roles(state, &profile.roles),
                    allowance: allowance_row(state, &snapshot, &profile),
                    reply_style: reply::style(state, &profile).await,
                })
            }
            None => None,
        },
        None => None,
    };
    Ok(Json(Me {
        display: session.display,
        method: session.method.as_str(),
        member,
        server_time: iso_instant(state.now()),
        version: env!("CARGO_PKG_VERSION"),
    })
    .into_response())
}

/// The member's roles in guild order, by name only where the directory knows them.
fn named_roles(state: &ApiState, held: &[String]) -> Option<Vec<RoleRow>> {
    state.channels.connected().then(|| {
        let guild = state.channels.roles();
        roles(&guild)
            .into_iter()
            .filter(|role| held.contains(&role.id))
            .collect()
    })
}
