//! The character each party member brings to a run: a read of the run's
//! slots (candidates, picked, inferred, warnings) and a write that sets or
//! clears one member's pick. Reads never write: `inferred` is recomputed
//! each time. A pick saves directly, also while the admin stages a draft
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
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Reply, Roster,
    write::{present, recall, record},
};
use crate::{
    api::{
        admin::{
            context::{context, run_ends},
            replay::{Keyed, mismatch, replayed},
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
            Character, CharacterRecord, PartySlot, PickChange, PickWrite, RunPick, party_slots,
            pick_version,
        },
        members::Roster as Members,
        schedule::{RUN_ENDED, Run},
        scheduler::Scope,
    },
    infrastructure::store::replays::KeyedWrite,
};

fn unavailable<T>(_: T) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

fn stale() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "stale",
        "This member's character on the run changed since it was loaded.",
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
fn refused(written: PickWrite) -> Refusal {
    match written {
        PickWrite::Stale(_) => stale(),
        PickWrite::NotTheirs => not_theirs(),
        PickWrite::Retired => retired(),
        PickWrite::UnknownCharacter | PickWrite::Picked(_) | PickWrite::Cleared => {
            character_not_found()
        }
    }
}

/// The run, from its own snapshot (`404 not_found` when it is gone).
async fn run_of(state: &ApiState, run_id: &str) -> Result<Run, Refusal> {
    load_run(state, run_id)
        .await?
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .ok_or_else(|| ApiError::UNAVAILABLE.into())
}

/// `run_id`'s run and its party slots as `GET …/characters` reads them
/// (picked, else inferred), for the run's MapleScouter estimate.
pub async fn party(
    state: &ApiState,
    roster: &dyn CharacterRoster,
    run_id: &str,
) -> Result<(Run, Vec<PartySlot>), Refusal> {
    let run = run_of(state, run_id).await?;
    let week = Week::load(state, roster, &run).await?;
    let slots = party_slots(
        &run,
        &week.runs,
        &week.characters,
        &week.picks,
        &state.catalog,
    );
    Ok((run, slots))
}

/// What a run's slots are computed from: the runs of its boss week, the
/// party's characters and the stored picks on those runs.
struct Week {
    runs: Vec<Run>,
    characters: BTreeMap<String, Vec<Character>>,
    picks: Vec<RunPick>,
}

impl Week {
    async fn load(
        state: &ApiState,
        roster: &dyn CharacterRoster,
        run: &Run,
    ) -> Result<Self, Refusal> {
        let mut runs = state
            .store
            .snapshot(Scope::Weeks(vec![run.week_start]))
            .await
            .map_err(unavailable)?
            .runs;
        // The run as just read, whatever the week read saw.
        runs.retain(|other| other.id != run.id);
        runs.push(run.clone());
        let mut characters = BTreeMap::new();
        for member in &run.participants {
            let rows = roster
                .member_characters(member.clone())
                .await
                .map_err(unavailable)?;
            characters.insert(member.clone(), rows);
        }
        let picks = roster
            .run_picks(runs.iter().map(|run| run.id.clone()).collect())
            .await
            .map_err(unavailable)?;
        Ok(Self {
            runs,
            characters,
            picks,
        })
    }

    fn pick(&self, run_id: &str, member_id: &str) -> Option<&RunPick> {
        self.picks
            .iter()
            .find(|pick| pick.run_id == run_id && pick.member_id == member_id)
    }

    /// `run`'s slots as the API answers them, in party order.
    fn answer(&self, site: &Site, state: &ApiState, run: &Run) -> RunCharacters {
        let ctx = context(site, state, Members::new(), state.now());
        let slots = party_slots(
            run,
            &self.runs,
            &self.characters,
            &self.picks,
            &state.catalog,
        );
        RunCharacters {
            run_id: run.id.clone(),
            slots: slots
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

/// Write `change`, answering `body`, which a keyed request stores as its
/// replay in the same transaction.
async fn apply(
    roster: &dyn CharacterRoster,
    keyed: Option<Keyed>,
    change: PickChange,
    record: CharacterRecord,
    body: Value,
) -> Reply {
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::OK, &body, record.at));
    match roster.write_pick(change, record, replay).await {
        Ok(KeyedWrite::Applied(written)) if written.applied() => Ok(Json(body).into_response()),
        Ok(KeyedWrite::Applied(written)) => Err(refused(written)),
        Ok(KeyedWrite::Replayed(found)) => replayed(&found),
        Ok(KeyedWrite::KeyTaken) => Err(mismatch()),
        Err(_) => Err(ApiError::UNAVAILABLE.into()),
    }
}

/// `PUT /api/admin/runs/{id}/characters/{member}`: set or clear one party
/// member's pick. The answer is that member's slot as it now reads.
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
    let version = body.version.ok_or_else(|| {
        Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "version_required",
            "Send the version the run's characters were loaded at.",
        )
    })?;
    let character_id = body
        .character_id
        .ok_or_else(|| Refusal::invalid("Send character_id, or null to clear the pick."))?;
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
    let current = week.pick(&run_id, &member_id).cloned();
    if pick_version(current.as_ref()) != version {
        return Err(stale());
    }
    let record = record(&session, now);
    let change = match character_id {
        Some(character_id) => {
            let character = roster
                .character(character_id.clone())
                .await
                .map_err(unavailable)?
                .ok_or_else(character_not_found)?;
            if character.member_id != member_id {
                return Err(not_theirs());
            }
            if !character.is_active() {
                return Err(retired());
            }
            // The slot as the write will leave it, for the answer.
            let after = match current {
                Some(current) if current.character_id == character_id => current,
                _ => RunPick {
                    run_id: run_id.clone(),
                    member_id: member_id.clone(),
                    character_id: character_id.clone(),
                    picked_at: now,
                    picked_by: record.actor.clone(),
                    version: version + 1,
                },
            };
            week.picks
                .retain(|pick| !(pick.run_id == run_id && pick.member_id == member_id));
            week.picks.push(after);
            PickChange::Set {
                run_id: run_id.clone(),
                member_id: member_id.clone(),
                character_id,
                version,
            }
        }
        None => {
            week.picks
                .retain(|pick| !(pick.run_id == run_id && pick.member_id == member_id));
            PickChange::Clear {
                run_id: run_id.clone(),
                member_id: member_id.clone(),
                version,
            }
        }
    };
    let answer = week.answer(&site, state, &run);
    let slot = answer
        .slots
        .into_iter()
        .find(|slot| slot.member_id == member_id)
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    apply(roster.as_ref(), keyed, change, record, json!(slot)).await
}
