//! Character create, edit and delete. Each answer is known before the
//! write: the row as it will be stored (its version is checked inside the
//! transaction), so a keyed request stores exactly what it answered and a
//! retry replays it.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::{Reply, Roster, require_member};
use crate::{
    api::{
        admin::{
            replay::{Keyed, mismatch, replayed},
            write::{Refusal, bad_body, origin, state},
        },
        auth::AdminSession,
        dto::characters::row,
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::{
        characters::{
            Character, CharacterChange, CharacterEdit, CharacterRecord, CharacterRole,
            CharacterWrite, DEFAULT_WORLD, new_role, parse_class, parse_ign, parse_level,
            parse_role, parse_world,
        },
        ids::{IdGenerator, RandomIds},
    },
    infrastructure::store::replays::{KeyedWrite, ReplayScope},
};

fn not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "That character is not on the roster.",
    )
}

fn stale() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "stale",
        "The character changed since it was loaded.",
    )
}

/// The answer to a write that did not go through.
fn refused(written: CharacterWrite) -> Refusal {
    match written {
        CharacterWrite::Stale(_) => stale(),
        CharacterWrite::IgnTaken => Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ign_taken",
            "That IGN is already on the roster in that world.",
        ),
        CharacterWrite::MainRequired => Refusal::new(
            StatusCode::CONFLICT,
            "main_required",
            "A member with other active characters keeps a main: make another character the main first.",
        ),
        CharacterWrite::Missing
        | CharacterWrite::Created(_)
        | CharacterWrite::Updated(_)
        | CharacterWrite::Deleted(_) => not_found(),
    }
}

fn invalid(error: impl std::fmt::Display) -> Refusal {
    Refusal::invalid(error.to_string())
}

/// Who writes at `now`, for the History record.
pub(super) fn record(session: &AdminSession, now: DateTime<Utc>) -> CharacterRecord {
    let origin = session.origin();
    CharacterRecord {
        at: now,
        actor: origin.actor,
        surface: origin.surface,
    }
}

/// Write `change`, answering `status` with `body`, which a keyed request
/// stores as its replay in the same transaction.
async fn apply(
    roster: &dyn CharacterRoster,
    keyed: Option<Keyed>,
    change: CharacterChange,
    record: CharacterRecord,
    status: StatusCode,
    body: Value,
) -> Reply {
    let replay = keyed.map(|keyed| keyed.answered(status, &body, record.at));
    match roster.write_character(change, record, replay).await {
        Ok(KeyedWrite::Applied(written)) if written.applied() => {
            Ok((status, Json(body)).into_response())
        }
        Ok(KeyedWrite::Applied(written)) => Err(refused(written)),
        Ok(KeyedWrite::Replayed(found)) => replayed(&found),
        Ok(KeyedWrite::KeyTaken) => Err(mismatch()),
        Err(_) => Err(ApiError::UNAVAILABLE.into()),
    }
}

/// The request's key, if any, and its stored answer when it already has
/// one (`idempotency_mismatch` for another request under it).
pub(super) async fn recall(
    state: &ApiState,
    session: &AdminSession,
    headers: &HeaderMap,
    request: &str,
) -> Result<(Option<Keyed>, Option<Response>), Refusal> {
    let Some(key) = origin(session, headers)?.request_id else {
        return Ok((None, None));
    };
    let keyed = Keyed::new(ReplayScope::Config, session, key, request);
    let found = keyed.recall(state.store.as_ref(), state.now()).await?;
    Ok(match found {
        Some(found) => (None, Some(replayed(&found)?)),
        None => (Some(keyed), None),
    })
}

/// The member's characters, for the role rules.
async fn member_characters(
    roster: &dyn CharacterRoster,
    member_id: &str,
) -> Result<Vec<Character>, Refusal> {
    roster
        .member_characters(member_id.to_owned())
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))
}

async fn current(roster: &dyn CharacterRoster, id: &str) -> Result<Character, Refusal> {
    roster
        .character(id.to_owned())
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?
        .ok_or_else(not_found)
}

/// `CharacterCreate`: only `ign` is required; the world defaults to
/// `msea` and the role to `alt`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterCreate {
    ign: String,
    #[serde(default)]
    world: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    level: Option<i64>,
    #[serde(default)]
    role: Option<String>,
}

pub async fn create(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(member_id): UrlPath<String>,
    headers: HeaderMap,
    body: Result<Json<CharacterCreate>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let ign = parse_ign(&body.ign).map_err(invalid)?;
    let world = parse_world(body.world.as_deref().unwrap_or(DEFAULT_WORLD)).map_err(invalid)?;
    let class = parse_class(body.class.as_deref().unwrap_or_default()).map_err(invalid)?;
    let level = body.level.map(parse_level).transpose().map_err(invalid)?;
    let asked = body
        .role
        .as_deref()
        .map(parse_role)
        .transpose()
        .map_err(invalid)?;
    let request = format!(
        "character.create\u{1f}{member_id}\u{1f}{}",
        json!({
            "ign": ign, "world": world, "class": class, "level": level,
            "role": asked.map(CharacterRole::as_str),
        })
    );
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    require_member(state, &member_id).await?;
    let others = member_characters(roster.as_ref(), &member_id).await?;
    let now = state.now();
    let by = session.origin().actor;
    let character = Character {
        id: RandomIds.new_id(),
        member_id,
        ign,
        world,
        class,
        level,
        role: new_role(asked, &others),
        retired_at: None,
        created_at: now,
        updated_at: now,
        updated_by: by,
        version: 1,
    };
    let body = json!(row(&character));
    apply(
        roster.as_ref(),
        keyed,
        CharacterChange::Create(character),
        record(&session, now),
        StatusCode::CREATED,
        body,
    )
    .await
}

/// A present JSON field, `null` included: `Some(None)` clears.
pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `CharacterPatch`: `version` is required; an absent field is kept, a
/// `null` (or empty) `class`/`level` clears it, `retired` retires or restores.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterPatch {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    ign: Option<String>,
    #[serde(default)]
    world: Option<String>,
    #[serde(default, deserialize_with = "present")]
    class: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    level: Option<Option<i64>>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    retired: Option<bool>,
}

pub async fn update(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Result<Json<CharacterPatch>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let version = body.version.ok_or_else(|| {
        Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "version_required",
            "Send the version the character was loaded at.",
        )
    })?;
    let edit = CharacterEdit {
        ign: body
            .ign
            .as_deref()
            .map(parse_ign)
            .transpose()
            .map_err(invalid)?,
        world: body
            .world
            .as_deref()
            .map(parse_world)
            .transpose()
            .map_err(invalid)?,
        class: body
            .class
            .map(|class| parse_class(class.as_deref().unwrap_or_default()))
            .transpose()
            .map_err(invalid)?,
        level: body
            .level
            .map(|level| level.map(parse_level).transpose())
            .transpose()
            .map_err(invalid)?,
        role: body
            .role
            .as_deref()
            .map(parse_role)
            .transpose()
            .map_err(invalid)?,
        retired: body.retired,
    };
    let request = format!(
        "character.patch\u{1f}{id}\u{1f}{}",
        json!({
            "version": version,
            "ign": edit.ign,
            "world": edit.world,
            "class": edit.class,
            "level": edit.level,
            "role": edit.role.map(CharacterRole::as_str),
            "retired": edit.retired,
        })
    );
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let current = current(roster.as_ref(), &id).await?;
    if current.version != version {
        return Err(stale());
    }
    let mut others = member_characters(roster.as_ref(), &current.member_id).await?;
    others.retain(|other| other.id != current.id);
    let now = state.now();
    let after = current.edited(&edit, &others, now, &session.origin().actor);
    let body = json!(row(&after));
    let answer = apply(
        roster.as_ref(),
        keyed,
        CharacterChange::Update {
            version,
            after: after.clone(),
        },
        record(&session, now),
        StatusCode::OK,
        body,
    )
    .await?;
    unlink_changed(state, &session, &id, Some(&after)).await;
    Ok(answer)
}

/// An explicit delete: the row and its History stay apart (History keeps
/// the deleted fields). Applies to the row as read.
pub async fn delete(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Reply {
    let state = state(&site)?;
    let request = format!("character.delete\u{1f}{id}");
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let current = current(roster.as_ref(), &id).await?;
    let body = json!({ "message": format!("{} is deleted.", current.ign) });
    let answer = apply(
        roster.as_ref(),
        keyed,
        CharacterChange::Delete {
            id: id.clone(),
            version: current.version,
        },
        record(&session, state.now()),
        StatusCode::OK,
        body,
    )
    .await?;
    unlink_changed(state, &session, &id, None).await;
    Ok(answer)
}

/// After a committed roster write: a deleted character, or one whose IGN,
/// region or member changed, loses its character-data link at once.
async fn unlink_changed(
    state: &ApiState,
    session: &AdminSession,
    id: &str,
    after: Option<&Character>,
) {
    if let Some(module) = &state.character_data {
        let origin = session.origin();
        module
            .roster_changed(id, after, &origin.actor, origin.surface)
            .await;
    }
}
