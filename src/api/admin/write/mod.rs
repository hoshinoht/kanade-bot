//! Admin mutations (A4). Every handler takes [`AdminSession`] (CSRF on unsafe
//! methods), attributes its change to the session (`Surface::AdminPortal`,
//! or `Cli` for the bearer), maps `Idempotency-Key` to the request id, and
//! writes through the one scheduler writer.

mod fixed;
mod members;
mod precondition;
mod refusal;
mod runs;

use std::sync::Arc;

use axum::{
    Router,
    http::HeaderMap,
    routing::{delete, patch, post},
};

use super::context::{roster, unavailable};
use crate::{
    api::{
        auth::AdminSession, error::ApiError, listeners::Site, state::ApiState, write::WriteContext,
    },
    domain::{history::Origin, members::MemberProfile},
};

pub use refusal::{Refusal, scheduler};
pub use runs::strict_time;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/runs/{id}/move", post(runs::move_run))
        .route("/api/admin/runs/{id}/swap", post(runs::swap))
        .route("/api/admin/runs/{id}/status", patch(runs::status))
        .route("/api/admin/runs/{id}/rsvp", post(runs::rsvp))
        .route(
            "/api/admin/runs/{id}/participants",
            patch(runs::participants),
        )
        .route("/api/admin/runs/{id}/reset", post(runs::reset))
        .route("/api/admin/runs/{id}/ping", post(runs::ping))
        .route("/api/admin/fixed", post(fixed::create))
        .route(
            "/api/admin/fixed/{id}",
            patch(fixed::update).delete(fixed::retire),
        )
        .route("/api/admin/validate/bosses", post(fixed::validate_bosses))
        .route("/api/admin/members/{id}", patch(members::update))
        .route("/api/admin/members/{id}/aliases", post(members::add_alias))
        .route(
            "/api/admin/members/{id}/aliases/{alias}",
            delete(members::remove_alias),
        )
}

pub const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// The session's attribution plus the request id from `Idempotency-Key`.
pub fn origin(session: &AdminSession, headers: &HeaderMap) -> Result<Origin, Refusal> {
    let mut origin = session.origin();
    let mut values = headers.get_all(IDEMPOTENCY_KEY).iter();
    match (values.next(), values.next()) {
        (None, _) => {}
        (Some(value), None) => {
            let key = value
                .to_str()
                .ok()
                .filter(|key| {
                    (1..=128).contains(&key.len())
                        && key.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric()
                                || matches!(byte, b'-' | b'_' | b'.' | b':')
                        })
                })
                .ok_or_else(|| {
                    Refusal::new(
                        axum::http::StatusCode::BAD_REQUEST,
                        "invalid_idempotency_key",
                        "Idempotency-Key is 1-128 letters, digits, '-', '_', '.' or ':'.",
                    )
                })?;
            origin.request_id = Some(key.to_owned());
        }
        (Some(_), Some(_)) => {
            return Err(Refusal::new(
                axum::http::StatusCode::BAD_REQUEST,
                "invalid_idempotency_key",
                "Send one Idempotency-Key.",
            ));
        }
    }
    Ok(origin)
}

pub fn state(site: &Site) -> Result<&Arc<ApiState>, Refusal> {
    super::context::state(site).map_err(Refusal::from)
}

/// The directory writes validate against: members plus watched channels.
pub async fn write_context(
    state: &ApiState,
) -> Result<(WriteContext, Vec<MemberProfile>), Refusal> {
    let profiles = state
        .store
        .members()
        .await
        .map_err(|error| Refusal::from(unavailable(error)))?;
    let mut directory = roster(&profiles);
    for channel in state.channels.channels() {
        if channel.watched {
            directory.watch(&channel.id);
        }
    }
    Ok((
        WriteContext {
            policy: state.policy.clone(),
            directory,
        },
        profiles,
    ))
}

/// A malformed or unknown-field JSON body.
pub fn bad_body<T>(_: T) -> Refusal {
    ApiError::INVALID_BODY.into()
}
