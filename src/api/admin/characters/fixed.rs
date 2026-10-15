//! The character each party member brings to a weekly timing (modules
//! `roster` and `character_data`): a read of the timing's slots and a
//! write that sets or clears one member's pick for it. Runs made from the
//! timing read the pick live (a run's own pick still wins). Reads never
//! write: the default (main, else the only character) is resolved each
//! time. A pick saves directly, outside the timing's own form and the
//! history chain, with its History row (key `fixedpick.<fixed>.<member>`)
//! and keyed replay in one transaction. Both routes answer the generic
//! `404` before the session is checked while either module is off.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{FromRequestParts, Path as UrlPath, State},
    http::{HeaderMap, request::Parts},
    response::IntoResponse,
};
use serde_json::json;

use super::{
    super::replay::recall,
    Reply,
    picks::{CharacterPick, SlotWrite, apply, named, stale},
    write::record,
};
use crate::{
    api::{
        admin::{
            context::weeks,
            write::{Body, Refusal, state},
        },
        auth::AdminSession,
        dto::characters::{TimingCharacters, slot},
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::{
        characters::{
            Character, PartySlot, Pick, PickScope, StoredPicks, pick_version, timing_slots,
        },
        schedule::FixedRun,
    },
};

/// The roster's store while both modules are on; the generic `404`
/// otherwise. Taken first, so an off module answers as an unmounted path
/// whoever asks.
pub(in crate::api::admin) struct Both(pub Arc<dyn CharacterRoster>);

impl FromRequestParts<Arc<Site>> for Both {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        site.state
            .as_ref()
            .filter(|state| state.character_data.is_some())
            .and_then(|state| state.characters.clone())
            .map(Self)
            .ok_or(ApiError::NOT_FOUND)
    }
}

fn timing_not_found() -> Refusal {
    Refusal::not_found("not_found", "That weekly timing no longer exists.")
}

/// What a timing's slots are computed from: every weekly timing (the one
/// asked for among them), the party's characters and the timing's picks.
pub(in crate::api::admin) struct Timing {
    pub fixed: Option<FixedRun>,
    timings: Vec<FixedRun>,
    characters: BTreeMap<String, Vec<Character>>,
    picks: Vec<Pick>,
}

impl Timing {
    /// `fixed_id`'s timing (`404 not_found` when it is gone, or `None` for
    /// one not added yet) and the characters of `party` (its saved party
    /// when `None`).
    pub(in crate::api::admin) async fn load(
        state: &ApiState,
        roster: &dyn CharacterRoster,
        fixed_id: Option<&str>,
        party: Option<&[String]>,
    ) -> Result<Self, Refusal> {
        let timings = weeks(state, Vec::new()).await?.fixed_runs;
        let fixed = match fixed_id {
            Some(id) => Some(
                timings
                    .iter()
                    .find(|fixed| fixed.id == id)
                    .cloned()
                    .ok_or_else(timing_not_found)?,
            ),
            None => None,
        };
        let party = party
            .or(fixed.as_ref().map(|fixed| fixed.participants.as_slice()))
            .unwrap_or_default();
        let mut characters = BTreeMap::new();
        for member in party {
            if characters.contains_key(member) {
                continue;
            }
            let rows = roster
                .member_characters(member.clone())
                .await
                .map_err(Refusal::unavailable)?;
            characters.insert(member.clone(), rows);
        }
        let picks = match &fixed {
            Some(fixed) => roster
                .picks(PickScope::Timing, vec![fixed.id.clone()])
                .await
                .map_err(Refusal::unavailable)?,
            None => Vec::new(),
        };
        Ok(Self {
            fixed,
            timings,
            characters,
            picks,
        })
    }

    /// The slots of `party` in its order (only the saved party's members
    /// have picks).
    pub(in crate::api::admin) fn slots(&self, party: &[String]) -> Vec<PartySlot> {
        timing_slots(
            self.fixed.as_ref().map(|fixed| fixed.id.as_str()),
            party,
            &self.characters,
            &StoredPicks::new(&self.picks, &self.timings),
        )
    }

    fn answer(&self, fixed: &FixedRun) -> TimingCharacters {
        TimingCharacters {
            fixed_run_id: fixed.id.clone(),
            slots: self.slots(&fixed.participants).iter().map(slot).collect(),
        }
    }
}

/// `GET /api/admin/fixed/{id}/characters`.
pub async fn read(
    Both(roster): Both,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(fixed_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let timing = Timing::load(state, roster.as_ref(), Some(&fixed_id), None).await?;
    let fixed = timing.fixed.as_ref().ok_or_else(timing_not_found)?;
    Ok(Json(timing.answer(fixed)).into_response())
}

/// `PUT /api/admin/fixed/{id}/characters/{member}`: set or clear one party
/// member's pick for the timing. The answer is that member's slot as it
/// now reads.
pub async fn pick(
    Both(roster): Both,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((fixed_id, member_id)): UrlPath<(String, String)>,
    headers: HeaderMap,
    Body(body): Body<CharacterPick>,
) -> Reply {
    let state = state(&site)?;
    let (character_id, version) = body.parts("timing's")?;
    let request = format!(
        "character.fixedpick\u{1f}{fixed_id}\u{1f}{member_id}\u{1f}{}",
        json!({ "character_id": character_id, "version": version })
    );
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let now = state.now();
    let mut timing = Timing::load(state, roster.as_ref(), Some(&fixed_id), None).await?;
    let fixed = timing.fixed.clone().ok_or_else(timing_not_found)?;
    if !fixed.participants.contains(&member_id) {
        return Err(Refusal::unprocessable(
            "not_on_timing",
            "That member is not in this timing's party.",
        ));
    }
    let record = record(&session, now);
    let write = SlotWrite {
        scope: PickScope::Timing,
        target: &fixed_id,
        member_id: &member_id,
        version,
        at: now,
        actor: &record.actor,
    };
    if pick_version(write.current(&timing.picks)) != version {
        return Err(stale(PickScope::Timing));
    }
    if let Some(character_id) = &character_id {
        named(roster.as_ref(), character_id, &member_id).await?;
    }
    let change = write.plan(&mut timing.picks, character_id);
    let answer = timing.answer(&fixed);
    let slot = answer
        .slots
        .into_iter()
        .find(|slot| slot.member_id == member_id)
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    apply(roster.as_ref(), keyed, change, record, json!(slot)).await
}
