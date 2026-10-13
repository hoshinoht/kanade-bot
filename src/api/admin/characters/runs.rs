//! The character each party member brings to a run: a read of the run's
//! slots (candidates, the run's own pick, what each member brings and why,
//! warnings) and a write that sets or clears one member's pick. Reads never
//! write: what a member brings is resolved each time (the run's pick, else
//! its weekly timing's pick, read live, else their main, else their only
//! character). A pick saves directly, also while the admin stages a draft
//! (it is roster data outside the history chain), with its History row and
//! keyed replay in one transaction; a live run past its end refuses it
//! (`409 run_ended`), as every other run write does.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::json;

use super::{
    super::replay::recall,
    Reply, Roster,
    picks::{CharacterPick, SlotWrite, apply, named, stale},
    write::record,
};
use crate::{
    api::{
        admin::{
            context::{context, run_ends},
            write::{Refusal, bad_body, load_run, state},
        },
        auth::AdminSession,
        dto::characters::{RunCharacters, run_slot},
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::{
        characters::{
            Character, PartySlot, Pick, PickScope, StoredPicks, party_slots, pick_version,
        },
        members::Roster as Members,
        schedule::{FixedRun, RUN_ENDED, Run},
        scheduler::Scope,
    },
};

/// The run, from its own snapshot (`404 not_found` when it is gone).
async fn run_of(state: &ApiState, run_id: &str) -> Result<Run, Refusal> {
    load_run(state, run_id)
        .await?
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .ok_or_else(|| ApiError::UNAVAILABLE.into())
}

/// `run_id`'s run and its party slots as `GET …/characters` reads them,
/// for the run's readiness and scouter estimate.
pub async fn party(
    state: &ApiState,
    roster: &dyn CharacterRoster,
    run_id: &str,
) -> Result<(Run, Vec<PartySlot>), Refusal> {
    let run = run_of(state, run_id).await?;
    let week = Week::load(state, roster, &run).await?;
    let slots = week.slots(state, &run);
    Ok((run, slots))
}

/// What a run's slots are computed from: the runs of its boss week, every
/// weekly timing, the party's characters, and the stored picks on those
/// runs and on their timings.
struct Week {
    runs: Vec<Run>,
    timings: Vec<FixedRun>,
    characters: BTreeMap<String, Vec<Character>>,
    picks: Vec<Pick>,
}

impl Week {
    async fn load(
        state: &ApiState,
        roster: &dyn CharacterRoster,
        run: &Run,
    ) -> Result<Self, Refusal> {
        let snapshot = state
            .store
            .snapshot(Scope::Weeks(vec![run.week_start]))
            .await
            .map_err(Refusal::unavailable)?;
        let mut runs = snapshot.runs;
        // The run as just read, whatever the week read saw.
        runs.retain(|other| other.id != run.id);
        runs.push(run.clone());
        let mut characters = BTreeMap::new();
        for member in &run.participants {
            let rows = roster
                .member_characters(member.clone())
                .await
                .map_err(Refusal::unavailable)?;
            characters.insert(member.clone(), rows);
        }
        let mut picks = roster
            .picks(
                PickScope::Run,
                runs.iter().map(|run| run.id.clone()).collect(),
            )
            .await
            .map_err(Refusal::unavailable)?;
        let mut fixed: Vec<String> = runs
            .iter()
            .filter_map(|run| run.fixed_run_id.clone())
            .collect();
        fixed.sort();
        fixed.dedup();
        if !fixed.is_empty() {
            picks.extend(
                roster
                    .picks(PickScope::Timing, fixed)
                    .await
                    .map_err(Refusal::unavailable)?,
            );
        }
        Ok(Self {
            runs,
            timings: snapshot.fixed_runs,
            characters,
            picks,
        })
    }

    fn slots(&self, state: &ApiState, run: &Run) -> Vec<PartySlot> {
        party_slots(
            run,
            &self.runs,
            &self.characters,
            &StoredPicks::new(&self.picks, &self.timings),
            &state.catalog,
        )
    }

    /// `run`'s slots as the API answers them, in party order.
    fn answer(&self, site: &Site, state: &ApiState, run: &Run) -> RunCharacters {
        let ctx = context(site, state, Members::new(), state.now());
        RunCharacters {
            run_id: run.id.clone(),
            slots: self
                .slots(state, run)
                .iter()
                .map(|slot| run_slot(&ctx, &self.runs, slot))
                .collect(),
        }
    }
}

/// `GET /api/admin/runs/{id}/characters`.
pub async fn read(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(run_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let run = run_of(state, &run_id).await?;
    let week = Week::load(state, roster.as_ref(), &run).await?;
    Ok(Json(week.answer(&site, state, &run)).into_response())
}

/// `PUT /api/admin/runs/{id}/characters/{member}`: set or clear one party
/// member's pick on the run. The answer is that member's slot as it now
/// reads.
pub async fn pick(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((run_id, member_id)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<CharacterPick>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let (character_id, version) = body.parts("run's")?;
    let request = format!(
        "character.pick\u{1f}{run_id}\u{1f}{member_id}\u{1f}{}",
        json!({ "character_id": character_id, "version": version })
    );
    let (keyed, answered) = recall(state, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let now = state.now();
    let run = run_of(state, &run_id).await?;
    if run_ends(state).await.frozen(&run, now) {
        return Err(Refusal::new(StatusCode::CONFLICT, "run_ended", RUN_ENDED));
    }
    if !run.participants.contains(&member_id) {
        return Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_on_run",
            "That member is not in this run's party.",
        ));
    }
    let mut week = Week::load(state, roster.as_ref(), &run).await?;
    let record = record(&session, now);
    let write = SlotWrite {
        scope: PickScope::Run,
        target: &run_id,
        member_id: &member_id,
        version,
        at: now,
        actor: &record.actor,
    };
    if pick_version(write.current(&week.picks)) != version {
        return Err(stale(PickScope::Run));
    }
    if let Some(character_id) = &character_id {
        named(roster.as_ref(), character_id, &member_id).await?;
    }
    let change = write.plan(&mut week.picks, character_id);
    let answer = week.answer(&site, state, &run);
    let slot = answer
        .slots
        .into_iter()
        .find(|slot| slot.member_id == member_id)
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    apply(roster.as_ref(), keyed, change, record, json!(slot)).await
}
