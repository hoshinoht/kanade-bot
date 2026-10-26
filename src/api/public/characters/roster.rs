//! The member's own roster rows: list (with each link and the admins'
//! requests waiting for them), add and edit (retire and Make main
//! included). The admin rules apply unchanged (`domain::characters`); there
//! is no delete. Writes replay their `Idempotency-Key` (scope `config`,
//! actor `member:<id>`) in the roster write's transaction.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{
        Path as UrlPath, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{module, own, roster};
use crate::{
    api::{
        admin::{
            characters::{CharacterCreate, CharacterPatch, patch_request, write_row},
            replay::{Keyed, replayed},
            write::{Refusal, state},
        },
        auth::{audit::AuditContext, member::MemberSession},
        dto::{
            character_data::link,
            characters::row,
            history::SettingsActor,
            iso_instant,
            public::{MyCharacterCreated, MyCharacters, PendingLink},
        },
        listeners::Site,
        public::write::{admit, invalid_body, path_id},
    },
    infrastructure::store::replays::ReplayScope,
};
use kanade_domain::{
    characters::{Character, CharacterChange, CharacterRecord, CharacterWrite},
    chardata::{LINK_REGION, Link, LinkStatus, NOTICE_VERSION, PENDING_TTL},
    history::{Actor, Surface},
    members::MemberProfile,
};

type Reply = Result<Response, Refusal>;

/// The answer to a roster write that did not go through: the admin
/// routes' codes, and the generic `404` for a row gone meanwhile.
fn refused(written: CharacterWrite) -> Refusal {
    match written {
        CharacterWrite::Stale(_) => {
            Refusal::conflict("stale", "The character changed since it was loaded.")
        }
        CharacterWrite::IgnTaken => Refusal::unprocessable(
            "ign_taken",
            "That IGN is already on the roster in that world.",
        ),
        CharacterWrite::MainRequired => Refusal::conflict(
            "main_required",
            "Your main stays while you have other active characters: make another character your main first.",
        ),
        CharacterWrite::Missing
        | CharacterWrite::Created(_)
        | CharacterWrite::Updated(_)
        | CharacterWrite::Deleted(_) => crate::api::error::ApiError::NOT_FOUND.into(),
    }
}

/// The member through the portal, for the History row.
fn record(me: &str, at: DateTime<Utc>) -> CharacterRecord {
    CharacterRecord {
        at,
        actor: Actor::member(me),
        surface: Surface::PublicPortal,
    }
}

/// The name of the admin who asked for `link`, when they signed in with
/// Discord and have a member row (as `/character link` names them).
pub(super) fn requester(link: &Link, profiles: &[MemberProfile]) -> Option<String> {
    if link.requested_by.kind() != "admin" {
        return None;
    }
    let user = link.requested_by.id().strip_prefix("discord:")?;
    profiles
        .iter()
        .find(|profile| profile.member.user_id == user)?
        .member
        .name()
        .map(str::to_owned)
}

/// A pending link on `character`, waiting for its member.
pub(super) fn pending(
    character: &Character,
    link: &Link,
    profiles: &[MemberProfile],
) -> PendingLink {
    PendingLink {
        character_id: character.id.clone(),
        ign: character.ign.clone(),
        class: character.class.clone(),
        requested_by: SettingsActor {
            kind: link.requested_by.kind(),
            id: link.requested_by.id().to_owned(),
        },
        requested_by_name: requester(link, profiles),
        requested_at: iso_instant(link.requested_at),
        expires_at: iso_instant(link.requested_at + PENDING_TTL),
    }
}

/// `GET /api/public/me/characters`.
pub(super) async fn list(State(site): State<Arc<Site>>, session: MemberSession) -> Reply {
    let state = state(&site)?;
    let characters = roster(state)?
        .member_characters(session.user_id.clone())
        .await
        .map_err(Refusal::unavailable)?;
    let module = state.character_data.as_ref();
    let mut links = BTreeMap::new();
    let mut waiting = Vec::new();
    if let Some(module) = module {
        let profiles = state.store.members().await.map_err(Refusal::unavailable)?;
        for character in &characters {
            let view = module
                .view(&character.id)
                .await
                .map_err(Refusal::unavailable)?;
            if let Some(found) = view
                .link
                .as_ref()
                .filter(|found| found.status == LinkStatus::Pending)
            {
                waiting.push(pending(character, found, &profiles));
            }
            links.insert(
                character.id.clone(),
                link(&character.id, &view, module.privacy_url()),
            );
        }
    }
    waiting.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
    Ok(Json(MyCharacters {
        characters: characters.iter().map(row).collect(),
        links,
        pending: waiting,
        character_data: module.is_some(),
        connected: module.is_some_and(|module| module.connected()),
        estimates: module.is_some_and(|module| module.estimates().is_some()),
        notice_version: NOTICE_VERSION,
        privacy_url: module.and_then(|module| module.privacy_url().map(str::to_owned)),
    })
    .into_response())
}

/// `POST /api/public/me/characters` (`MyCharacterCreate`, as the admin's
/// `CharacterCreate`) → `201 MyCharacterCreated`. Class and level are
/// required unless NEXON can be asked (a MapleSEA character with NEXON
/// connected). No link request is made: the notice opens next on the
/// portal (`linkable`).
pub(super) async fn create(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    body: Result<Json<CharacterCreate>, JsonRejection>,
) -> Reply {
    let (member, key) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let Ok(Json(body)) = body else {
        return Err(invalid_body());
    };
    let state = state(&site)?;
    let roster = roster(state)?;
    let me = session.user_id.as_str();
    let fields = body.parse()?;
    let linkable = fields.world == LINK_REGION
        && state
            .character_data
            .as_ref()
            .is_some_and(|module| module.connected());
    if !linkable && (fields.class.is_none() || fields.level.is_none()) {
        return Err(Refusal::unprocessable(
            "class_level_required",
            "Give the class and level: NEXON can't be asked for this character.",
        ));
    }
    let request = fields.request(me);
    let keyed = Keyed::member(ReplayScope::Config, me, key, &request);
    let now = state.now();
    if let Some(found) = keyed.recall(state.store.as_ref(), now).await? {
        return replayed(&found);
    }
    let others = roster
        .member_characters(me.to_owned())
        .await
        .map_err(Refusal::unavailable)?;
    let character = fields.character(me.to_owned(), &others, now, Actor::member(me));
    let body = json!(MyCharacterCreated {
        row: row(&character),
        linkable,
    });
    let replay = keyed.answered(StatusCode::CREATED, &body, now);
    if let Some(answer) = write_row(
        roster.as_ref(),
        CharacterChange::Create(character),
        record(me, now),
        Some(replay),
        refused,
    )
    .await?
    {
        return Ok(answer);
    }
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// `PATCH /api/public/me/characters/{id}` (`MyCharacterPatch`, as the
/// admin's `CharacterPatch`) → the row as stored.
pub(super) async fn update(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<CharacterPatch>, JsonRejection>,
) -> Reply {
    let (member, key) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let id = path_id(path)?;
    let Ok(Json(body)) = body else {
        return Err(invalid_body());
    };
    let state = state(&site)?;
    let roster = roster(state)?;
    let me = session.user_id.as_str();
    let (version, edit) = body.parse()?;
    let request = patch_request(&id, version, &edit);
    let keyed = Keyed::member(ReplayScope::Config, me, key, &request);
    let now = state.now();
    if let Some(found) = keyed.recall(state.store.as_ref(), now).await? {
        return replayed(&found);
    }
    let current = own(roster.as_ref(), me, &id).await?;
    if current.version != version {
        return Err(refused(CharacterWrite::Stale(current)));
    }
    let mut others = roster
        .member_characters(me.to_owned())
        .await
        .map_err(Refusal::unavailable)?;
    others.retain(|other| other.id != current.id);
    let actor = Actor::member(me);
    let after = current.edited(&edit, &others, now, &actor);
    let body: Value = json!(row(&after));
    let replay = keyed.answered(StatusCode::OK, &body, now);
    let answer = write_row(
        roster.as_ref(),
        CharacterChange::Update {
            version,
            after: after.clone(),
        },
        record(me, now),
        Some(replay),
        refused,
    )
    .await?;
    if let Some(answer) = answer {
        return Ok(answer);
    }
    // A new IGN, region or member loses the NEXON link at once.
    if let Ok(module) = module(state) {
        module
            .roster_changed(&id, Some(&after), &actor, Surface::PublicPortal)
            .await;
    }
    Ok(Json(body).into_response())
}
