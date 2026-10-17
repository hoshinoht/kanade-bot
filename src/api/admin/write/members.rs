//! Member edits (ping level, reply style, aliases, the tease opt-in and the
//! running-joke note). Members are not history rows: these go straight to
//! the member store as one portal-only write, which a concurrent gateway
//! update can never overwrite. The same write records an effective edit in
//! History beside the Config saves (section `members`) and stores an
//! `Idempotency-Key` replay (scope `config`): the same key and request
//! answers the member's current row without writing again; the key of
//! another live request is `422 idempotency_mismatch`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{Body, origin, refusal::Refusal, state};
use crate::{
    api::{
        admin::{
            context::{frames, weeks},
            replay::{Keyed, mismatch},
        },
        auth::AdminSession,
        dto,
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        members::{MemberProfile, PingLevel, PortalEdit},
        scheduler::StoreError,
        tease::{NoteProblem, note_problem},
    },
    infrastructure::store::replays::{PortalRecord, PortalWrite, ReplayScope},
};

type Reply = Result<Response, Refusal>;

async fn reply(state: &ApiState, profile: &MemberProfile) -> Reply {
    let [this, _] = frames(state, state.now()).map_err(Refusal::from)?;
    let snapshot = weeks(state, vec![this.start]).await?;
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
    Refusal::not_found("not_found", "That member is not on the roster.")
}

/// One edit, keyed by `request` (the normalized full request) when the
/// client sent an `Idempotency-Key`.
async fn apply(
    state: &ApiState,
    session: &AdminSession,
    headers: &HeaderMap,
    user_id: String,
    edit: PortalEdit,
    request: &str,
) -> Reply {
    let origin = origin(session, headers)?;
    let now = state.now();
    let replay = origin.request_id.map(|key| {
        Keyed::new(ReplayScope::Config, session, key, request).answered(
            StatusCode::OK,
            &json!({ "member": user_id }),
            now,
        )
    });
    let record = PortalRecord {
        at: now,
        actor: origin.actor,
        surface: origin.surface,
    };
    match state
        .store
        .edit_member_recorded(user_id.clone(), edit, record, replay)
        .await
    {
        Ok(PortalWrite::Edited(profile)) => reply(state, &profile).await,
        // As the first answer: the member's row as it is now.
        Ok(PortalWrite::Replayed) => match state.store.member(user_id).await {
            Ok(Some(profile)) => reply(state, &profile).await,
            Ok(None) => Err(not_found()),
            Err(_) => Err(ApiError::UNAVAILABLE.into()),
        },
        Ok(PortalWrite::Missing) => Err(not_found()),
        Ok(PortalWrite::KeyTaken) => Err(mismatch()),
        // The only constraint a portal edit can hit: the alias is held.
        Err(StoreError::Constraint(_)) => Err(Refusal::unprocessable(
            "alias_taken",
            "That alias already names someone.",
        )),
        Err(_) => Err(ApiError::UNAVAILABLE.into()),
    }
}

/// `MemberPatch`: `persona: ""` clears back to the default reply style and
/// `running_joke: ""` clears the note.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberPatch {
    #[serde(default)]
    ping_level: Option<String>,
    #[serde(default)]
    persona: Option<String>,
    #[serde(default)]
    tease_ok: Option<bool>,
    #[serde(default)]
    running_joke: Option<String>,
}

/// Every name a running-joke note must not carry (D10): each member's
/// display name, nickname and aliases, and with module `roster` every
/// character's IGN (retired ones too).
pub(crate) async fn roster_names(state: &ApiState) -> Result<Vec<String>, Refusal> {
    let profiles = state.store.members().await.map_err(Refusal::unavailable)?;
    let mut names = Vec::new();
    for profile in &profiles {
        let member = &profile.member;
        names.extend(member.display_name.iter().cloned());
        names.extend(member.nickname.iter().cloned());
        names.extend(profile.aliases.iter().cloned());
        if let Some(roster) = &state.characters {
            let characters = roster
                .member_characters(member.user_id.clone())
                .await
                .map_err(Refusal::unavailable)?;
            names.extend(characters.into_iter().map(|character| character.ign));
        }
    }
    Ok(names)
}

/// The note as stored (`None` clears it), or its `422`.
async fn running_joke(state: &ApiState, text: &str) -> Result<Option<String>, Refusal> {
    let note = text.trim();
    if note.is_empty() {
        return Ok(None);
    }
    let names = roster_names(state).await?;
    match note_problem(note, names.iter().map(String::as_str)) {
        None => Ok(Some(note.to_owned())),
        Some(NoteProblem::TooLong) => Err(Refusal::unprocessable(
            "note_too_long",
            "Keep the running joke to 80 characters.",
        )),
        Some(NoteProblem::NotPlain) => Err(Refusal::unprocessable(
            "note_not_plain",
            "Write the running joke as plain text: no mentions, @ or # handles, links or formatting.",
        )),
        Some(NoteProblem::NamesMember) => Err(Refusal::unprocessable(
            "note_names_member",
            "The running joke can't name anyone on the roster, by name, alias or IGN.",
        )),
    }
}

pub async fn update(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(user_id): UrlPath<String>,
    headers: HeaderMap,
    Body(patch): Body<MemberPatch>,
) -> Reply {
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
    let note = match patch.running_joke.as_deref() {
        None => None,
        Some(text) => Some(running_joke(state, text).await?),
    };
    let request = format!(
        "member.patch\u{1f}{user_id}\u{1f}{}",
        json!({
            "ping_level": patch.ping_level,
            "persona": patch.persona,
            "tease_ok": patch.tease_ok,
            "running_joke": patch.running_joke,
        })
    );
    apply(
        state,
        &session,
        &headers,
        user_id,
        PortalEdit {
            ping_level,
            reply_style,
            tease_ok: patch.tease_ok,
            running_joke: note,
            ..PortalEdit::default()
        },
        &request,
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
    session: AdminSession,
    UrlPath(user_id): UrlPath<String>,
    headers: HeaderMap,
    Body(request): Body<AliasRequest>,
) -> Reply {
    let state = state(&site)?;
    let alias = portal_alias(&request.alias)
        .ok_or_else(|| Refusal::invalid("An alias is one word: letters, digits, - or _."))?;
    let request = format!("member.alias.add\u{1f}{user_id}\u{1f}{alias}");
    apply(
        state,
        &session,
        &headers,
        user_id,
        PortalEdit {
            add_alias: Some(alias),
            ..PortalEdit::default()
        },
        &request,
    )
    .await
}

/// Idempotent: an alias the member does not hold leaves the row as it is.
/// Matched against any stored alias (v4 rows too), not the portal input rule.
pub async fn remove_alias(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((user_id, alias)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Reply {
    let state = state(&site)?;
    let alias = alias.trim().to_lowercase();
    let request = format!("member.alias.remove\u{1f}{user_id}\u{1f}{alias}");
    apply(
        state,
        &session,
        &headers,
        user_id,
        PortalEdit {
            remove_alias: Some(alias),
            ..PortalEdit::default()
        },
        &request,
    )
    .await
}
