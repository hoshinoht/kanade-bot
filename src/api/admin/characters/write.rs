//! Character create, edit and delete. Each answer is known before the
//! write: the row as it will be stored (its version is checked inside the
//! transaction), so a keyed request stores exactly what it answered and a
//! retry replays it.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
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
            replay::{Keyed, mismatch, recall, replayed},
            write::{Body, Refusal, origin, state},
        },
        auth::AdminSession,
        dto::characters::{CharacterCreated, row},
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::{
        characters::{
            Character, CharacterChange, CharacterEdit, CharacterRecord, CharacterRole,
            CharacterWrite, DEFAULT_WORLD, new_role, parse_class, parse_ign, parse_level,
            parse_role, parse_world,
        },
        chardata::LINK_REGION,
        history::Actor,
        ids::{IdGenerator, RandomIds},
    },
    infrastructure::{
        character_data::AdminLinkOutcome,
        store::replays::{KeyedWrite, ReplayScope, StoredReplay},
    },
};

fn not_found() -> Refusal {
    Refusal::not_found("not_found", "That character is not on the roster.")
}

fn stale() -> Refusal {
    Refusal::conflict("stale", "The character changed since it was loaded.")
}

/// The answer to a write that did not go through.
fn refused(written: CharacterWrite) -> Refusal {
    match written {
        CharacterWrite::Stale(_) => stale(),
        CharacterWrite::IgnTaken => Refusal::unprocessable(
            "ign_taken",
            "That IGN is already on the roster in that world.",
        ),
        CharacterWrite::MainRequired => Refusal::conflict(
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

/// Write `change` with a keyed request's `replay` in the same transaction:
/// `None` once written now, else the answer a retry gets. `refused` is the
/// origin's answer to a write that did not go through.
pub(crate) async fn write_row(
    roster: &dyn CharacterRoster,
    change: CharacterChange,
    record: CharacterRecord,
    replay: Option<StoredReplay>,
    refused: fn(CharacterWrite) -> Refusal,
) -> Result<Option<Response>, Refusal> {
    match roster.write_character(change, record, replay).await {
        Ok(KeyedWrite::Applied(written)) if written.applied() => Ok(None),
        Ok(KeyedWrite::Applied(written)) => Err(refused(written)),
        Ok(KeyedWrite::Replayed(found)) => replayed(&found).map(Some),
        Ok(KeyedWrite::KeyTaken) => Err(mismatch()),
        Err(error) => Err(Refusal::unavailable(error)),
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
    Ok(write_row(roster, change, record, replay, refused)
        .await?
        .unwrap_or_else(|| (status, Json(body)).into_response()))
}

/// The member's characters, for the role rules.
async fn member_characters(
    roster: &dyn CharacterRoster,
    member_id: &str,
) -> Result<Vec<Character>, Refusal> {
    roster
        .member_characters(member_id.to_owned())
        .await
        .map_err(Refusal::unavailable)
}

async fn current(roster: &dyn CharacterRoster, id: &str) -> Result<Character, Refusal> {
    roster
        .character(id.to_owned())
        .await
        .map_err(Refusal::unavailable)?
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

/// A checked [`CharacterCreate`]; `role` is the one asked for, if any.
pub(crate) struct NewCharacter {
    pub(crate) ign: String,
    pub(crate) world: String,
    pub(crate) class: Option<String>,
    pub(crate) level: Option<u16>,
    pub(crate) role: Option<CharacterRole>,
}

impl CharacterCreate {
    /// Checked field by field in this order; the first bad one refuses.
    pub(crate) fn parse(self) -> Result<NewCharacter, Refusal> {
        Ok(NewCharacter {
            ign: parse_ign(&self.ign).map_err(invalid)?,
            world: parse_world(self.world.as_deref().unwrap_or(DEFAULT_WORLD)).map_err(invalid)?,
            class: parse_class(self.class.as_deref().unwrap_or_default()).map_err(invalid)?,
            level: self.level.map(parse_level).transpose().map_err(invalid)?,
            role: self
                .role
                .as_deref()
                .map(parse_role)
                .transpose()
                .map_err(invalid)?,
        })
    }
}

impl NewCharacter {
    /// The keyed request's identity: a create on `member_id`'s roster.
    pub(crate) fn request(&self, member_id: &str) -> String {
        format!(
            "character.create\u{1f}{member_id}\u{1f}{}",
            json!({
                "ign": self.ign, "world": self.world, "class": self.class, "level": self.level,
                "role": self.role.map(CharacterRole::as_str),
            })
        )
    }

    /// The new row of `member_id`, whose other characters are `others`.
    pub(crate) fn character(
        self,
        member_id: String,
        others: &[Character],
        now: DateTime<Utc>,
        by: Actor,
    ) -> Character {
        Character {
            id: RandomIds.new_id(),
            member_id,
            ign: self.ign,
            world: self.world,
            class: self.class,
            level: self.level,
            role: new_role(self.role, others),
            retired_at: None,
            created_at: now,
            updated_at: now,
            updated_by: by,
            version: 1,
        }
    }
}

pub async fn create(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(member_id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<CharacterCreate>,
) -> Reply {
    let state = state(&site)?;
    let fields = body.parse()?;
    let request = fields.request(&member_id);
    let (keyed, answered) = match origin(&session, &headers)?.request_id {
        None => (None, None),
        Some(key) => {
            let keyed = Keyed::new(ReplayScope::Config, &session, key, &request);
            match keyed.recall(state.store.as_ref(), state.now()).await? {
                Some(found) => (None, Some((keyed, found))),
                None => (Some(keyed), None),
            }
        }
    };
    if let Some((keyed, found)) = answered {
        return resume_link(state, &session, &keyed, found).await;
    }
    require_member(state, &member_id).await?;
    let others = member_characters(roster.as_ref(), &member_id).await?;
    let now = state.now();
    let character = fields.character(member_id, &others, now, session.origin().actor);
    let body = created(&character, None);
    let replay = keyed
        .as_ref()
        .map(|keyed| keyed.answered(StatusCode::CREATED, &body, now));
    if let Some(answer) = write_row(
        roster.as_ref(),
        CharacterChange::Create(character.clone()),
        record(&session, now),
        replay,
        refused,
    )
    .await?
    {
        return Ok(answer);
    }
    let Some(link) = ask_link(state, &session, &character).await else {
        return Ok((StatusCode::CREATED, Json(body)).into_response());
    };
    let body = created(&linked_row(roster.as_ref(), character).await, Some(link));
    if let Some(keyed) = keyed
        && state
            .store
            .put_replay(keyed.answered(StatusCode::CREATED, &body, now))
            .await
            .is_err()
    {
        // A retry finds the link on file and answers `linked` (resume_link).
        kanade_log::log!(warn, "idempotency_replay_unrecorded");
    }
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

fn created(character: &Character, link: Option<&'static str>) -> Value {
    json!(CharacterCreated {
        row: row(character),
        link,
    })
}

/// `character` as stored after its link's first fetch, which may have
/// copied NEXON's class and level into it (a failed read answers the row
/// as created; its next load shows the rest).
async fn linked_row(roster: &dyn CharacterRoster, character: Character) -> Character {
    roster
        .character(character.id.clone())
        .await
        .ok()
        .flatten()
        .unwrap_or(character)
}

/// A keyed create's stored answer, with its link outcome resumed: the
/// replay is first stored with `link: null` inside the roster write, so a
/// crash (or a failed replay update) before the link leaves it unanswered.
/// A link already on file means it was made (`linked`); none means it is
/// made now, once, and the replay updated.
async fn resume_link(
    state: &ApiState,
    session: &AdminSession,
    keyed: &Keyed,
    found: StoredReplay,
) -> Reply {
    let body: Value = serde_json::from_str(&found.body).map_err(Refusal::unavailable)?;
    let unanswered = found.status == StatusCode::CREATED.as_u16()
        && body.get("link").is_some_and(Value::is_null)
        && body["world"] == LINK_REGION
        && state
            .character_data
            .as_ref()
            .is_some_and(|module| module.connected());
    let id = body["id"].as_str().unwrap_or_default().to_owned();
    let (roster, character) = match (unanswered, state.characters.as_ref()) {
        (true, Some(roster)) => (roster, roster.character(id.clone()).await.ok().flatten()),
        _ => return replayed(&found),
    };
    let Some(character) = character else {
        return replayed(&found);
    };
    let module = state.character_data.as_ref().expect("checked connected");
    let made = module
        .status(&id)
        .await
        .is_ok_and(|view| view.link.is_some());
    let (character, link) = if made {
        (character, Some("linked"))
    } else {
        let link = ask_link(state, session, &character).await;
        (linked_row(roster.as_ref(), character).await, link)
    };
    let body = created(&character, link);
    let now = state.now();
    let _ = state
        .store
        .put_replay(keyed.answered(StatusCode::CREATED, &body, now))
        .await;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// With module `character_data` connected, a new MapleSEA character is
/// linked at once for its member, as Link… does (no member confirmation;
/// History row), and its first fetch runs. The character-data file is not
/// the roster's, so this follows the committed create: `failed` when the
/// link did not go through.
async fn ask_link(
    state: &ApiState,
    session: &AdminSession,
    character: &Character,
) -> Option<&'static str> {
    let module = state
        .character_data
        .as_ref()
        .filter(|module| module.connected())?;
    if character.world != LINK_REGION {
        return None;
    }
    let origin = session.origin();
    match module
        .admin_link(&character.id, 0, &origin.actor, origin.surface)
        .await
    {
        Ok(AdminLinkOutcome::Linked(..)) => Some("linked"),
        // Lost a race with a concurrent retry (stale / already linked): the
        // link on file is the answer, so every retry agrees.
        _ if module
            .status(&character.id)
            .await
            .is_ok_and(|view| view.link.is_some()) =>
        {
            Some("linked")
        }
        _ => {
            kanade_log::log!(warn, "chardata_create_link_failed");
            Some("failed")
        }
    }
}

/// A present JSON field, `null` included: `Some(None)` clears.
pub(crate) fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
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

impl CharacterPatch {
    /// The version the row was loaded at, then the edit, checked field by
    /// field in this order; the first bad one refuses.
    pub(crate) fn parse(self) -> Result<(u32, CharacterEdit), Refusal> {
        let version = self.version.ok_or_else(|| {
            Refusal::unprocessable(
                "version_required",
                "Send the version the character was loaded at.",
            )
        })?;
        let edit = CharacterEdit {
            ign: self
                .ign
                .as_deref()
                .map(parse_ign)
                .transpose()
                .map_err(invalid)?,
            world: self
                .world
                .as_deref()
                .map(parse_world)
                .transpose()
                .map_err(invalid)?,
            class: self
                .class
                .map(|class| parse_class(class.as_deref().unwrap_or_default()))
                .transpose()
                .map_err(invalid)?,
            level: self
                .level
                .map(|level| level.map(parse_level).transpose())
                .transpose()
                .map_err(invalid)?,
            role: self
                .role
                .as_deref()
                .map(parse_role)
                .transpose()
                .map_err(invalid)?,
            retired: self.retired,
        };
        Ok((version, edit))
    }
}

/// The keyed request's identity: `edit` of character `id` at `version`.
pub(crate) fn patch_request(id: &str, version: u32, edit: &CharacterEdit) -> String {
    format!(
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
    )
}

pub async fn update(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<CharacterPatch>,
) -> Reply {
    let state = state(&site)?;
    let (version, edit) = body.parse()?;
    let request = patch_request(&id, version, &edit);
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
