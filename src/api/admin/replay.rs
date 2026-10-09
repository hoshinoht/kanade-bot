//! `Idempotency-Key` replays for admin effects that are not scheduler commits
//! (Limits, rescan, Config), stored in the SQLite store so they survive a
//! restart. A key's identity is the SHA-256 of the normalized full request:
//! the same key and identity replays the stored answer without reapplying,
//! another identity is `422 idempotency_mismatch`, an expired key is new.
//! Each desk holds one lock across lookup, effect and record, so a concurrent
//! retry with the same key waits for the first request and replays it.

use axum::{Json, http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{context::unavailable, write::Refusal};
use crate::{
    api::{auth::AdminSession, auth::crypto::sha256_hex, state::ReadStore},
    infrastructure::store::replays::{ReplayScope, StoredReplay},
};

pub(super) fn mismatch() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "idempotency_mismatch",
        "That Idempotency-Key was already used for a different request.",
    )
}

/// One keyed request: its desk, sender, key and identity.
pub(super) struct Keyed {
    scope: ReplayScope,
    actor: String,
    key: String,
    request: String,
}

impl Keyed {
    /// `request` is the normalized full request; only its digest is kept.
    pub(super) fn new(
        scope: ReplayScope,
        session: &AdminSession,
        key: String,
        request: &str,
    ) -> Self {
        Self {
            scope,
            actor: format!("{}:{}", session.actor.kind(), session.actor.id()),
            key,
            request: sha256_hex(request.as_bytes()),
        }
    }

    pub(super) fn actor(&self) -> &str {
        &self.actor
    }

    pub(super) fn key(&self) -> &str {
        &self.key
    }

    /// The stored answer to this request, if the key was used for it;
    /// `idempotency_mismatch` when it was used for another.
    pub(super) fn check(
        &self,
        found: Option<StoredReplay>,
    ) -> Result<Option<StoredReplay>, Refusal> {
        match found {
            Some(found) if found.request != self.request => Err(mismatch()),
            found => Ok(found),
        }
    }

    /// [`Self::check`] against the API store.
    pub(super) async fn recall(
        &self,
        store: &dyn ReadStore,
        now: DateTime<Utc>,
    ) -> Result<Option<StoredReplay>, Refusal> {
        let found = store
            .replay(self.scope, self.actor.clone(), self.key.clone(), now)
            .await
            .map_err(unavailable)?;
        self.check(found)
    }

    /// The row recording `body` (answered with `status`) for this request.
    pub(super) fn answered(
        &self,
        status: StatusCode,
        body: &Value,
        now: DateTime<Utc>,
    ) -> StoredReplay {
        StoredReplay::new(
            self.scope,
            self.actor.clone(),
            self.key.clone(),
            self.request.clone(),
            status.as_u16(),
            body.to_string(),
            now,
        )
    }
}

/// The stored answer as it was sent: its status and JSON body.
pub(super) fn replayed(replay: &StoredReplay) -> Result<axum::response::Response, Refusal> {
    let status = StatusCode::from_u16(replay.status).map_err(|_| unavailable_replay())?;
    let body: Value = serde_json::from_str(&replay.body).map_err(|_| unavailable_replay())?;
    Ok((status, Json(body)).into_response())
}

fn unavailable_replay() -> Refusal {
    crate::api::error::ApiError::UNAVAILABLE.into()
}
