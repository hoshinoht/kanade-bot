//! The character each party member brings to a run: a read of the run's
//! slots (candidates, the run's own pick, what each member brings to each
//! boss and why, warnings), a write that sets or clears one member's pick
//! (the first boss's, and every boss's without one of its own), one that
//! sets or clears their pick for a later boss, and "same for every boss",
//! which clears their per-boss picks in one write. Reads never write: what
//! a member brings is resolved each time (the run's pick, else its weekly
//! timing's pick, read live, else their main, else their only character;
//! a later boss its own run pick, else its timing's, else the first boss's
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
    picks::{
        CharacterPick, SameForEveryBoss, SlotWrite, apply, later_boss, later_versions, named,
        plan_same, stale,
    },
    write::record,
};
use crate::{
    api::{
        admin::{
            context::{context, run_ends, weeks},
            write::{Body, Refusal, load_run, state},
        },
        auth::AdminSession,
        dto::{
            characters::{RunCharacterSlot, RunCharacters, run_slot},
            estimates::party_total,
        },
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
        let slots = self.slots(state, run);
        let estimates = state
            .character_data
            .as_ref()
            .and_then(|module| module.estimates());
        RunCharacters {
            run_id: run.id.clone(),
            party_total: estimates.map(|estimates| party_total(estimates, &run.bosses, &slots)),
            slots: slots
                .iter()
                .map(|slot| run_slot(&ctx, &self.runs, slot))
                .collect(),
        }
    }

    /// `member_id`'s slot as the API answers it (a write's answer).
    fn slot(
        &self,
        site: &Site,
        state: &ApiState,
        run: &Run,
        member_id: &str,
    ) -> Result<RunCharacterSlot, Refusal> {
        self.answer(site, state, run)
            .slots
            .into_iter()
            .find(|slot| slot.member_id == member_id)
            .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))
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
    let write = Asked {
        site: &site,
        session: &session,
        headers: &headers,
        run_id: &run_id,
        member_id: &member_id,
    };
    write.pick(roster.as_ref(), None, body).await
}

/// `PUT /api/admin/runs/{id}/characters/{member}/bosses/{boss}`: set or
/// clear one party member's pick for a later boss of the run (`boss`: its
/// token or catalog boss). The answer is that member's slot as it now
/// reads.
pub async fn boss_pick(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((run_id, member_id, boss)): UrlPath<(String, String, String)>,
    headers: HeaderMap,
    Body(body): Body<CharacterPick>,
) -> Reply {
    let write = Asked {
        site: &site,
        session: &session,
        headers: &headers,
        run_id: &run_id,
        member_id: &member_id,
    };
    write.pick(roster.as_ref(), Some(&boss), body).await
}

/// `POST /api/admin/runs/{id}/characters/{member}/same`: "same for every
/// boss", clearing the member's per-boss picks on the run in one write.
/// The answer is that member's slot as it now reads.
pub async fn same(
    Roster(roster): Roster,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath((run_id, member_id)): UrlPath<(String, String)>,
    headers: HeaderMap,
    Body(body): Body<SameForEveryBoss>,
) -> Reply {
    let write = Asked {
        site: &site,
        session: &session,
        headers: &headers,
        run_id: &run_id,
        member_id: &member_id,
    };
    write.same(roster.as_ref(), body).await
}

/// One write to a member's slots on a run, as asked.
struct Asked<'a> {
    site: &'a Site,
    session: &'a AdminSession,
    headers: &'a HeaderMap,
    run_id: &'a str,
    member_id: &'a str,
}

impl Asked<'_> {
    /// The run, unless it is past its end or the member is not in its party.
    async fn run(&self, state: &ApiState) -> Result<Run, Refusal> {
        let run = run_of(state, self.run_id).await?;
        if run_ends(state).await.frozen(&run, state.now()) {
            return Err(Refusal::conflict("run_ended", RUN_ENDED));
        }
        if !run.participants.iter().any(|id| id == self.member_id) {
            return Err(Refusal::unprocessable(
                "not_on_run",
                "That member is not in this run's party.",
            ));
        }
        Ok(run)
    }

    /// `body` for the member's own pick, or with `boss` for that boss's.
    async fn pick(
        &self,
        roster: &dyn CharacterRoster,
        boss: Option<&str>,
        body: CharacterPick,
    ) -> Reply {
        let state = state(self.site)?;
        let (character_id, version) = body.parts("run's")?;
        let (run_id, member_id) = (self.run_id, self.member_id);
        let asked = json!({ "character_id": character_id, "version": version });
        let request = match boss {
            None => format!("character.pick\u{1f}{run_id}\u{1f}{member_id}\u{1f}{asked}"),
            Some(boss) => {
                format!(
                    "character.bosspick\u{1f}{run_id}\u{1f}{member_id}\u{1f}{boss}\u{1f}{asked}"
                )
            }
        };
        let (keyed, answered) = recall(state, self.session, self.headers, &request).await?;
        if let Some(answer) = answered {
            return Ok(answer);
        }
        let now = state.now();
        let run = self.run(state).await?;
        let boss = boss
            .map(|asked| later_boss(&state.catalog, &run.bosses, asked, PickScope::Run))
            .transpose()?;
        let mut week = Week::load(state, roster, &run).await?;
        let record = record(self.session, now);
        let write = SlotWrite {
            scope: PickScope::Run,
            target: run_id,
            member_id,
            boss: boss.as_deref(),
            version,
            at: now,
            actor: &record.actor,
        };
        if pick_version(write.current(&week.party.picks)) != version {
            return Err(stale(PickScope::Run));
        }
        if let Some(character_id) = &character_id {
            named(roster, character_id, member_id).await?;
        }
        let change = write.plan(&mut week.party.picks, character_id);
        let slot = week.slot(self.site, state, &run, member_id)?;
        apply(roster, keyed, change, record, json!(slot)).await
    }

    /// "Same for every boss" against the versions `body` was loaded at.
    async fn same(&self, roster: &dyn CharacterRoster, body: SameForEveryBoss) -> Reply {
        let state = state(self.site)?;
        let sent = body.versions("run's")?;
        let (run_id, member_id) = (self.run_id, self.member_id);
        let request = format!(
            "character.samepick\u{1f}{run_id}\u{1f}{member_id}\u{1f}{}",
            json!({ "versions": sent })
        );
        let (keyed, answered) = recall(state, self.session, self.headers, &request).await?;
        if let Some(answer) = answered {
            return Ok(answer);
        }
        let now = state.now();
        let run = self.run(state).await?;
        let mut week = Week::load(state, roster, &run).await?;
        let slots = week.slots(state, &run);
        let slot = slots
            .iter()
            .find(|slot| slot.member_id == member_id)
            .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
        let (versions, fresh) = later_versions(slot, &sent);
        if !fresh {
            return Err(stale(PickScope::Run));
        }
        let change = plan_same(
            &mut week.party.picks,
            PickScope::Run,
            run_id,
            member_id,
            versions,
        );
        let slot = week.slot(self.site, state, &run, member_id)?;
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
