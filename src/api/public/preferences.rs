//! The member's own preferences (D9): the "OK to be teased" opt-in they
//! set themselves, the admins' running-joke note (read-only here) and
//! whether teasing is switched on at all. The opt-in stays editable while
//! teasing is off, and needs no fresh sign-in. A write takes a member write
//! token and an `Idempotency-Key` (scope `config`, actor `member:<id>`) and
//! is recorded in History (section `members`, surface `public_portal`) in
//! the same transaction.

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::write::{admit, invalid_body};
use crate::api::{
    admin::{
        replay::{Keyed, mismatch},
        write::{Refusal, state},
    },
    auth::{audit::AuditContext, member::MemberSession},
    dto::public::MyPreferences,
    error::ApiError,
    listeners::Site,
    state::ApiState,
};
use kanade_domain::{
    history::{Actor, Surface},
    members::{MemberProfile, PortalEdit},
};
use kanade_store::replays::{PortalRecord, PortalWrite, ReplayScope};

type Reply = Result<Response, Refusal>;

/// `profile`'s preferences as the member sees them (a member with no row
/// yet has the defaults).
async fn answer(state: &ApiState, profile: Option<&MemberProfile>) -> Reply {
    let teasing_enabled = match &state.config {
        Some(desk) => desk.settings().await.notifications.teasing_enabled(),
        None => false,
    };
    Ok(Json(MyPreferences {
        tease_ok: profile.is_some_and(|profile| profile.tease_ok),
        running_joke: profile.and_then(|profile| profile.running_joke.clone()),
        teasing_enabled,
    })
    .into_response())
}

/// `GET /api/public/me/preferences` → `MyPreferences`.
pub(super) async fn read(State(site): State<Arc<Site>>, session: MemberSession) -> Reply {
    let state = state(&site)?;
    let profile = state
        .store
        .member(session.user_id.clone())
        .await
        .map_err(Refusal::unavailable)?;
    answer(state, profile.as_ref()).await
}

/// `MyPreferencesPatch`: only the opt-in; the note is the admins'.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreferencesPatch {
    tease_ok: bool,
}

/// `PATCH /api/public/me/preferences` (`MyPreferencesPatch`) →
/// `MyPreferences`. The same key and request answers the preferences as
/// they are now without writing again; another request under the key is
/// `422 idempotency_mismatch`.
pub(super) async fn update(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    body: Result<Json<PreferencesPatch>, JsonRejection>,
) -> Reply {
    let (_, key) = admit(&site, &audit, &session, &headers)?;
    let Ok(Json(body)) = body else {
        return Err(invalid_body());
    };
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let request = format!(
        "me.preferences\u{1f}{me}\u{1f}{}",
        json!({ "tease_ok": body.tease_ok })
    );
    let now = state.now();
    let replay = Keyed::member(ReplayScope::Config, me, key, &request).answered(
        StatusCode::OK,
        &json!({ "member": me }),
        now,
    );
    let record = PortalRecord {
        at: now,
        actor: Actor::member(me),
        surface: Surface::PublicPortal,
    };
    let edit = PortalEdit {
        tease_ok: Some(body.tease_ok),
        ..PortalEdit::default()
    };
    match state
        .store
        .edit_member_recorded(me.to_owned(), edit, record, Some(replay))
        .await
    {
        Ok(PortalWrite::Edited(profile)) => answer(state, Some(&profile)).await,
        // As the first answer: the preferences as they are now.
        Ok(PortalWrite::Replayed) => match state.store.member(me.to_owned()).await {
            Ok(profile) => answer(state, profile.as_ref()).await,
            Err(error) => Err(Refusal::unavailable(error)),
        },
        Ok(PortalWrite::Missing) => Err(ApiError::NOT_FOUND.into()),
        Ok(PortalWrite::KeyTaken) => Err(mismatch()),
        Err(error) => Err(Refusal::unavailable(error)),
    }
}
