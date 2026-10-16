//! The character each party member brings to a weekly timing (modules
//! `roster` and `character_data`): a read of the timing's slots, a write
//! that sets or clears one member's pick for it (the first boss's, and
//! every boss's without one of its own), one for a later boss, and "same
//! for every boss". Runs made from the timing read the picks live (a run's
//! own pick for a boss still wins). Reads never write: the default (main,
//! else the only character; a later boss the first boss's character) is
//! resolved each time. A pick saves directly, outside the timing's own form
//! and the history chain, with its History row (key
//! `fixedpick.<fixed>.<member>`, `.<boss>` per boss, `.*` for "same for
//! every boss") and keyed replay in one transaction. Every route answers
//! the generic `404` before the session is checked while either module is
//! off.

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
    picks::{
        CharacterPick, SameForEveryBoss, SlotWrite, apply, later_boss, later_versions, named,
        plan_same, stale,
    },
    write::record,
};
use crate::{
    api::{
        admin::{
            context::weeks,
            write::{Body, Refusal, state},
        },
        auth::AdminSession,
        dto::characters::{RunCharacterSlot, TimingCharacters, slot},
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::{
        catalog::BossTable,
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

    /// The slots of `party` in its order against `bosses` (only the saved
    /// party's members have picks).
    pub(in crate::api::admin) fn slots(
        &self,
        catalog: &BossTable,
        party: &[String],
        bosses: &[String],
    ) -> Vec<PartySlot> {
        timing_slots(
            self.fixed.as_ref().map(|fixed| fixed.id.as_str()),
            party,
            bosses,
            &self.characters,
            &StoredPicks::new(&self.picks, &self.timings),
            catalog,
        )
    }

    fn answer(&self, catalog: &BossTable, fixed: &FixedRun) -> TimingCharacters {
        TimingCharacters {
            fixed_run_id: fixed.id.clone(),
            slots: self
                .slots(catalog, &fixed.participants, &fixed.bosses)
                .iter()
                .map(slot)
                .collect(),
        }
    }

    /// `member_id`'s slot as the API answers it (a write's answer).
    fn slot(
        &self,
        catalog: &BossTable,
        fixed: &FixedRun,
        member_id: &str,
    ) -> Result<RunCharacterSlot, Refusal> {
        self.answer(catalog, fixed)
            .slots
            .into_iter()
            .find(|slot| slot.member_id == member_id)
            .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))
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
    Ok(Json(timing.answer(&state.catalog, fixed)).into_response())
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
    let write = Asked {
        site: &site,
        session: &session,
        headers: &headers,
        fixed_id: &fixed_id,
        member_id: &member_id,
    };
    write.pick(roster.as_ref(), None, body).await
}

/// `PUT /api/admin/fixed/{id}/characters/{member}/bosses/{boss}`: set or
/// clear one party member's pick for a later boss of the timing (`boss`:
/// its token or catalog boss). The answer is that member's slot as it now
/// reads.
pub async fn boss_pick(
    Both(roster): Both,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((fixed_id, member_id, boss)): UrlPath<(String, String, String)>,
    headers: HeaderMap,
    Body(body): Body<CharacterPick>,
) -> Reply {
    let write = Asked {
        site: &site,
        session: &session,
        headers: &headers,
        fixed_id: &fixed_id,
        member_id: &member_id,
    };
    write.pick(roster.as_ref(), Some(&boss), body).await
}

/// `POST /api/admin/fixed/{id}/characters/{member}/same`: "same for every
/// boss", clearing the member's per-boss picks for the timing in one
/// write. The answer is that member's slot as it now reads.
pub async fn same(
    Both(roster): Both,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((fixed_id, member_id)): UrlPath<(String, String)>,
    headers: HeaderMap,
    Body(body): Body<SameForEveryBoss>,
) -> Reply {
    let write = Asked {
        site: &site,
        session: &session,
        headers: &headers,
        fixed_id: &fixed_id,
        member_id: &member_id,
    };
    write.same(roster.as_ref(), body).await
}

/// One write to a member's slots on a timing, as asked.
struct Asked<'a> {
    site: &'a Site,
    session: &'a AdminSession,
    headers: &'a HeaderMap,
    fixed_id: &'a str,
    member_id: &'a str,
}

impl Asked<'_> {
    /// The timing as read now and the saved timing, unless the member is
    /// not in its saved party.
    async fn timing(
        &self,
        state: &ApiState,
        roster: &dyn CharacterRoster,
    ) -> Result<(Timing, FixedRun), Refusal> {
        let timing = Timing::load(state, roster, Some(self.fixed_id), None).await?;
        let fixed = timing.fixed.clone().ok_or_else(timing_not_found)?;
        if !fixed.participants.iter().any(|id| id == self.member_id) {
            return Err(Refusal::unprocessable(
                "not_on_timing",
                "That member is not in this timing's party.",
            ));
        }
        Ok((timing, fixed))
    }

    /// `body` for the member's own pick, or with `boss` for that boss's.
    async fn pick(
        &self,
        roster: &dyn CharacterRoster,
        boss: Option<&str>,
        body: CharacterPick,
    ) -> Reply {
        let state = state(self.site)?;
        let (character_id, version) = body.parts("timing's")?;
        let (fixed_id, member_id) = (self.fixed_id, self.member_id);
        let asked = json!({ "character_id": character_id, "version": version });
        let request = match boss {
            None => format!("character.fixedpick\u{1f}{fixed_id}\u{1f}{member_id}\u{1f}{asked}"),
            Some(boss) => format!(
                "character.fixedbosspick\u{1f}{fixed_id}\u{1f}{member_id}\u{1f}{boss}\u{1f}{asked}"
            ),
        };
        let (keyed, answered) = recall(state, self.session, self.headers, &request).await?;
        if let Some(answer) = answered {
            return Ok(answer);
        }
        let now = state.now();
        let (mut timing, fixed) = self.timing(state, roster).await?;
        let boss = boss
            .map(|asked| later_boss(&state.catalog, &fixed.bosses, asked, PickScope::Timing))
            .transpose()?;
        let record = record(self.session, now);
        let write = SlotWrite {
            scope: PickScope::Timing,
            target: fixed_id,
            member_id,
            boss: boss.as_deref(),
            version,
            at: now,
            actor: &record.actor,
        };
        if pick_version(write.current(&timing.picks)) != version {
            return Err(stale(PickScope::Timing));
        }
        if let Some(character_id) = &character_id {
            named(roster, character_id, member_id).await?;
        }
        let change = write.plan(&mut timing.picks, character_id);
        let slot = timing.slot(&state.catalog, &fixed, member_id)?;
        apply(roster, keyed, change, record, json!(slot)).await
    }

    /// "Same for every boss" against the versions `body` was loaded at.
    async fn same(&self, roster: &dyn CharacterRoster, body: SameForEveryBoss) -> Reply {
        let state = state(self.site)?;
        let sent = body.versions("timing's")?;
        let (fixed_id, member_id) = (self.fixed_id, self.member_id);
        let request = format!(
            "character.fixedsamepick\u{1f}{fixed_id}\u{1f}{member_id}\u{1f}{}",
            json!({ "versions": sent })
        );
        let (keyed, answered) = recall(state, self.session, self.headers, &request).await?;
        if let Some(answer) = answered {
            return Ok(answer);
        }
        let now = state.now();
        let (mut timing, fixed) = self.timing(state, roster).await?;
        let slots = timing.slots(&state.catalog, &fixed.participants, &fixed.bosses);
        let slot = slots
            .iter()
            .find(|slot| slot.member_id == member_id)
            .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
        let (versions, fresh) = later_versions(slot, &sent);
        if !fresh {
            return Err(stale(PickScope::Timing));
        }
        let change = plan_same(
            &mut timing.picks,
            PickScope::Timing,
            fixed_id,
            member_id,
            versions,
        );
        let slot = timing.slot(&state.catalog, &fixed, member_id)?;
        apply(
            roster,
            keyed,
            change,
            record(self.session, now),
            json!(slot),
        )
        .await
    }
}
