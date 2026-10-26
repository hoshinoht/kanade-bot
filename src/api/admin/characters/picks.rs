//! What run picks (`runs.rs`) and timing picks (`fixed.rs`) share: the
//! `CharacterPick` and `SameForEveryBoss` bodies, the refusals, the boss a
//! per-boss write names, the character a set names (checked before the
//! write, so the answer is known and a keyed request stores exactly what
//! it answered) and the keyed write itself.

use std::collections::BTreeMap;

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
    },
    app::read::CharacterRoster,
    infrastructure::store::replays::KeyedWrite,
};
use kanade_domain::{
    catalog::BossTable,
    characters::{Character, CharacterRecord, PartySlot, Pick, PickChange, PickScope, PickWrite},
    history::Actor,
};

/// What the slot is on, for the stale message.
fn on(scope: PickScope) -> &'static str {
    match scope {
        PickScope::Run => "the run",
        PickScope::Timing => "this timing",
    }
}

pub(super) fn stale(scope: PickScope) -> Refusal {
    Refusal::conflict(
        "stale",
        format!(
            "This member's character on {} changed since it was loaded.",
            on(scope)
        ),
    )
}

fn character_not_found() -> Refusal {
    Refusal::not_found("not_found", "That character is not on the roster.")
}

fn not_theirs() -> Refusal {
    Refusal::unprocessable(
        "not_their_character",
        "That character belongs to another member.",
    )
}

fn retired() -> Refusal {
    Refusal::unprocessable(
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
            Refusal::unprocessable(
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

/// `SameForEveryBoss`: `versions`, each later boss's `version` as read
/// (by its catalog `key`; a boss left out counts as 0).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SameForEveryBoss {
    #[serde(default)]
    versions: Option<BTreeMap<String, u32>>,
}

impl SameForEveryBoss {
    /// The versions, required.
    pub(super) fn versions(self, loaded: &str) -> Result<BTreeMap<String, u32>, Refusal> {
        self.versions.ok_or_else(|| {
            Refusal::unprocessable(
                "version_required",
                format!("Send the versions the {loaded} characters were loaded at."),
            )
        })
    }
}

/// The catalog boss `asked` names among `tokens` (a token of the run or
/// timing, or its catalog boss) when it is a later boss: the first boss's
/// character is the member's own pick, never a per-boss one.
pub(super) fn later_boss(
    catalog: &BossTable,
    tokens: &[String],
    asked: &str,
    scope: PickScope,
) -> Result<String, Refusal> {
    let key = |token: &str| {
        catalog
            .split(token)
            .map_or_else(|| token.to_owned(), |(_, boss)| boss.short().to_owned())
    };
    let first = tokens.first().map(|token| key(token));
    let Some(boss) = tokens
        .iter()
        .map(|token| (token, key(token)))
        .find(|(token, boss)| *token == asked || boss == asked)
        .map(|(_, boss)| boss)
    else {
        return Err(Refusal::unprocessable(
            match scope {
                PickScope::Run => "boss_not_on_run",
                PickScope::Timing => "boss_not_on_timing",
            },
            format!("That boss is not on {}.", on(scope)),
        ));
    };
    if first.as_ref() == Some(&boss) {
        return Err(Refusal::unprocessable(
            "first_boss",
            "The first boss brings the member's own pick: pick it on their row.",
        ));
    }
    Ok(boss)
}

/// Every later boss's stored per-boss version on `slot` as read now (by
/// catalog boss), and whether `sent` (the versions a "same for every boss"
/// was loaded at, absent = 0) matches it. A nonzero version sent for a boss
/// that is no longer a later boss is stale too: the form was loaded before
/// that boss left.
pub(super) fn later_versions(
    slot: &PartySlot,
    sent: &BTreeMap<String, u32>,
) -> (BTreeMap<String, u32>, bool) {
    let first = slot.bosses.first().map(|boss| boss.boss.as_str());
    let now: BTreeMap<String, u32> = slot
        .bosses
        .iter()
        .skip(1)
        .filter(|boss| Some(boss.boss.as_str()) != first)
        .map(|boss| (boss.boss.clone(), boss.version))
        .collect();
    let fresh = now
        .iter()
        .all(|(boss, version)| sent.get(boss).copied().unwrap_or(0) == *version)
        && sent
            .iter()
            .all(|(boss, version)| *version == 0 || now.contains_key(boss));
    (now, fresh)
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
        .map_err(Refusal::unavailable)?
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
/// place to how the write leaves them, for the answer). `boss` names a
/// per-boss slot (a later boss's catalog boss).
pub(super) struct SlotWrite<'a> {
    pub scope: PickScope,
    pub target: &'a str,
    pub member_id: &'a str,
    pub boss: Option<&'a str>,
    pub version: u32,
    pub at: DateTime<Utc>,
    pub actor: &'a Actor,
}

impl SlotWrite<'_> {
    fn here(&self, pick: &Pick) -> bool {
        pick.scope == self.scope
            && pick.target == self.target
            && pick.member_id == self.member_id
            && pick.boss.as_deref() == self.boss
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
        let boss = self.boss.map(str::to_owned);
        match character_id {
            Some(character_id) => {
                picks.push(match current {
                    Some(current) if current.character_id == character_id => current,
                    _ => Pick {
                        scope: self.scope,
                        target: self.target.to_owned(),
                        member_id: self.member_id.to_owned(),
                        boss: boss.clone(),
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
                    boss,
                    character_id,
                    version: self.version,
                }
            }
            None => PickChange::Clear {
                scope: self.scope,
                target: self.target.to_owned(),
                member_id: self.member_id.to_owned(),
                boss,
                version: self.version,
            },
        }
    }
}

/// Leave `picks` as "same for every boss" will (`member`'s per-boss rows
/// on `target` for `versions`' bosses gone) and return the change to send.
pub(super) fn plan_same(
    picks: &mut Vec<Pick>,
    scope: PickScope,
    target: &str,
    member_id: &str,
    versions: BTreeMap<String, u32>,
) -> PickChange {
    picks.retain(|pick| {
        !(pick.scope == scope
            && pick.target == target
            && pick.member_id == member_id
            && pick
                .boss
                .as_ref()
                .is_some_and(|boss| versions.contains_key(boss)))
    });
    PickChange::Same {
        scope,
        target: target.to_owned(),
        member_id: member_id.to_owned(),
        versions,
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
