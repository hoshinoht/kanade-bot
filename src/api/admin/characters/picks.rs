//! What run picks (`runs.rs`) and timing picks (`fixed.rs`) share: the
//! `CharacterPick` body, the refusals, the character a set names (checked
//! before the write, so the answer is known and a keyed request stores
//! exactly what it answered) and the keyed write itself.

use axum::{Json, http::StatusCode, response::IntoResponse};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::{Reply, write::present};
use crate::{
    api::{
        admin::{
            replay::{Keyed, mismatch, replayed},
            write::Refusal,
        },
        error::ApiError,
        state::CharacterRoster,
    },
    domain::{
        characters::{Character, CharacterRecord, Pick, PickChange, PickScope, PickWrite},
        history::Actor,
    },
    infrastructure::store::replays::KeyedWrite,
};

pub(super) fn unavailable<T>(_: T) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

/// What the slot is on, for the stale message.
fn on(scope: PickScope) -> &'static str {
    match scope {
        PickScope::Run => "the run",
        PickScope::Timing => "this timing",
    }
}

pub(super) fn stale(scope: PickScope) -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "stale",
        format!(
            "This member's character on {} changed since it was loaded.",
            on(scope)
        ),
    )
}

fn character_not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "That character is not on the roster.",
    )
}

fn not_theirs() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "not_their_character",
        "That character belongs to another member.",
    )
}

fn retired() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "character_retired",
        "That character is retired: bring it back on the member's Characters tab first.",
    )
}

/// The answer to a pick that did not go through.
fn refused(written: PickWrite, scope: PickScope) -> Refusal {
    match written {
        PickWrite::Stale(_) => stale(scope),
        PickWrite::NotTheirs => not_theirs(),
        PickWrite::Retired => retired(),
        PickWrite::UnknownCharacter | PickWrite::Picked(_) | PickWrite::Cleared => {
            character_not_found()
        }
    }
}

/// `CharacterPick`: `character_id` (a character id, or `null` to clear)
/// and the slot `version` that was read are both required.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterPick {
    #[serde(default, deserialize_with = "present")]
    character_id: Option<Option<String>>,
    #[serde(default)]
    version: Option<u32>,
}

impl CharacterPick {
    /// The character (`None` clears) and the version, both required.
    pub(super) fn parts(self, loaded: &str) -> Result<(Option<String>, u32), Refusal> {
        let version = self.version.ok_or_else(|| {
            Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "version_required",
                format!("Send the version the {loaded} characters were loaded at."),
            )
        })?;
        let character_id = self
            .character_id
            .ok_or_else(|| Refusal::invalid("Send character_id, or null to clear the pick."))?;
        Ok((character_id, version))
    }
}

/// The character a set names: on the roster, the member's and active.
pub(super) async fn named(
    roster: &dyn CharacterRoster,
    character_id: &str,
    member_id: &str,
) -> Result<Character, Refusal> {
    let character = roster
        .character(character_id.to_owned())
        .await
        .map_err(unavailable)?
        .ok_or_else(character_not_found)?;
    if character.member_id != member_id {
        return Err(not_theirs());
    }
    if !character.is_active() {
        return Err(retired());
    }
    Ok(character)
}

/// One slot write, planned against `picks` (the loaded rows, changed in
/// place to how the write leaves them, for the answer).
pub(super) struct SlotWrite<'a> {
    pub scope: PickScope,
    pub target: &'a str,
    pub member_id: &'a str,
    pub version: u32,
    pub at: DateTime<Utc>,
    pub actor: &'a Actor,
}

impl SlotWrite<'_> {
    fn here(&self, pick: &Pick) -> bool {
        pick.scope == self.scope && pick.target == self.target && pick.member_id == self.member_id
    }

    /// The stored row of the slot (whatever it names), for its version.
    pub(super) fn current<'p>(&self, picks: &'p [Pick]) -> Option<&'p Pick> {
        picks.iter().find(|pick| self.here(pick))
    }

    /// Leave `picks` as the write will (`character_id` set, or cleared) and
    /// return the change to send.
    pub(super) fn plan(&self, picks: &mut Vec<Pick>, character_id: Option<String>) -> PickChange {
        let current = self.current(picks).cloned();
        picks.retain(|pick| !self.here(pick));
        match character_id {
            Some(character_id) => {
                picks.push(match current {
                    Some(current) if current.character_id == character_id => current,
                    _ => Pick {
                        scope: self.scope,
                        target: self.target.to_owned(),
                        member_id: self.member_id.to_owned(),
                        character_id: character_id.clone(),
                        picked_at: self.at,
                        picked_by: self.actor.clone(),
                        version: self.version + 1,
                    },
                });
                PickChange::Set {
                    scope: self.scope,
                    target: self.target.to_owned(),
                    member_id: self.member_id.to_owned(),
                    character_id,
                    version: self.version,
                }
            }
            None => PickChange::Clear {
                scope: self.scope,
                target: self.target.to_owned(),
                member_id: self.member_id.to_owned(),
                version: self.version,
            },
        }
    }
}

/// Write `change`, answering `body`, which a keyed request stores as its
/// replay in the same transaction.
pub(super) async fn apply(
    roster: &dyn CharacterRoster,
    keyed: Option<Keyed>,
    change: PickChange,
    record: CharacterRecord,
    body: Value,
) -> Reply {
    let scope = change.slot().0;
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::OK, &body, record.at));
    match roster.write_pick(change, record, replay).await {
        Ok(KeyedWrite::Applied(written)) if written.applied() => Ok(Json(body).into_response()),
        Ok(KeyedWrite::Applied(written)) => Err(refused(written, scope)),
        Ok(KeyedWrite::Replayed(found)) => replayed(&found),
        Ok(KeyedWrite::KeyTaken) => Err(mismatch()),
        Err(_) => Err(ApiError::UNAVAILABLE.into()),
    }
}
