//! The character each party member brings to a run: a read of the run's
//! slots (candidates, the run's own pick, what each member brings and why,
//! warnings) and a write that sets or clears one member's pick. Reads never
//! write: what a member brings is resolved each time (the run's pick, else
//! its weekly timing's pick, read live, else their main, else their only
//! character). A pick saves directly, also while the admin stages a draft
//! (it is roster data outside the history chain), with its History row and
//! keyed replay in one transaction; a live run past its end refuses it
//! (`409 run_ended`), as every other run write does.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::HeaderMap,
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
            context::{context, run_ends, weeks},
            write::{Body, Refusal, load_run, state},
        },
        auth::AdminSession,
        dto::characters::{RunCharacters, run_slot},
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::{
        characters::{PartyRoster, PartySlot, PickScope, pick_version},
        members::Roster as Members,
        schedule::{FixedRun, RUN_ENDED, Run},
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
/// weekly timing, and the party's characters and picks.
struct Week {
    runs: Vec<Run>,
    timings: Vec<FixedRun>,
    party: PartyRoster,
}

impl Week {
    async fn load(
        state: &ApiState,
        roster: &dyn CharacterRoster,
        run: &Run,
    ) -> Result<Self, Refusal> {
        let snapshot = weeks(state, vec![run.week_start]).await?;
        let mut runs = snapshot.runs;
        // The run as just read, whatever the week read saw.
        runs.retain(|other| other.id != run.id);
        runs.push(run.clone());
        let party = PartyRoster::load(roster, run, &runs)
            .await
            .map_err(Refusal::unavailable)?;
        Ok(Self {
            runs,
            timings: snapshot.fixed_runs,
            party,
        })
    }

    fn slots(&self, state: &ApiState, run: &Run) -> Vec<PartySlot> {
        self.party
            .slots(run, &self.runs, &self.timings, &state.catalog)
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
    Body(body): Body<CharacterPick>,
) -> Reply {
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
        return Err(Refusal::conflict("run_ended", RUN_ENDED));
    }
    if !run.participants.contains(&member_id) {
        return Err(Refusal::unprocessable(
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
    if pick_version(write.current(&week.party.picks)) != version {
        return Err(stale(PickScope::Run));
    }
    if let Some(character_id) = &character_id {
        named(roster.as_ref(), character_id, &member_id).await?;
    }
    let change = write.plan(&mut week.party.picks, character_id);
    let answer = week.answer(&site, state, &run);
    let slot = answer
        .slots
        .into_iter()
        .find(|slot| slot.member_id == member_id)
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    apply(roster.as_ref(), keyed, change, record, json!(slot)).await
}
