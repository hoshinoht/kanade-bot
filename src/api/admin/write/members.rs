//! Member edits (ping level, reply style, aliases). Members are not history
//! rows: these go straight to the member store as one portal-only write,
//! which a concurrent gateway update can never overwrite.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::{bad_body, refusal::Refusal, state};
use crate::{
    api::{
        admin::context::frames, auth::AdminSession, dto, error::ApiError, listeners::Site,
        state::ApiState,
    },
    domain::{
        members::{MemberProfile, PingLevel, PortalEdit},
        scheduler::{Scope, StoreError},
    },
};

type Reply = Result<Response, Refusal>;

async fn reply(state: &ApiState, profile: &MemberProfile) -> Reply {
    let [this, _] = frames(state, state.now()).map_err(Refusal::from)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start]))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    let personas = state.profile_options();
    Ok(Json(dto::members::row(
        profile,
        &state.access,
        &personas,
        &snapshot,
        this.start,
    ))
    .into_response())
}

fn not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "That member is not on the roster.",
    )
}

async fn apply(state: &ApiState, user_id: String, edit: PortalEdit) -> Reply {
    match state.store.edit_member(user_id, edit).await {
        Ok(Some(profile)) => reply(state, &profile).await,
        Ok(None) => Err(not_found()),
        // The only constraint a portal edit can hit: the alias is held.
        Err(StoreError::Constraint(_)) => Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "alias_taken",
            "That alias already names someone.",
        )),
        Err(_) => Err(ApiError::UNAVAILABLE.into()),
    }
}

/// `MemberPatch`: `persona: ""` clears back to the default reply style.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberPatch {
    #[serde(default)]
    ping_level: Option<String>,
    #[serde(default)]
    persona: Option<String>,
}

pub async fn update(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(user_id): UrlPath<String>,
    body: Result<Json<MemberPatch>, JsonRejection>,
) -> Reply {
    let Json(patch) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let personas = state.profile_options();
    let ping_level = patch
        .ping_level
        .as_deref()
        .map(|level| {
            PingLevel::parse_stored(level)
                .map_err(|_| Refusal::invalid("Ping level is essential, all or off."))
        })
        .transpose()?;
    let reply_style = match patch.persona.as_deref() {
        None => None,
        Some("" | "default") => Some(None),
        Some(key) if personas.iter().any(|persona| persona.key == key) => {
            Some(Some(key.to_owned()))
        }
        Some(_) => return Err(Refusal::invalid("No such reply style.")),
    };
    apply(
        state,
        user_id,
        PortalEdit {
            ping_level,
            reply_style,
            ..PortalEdit::default()
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AliasRequest {
    alias: String,
}

/// The portal's input rule is stricter than what the store accepts (v4 rows).
fn portal_alias(text: &str) -> Option<String> {
    let alias = text.trim().to_lowercase();
    ((1..=32).contains(&alias.chars().count())
        && alias
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'))
    .then_some(alias)
}

pub async fn add_alias(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(user_id): UrlPath<String>,
    body: Result<Json<AliasRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let alias = portal_alias(&request.alias)
        .ok_or_else(|| Refusal::invalid("An alias is one word: letters, digits, - or _."))?;
    apply(
        state,
        user_id,
        PortalEdit {
            add_alias: Some(alias),
            ..PortalEdit::default()
        },
    )
    .await
}

/// Idempotent: an alias the member does not hold leaves the row as it is.
/// Matched against any stored alias (v4 rows too), not the portal input rule.
pub async fn remove_alias(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath((user_id, alias)): UrlPath<(String, String)>,
) -> Reply {
    let state = state(&site)?;
    apply(
        state,
        user_id,
        PortalEdit {
            remove_alias: Some(alias.trim().to_lowercase()),
            ..PortalEdit::default()
        },
    )
    .await
}
