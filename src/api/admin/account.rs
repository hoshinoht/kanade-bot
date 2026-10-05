//! `GET /api/admin/me`: the signed-in admin's Account view. Token and
//! Tailscale sessions stay neutral (no member); a Discord session adds the
//! member's access, named guild roles and the same allowance row as Limits.

use std::sync::Arc;

use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};

use super::{
    context::{state, unavailable},
    limits::{allowance_row, allowance_snapshot},
};
use crate::api::{
    auth::AdminSession,
    dto::{
        RoleRow,
        account::{Me, MeMember},
        roles,
    },
    error::ApiError,
    listeners::Site,
    state::ApiState,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/me", get(me))
}

async fn me(
    State(site): State<Arc<Site>>,
    session: AdminSession,
) -> Result<axum::response::Response, ApiError> {
    let state = state(&site)?;
    let member = match session.discord_user() {
        Some(id) => state
            .store
            .member(id.to_owned())
            .await
            .map_err(unavailable)?
            .filter(|profile| !profile.member.is_bot)
            .map(|profile| {
                let snapshot = allowance_snapshot(state);
                MeMember {
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
                }
            }),
        None => None,
    };
    Ok(Json(Me {
        display: session.display,
        method: session.method.as_str(),
        member,
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
