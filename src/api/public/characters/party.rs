//! `GET /api/public/runs/{id}/characters`: on a run the caller is on, the
//! character each party member brings (as the admin's `…/characters`
//! resolves it) with its IGN, class and roster level, read-only, one line
//! per boss when they bring different characters to different bosses, and
//! each boss's stored scouter reading of the character brought to it, of
//! any age, marked `stale` once at least `estimate_days` old (never looked
//! up here), plus the party totals of the run's newest finished lookup.
//! NEXON data shows only on the caller's own slot: the notice names admins
//! and share-card channels as who sees it, never the party (others see the
//! roster level, which holds NEXON's copied level). No combat power, no
//! verdicts, no picking. Anyone not on the run gets the generic `404`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    response::{IntoResponse, Response},
};

use super::roster;
use crate::{
    api::{
        admin::{
            characters::party,
            context::{context, roster as directory},
            write::{Refusal, run_not_found, state},
        },
        auth::member::MemberSession,
        dto::{
            self, Art,
            estimates::{LABEL, NOTE, run_totals},
            iso_instant,
            public::{
                PartyBossCharacter, PartyCharacter, PartyCharacterSlot, PartyEstimate, PartyMember,
                PartyTotal, PartyTotals, RunPartyCharacters,
            },
            readiness::{Cached, reading},
        },
        listeners::Site,
        public::write::path_id,
    },
    domain::{
        catalog::BossTable,
        characters::{Character, PartySlot},
        chardata::{ATTRIBUTION, Requirement, date_text},
    },
    infrastructure::{
        character_data::CharacterData,
        scouter::{Estimates, RosterIgn, Target},
    },
};

/// `character` as the party sees it: the caller's own with their fresh
/// NEXON level when there is one, anyone else's with the roster level.
/// Also whether a NEXON value shows.
async fn shown(
    module: Option<&CharacterData>,
    character: &Character,
    you: bool,
) -> Result<(PartyCharacter, bool), Refusal> {
    let nexon = match module {
        Some(module) if you => module
            .data(&character.id)
            .await
            .map_err(Refusal::unavailable)?
            .and_then(|snapshot| {
                snapshot
                    .level
                    .map(|level| (level, date_text(snapshot.as_of())))
            }),
        _ => None,
    };
    Ok(match nexon {
        Some((level, as_of)) => (
            PartyCharacter {
                ign: character.ign.clone(),
                class: character.class.clone(),
                level: Some(level),
                level_source: "nexon",
                as_of: Some(as_of),
            },
            true,
        ),
        None => (
            PartyCharacter {
                ign: character.ign.clone(),
                class: character.class.clone(),
                level: character.level,
                level_source: "roster",
                as_of: None,
            },
            false,
        ),
    })
}

/// The character brought to each of `bosses` (the run's tokens the catalog
/// knows, in order).
fn per_boss<'s>(
    catalog: &BossTable,
    slot: &'s PartySlot,
    bosses: &[Requirement],
) -> Vec<Option<&'s Character>> {
    let known: Vec<usize> = (0..slot.bosses.len())
        .filter(|at| catalog.split(&slot.bosses[*at].token).is_some())
        .collect();
    (0..bosses.len())
        .map(|at| match known.get(at) {
            Some(boss) => slot.brought_to(*boss).map(|(character, _)| character),
            None => slot.brought().map(|(character, _)| character),
        })
        .collect()
}

pub(super) async fn read(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Response, Refusal> {
    let run_id = path_id(path)?;
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let (run, slots) = party(state, roster(state)?.as_ref(), &run_id).await?;
    // Not on the run: the same answer as a run that does not exist.
    if !run.participants.iter().any(|id| id == me) {
        return Err(run_not_found());
    }
    let profiles = state.store.members().await.map_err(Refusal::unavailable)?;
    let ctx = context(&site, state, directory(&profiles), state.now());
    let bosses: Vec<Requirement> = run
        .bosses
        .iter()
        .filter_map(|token| dto::boss(&state.catalog, &Art { root: None }, token))
        .map(|boss| Requirement {
            boss: boss.key,
            difficulty: boss.difficulty,
            entry_level: None,
        })
        .collect();
    let module = state.character_data.as_deref();
    let estimates = module.and_then(CharacterData::estimates);
    let mut nexon_data = false;
    let mut out = Vec::with_capacity(slots.len());
    for slot in &slots {
        let you = slot.member_id == me;
        let brought = slot.brought().map(|(character, _)| character);
        let character = match brought {
            Some(character) => {
                let (line, nexon) = shown(module, character, you).await?;
                nexon_data |= nexon;
                Some(line)
            }
            None => None,
        };
        let carried = per_boss(&state.catalog, slot, &bosses);
        let differs = carried
            .iter()
            .any(|character| character.map(|row| &row.id) != brought.map(|row| &row.id));
        let mut lines = Vec::new();
        if differs {
            for (boss, character) in bosses.iter().zip(&carried) {
                let character = match character {
                    Some(character) => {
                        let (line, nexon) = shown(module, character, you).await?;
                        nexon_data |= nexon;
                        Some(line)
                    }
                    None => None,
                };
                lines.push(PartyBossCharacter {
                    boss: boss.boss.clone(),
                    difficulty: boss.difficulty.clone(),
                    character,
                });
            }
        }
        // Each boss's reading is of the character brought to it.
        let scouter = estimates
            .filter(|_| carried.iter().any(Option::is_some))
            .map(|estimates| {
                bosses
                    .iter()
                    .zip(&carried)
                    .filter_map(|(boss, character)| {
                        let ign = RosterIgn::lookup((*character)?).ok()?;
                        let cached = Cached::of(estimates, &ign)?;
                        reading(boss, &cached).map(|reading| PartyEstimate {
                            boss: boss.boss.clone(),
                            difficulty: boss.difficulty.clone(),
                            reading,
                        })
                    })
                    .collect()
            });
        out.push(PartyCharacterSlot {
            member: PartyMember {
                id: slot.member_id.clone(),
                name: ctx.name(&slot.member_id),
            },
            you,
            character,
            scouter,
            bosses: lines,
        });
    }
    let totals =
        estimates.and_then(|estimates| party_totals(estimates, &run.id, &bosses, &state.catalog));
    Ok(Json(RunPartyCharacters {
        run_id: run.id,
        slots: out,
        character_data: module.is_some(),
        estimates: estimates.is_some(),
        fresh_days: estimates.map(Estimates::fresh_days),
        totals,
        nexon_data,
        attribution: ATTRIBUTION,
        estimate_label: LABEL,
        estimate_note: NOTE,
    })
    .into_response())
}

/// The totals of the run's newest lookup once it is done (what the admin's
/// Estimate panel shows), for the run's bosses as they are now, `stale` by
/// the job's `fetched_at`; never starts one.
fn party_totals(
    estimates: &Estimates,
    run_id: &str,
    bosses: &[Requirement],
    catalog: &BossTable,
) -> Option<PartyTotals> {
    let job = estimates.latest(&Target::Run {
        run_id: run_id.to_owned(),
        bosses: Vec::new(),
    })?;
    let (fetched_at, rows) = run_totals(&job, catalog)?;
    let bosses = bosses
        .iter()
        .filter_map(|boss| {
            let token = format!("{}{}", boss.difficulty.to_ascii_uppercase(), boss.boss);
            let row = rows.iter().find(|row| row.boss == token)?;
            Some(PartyTotal {
                boss: boss.boss.clone(),
                difficulty: boss.difficulty.clone(),
                combined_percentage: row.combined_percentage,
                estimated_minutes: row.estimated_minutes,
                technically_possible: row.technically_possible,
                complete: row.complete,
            })
        })
        .collect();
    Some(PartyTotals {
        fetched_at: iso_instant(fetched_at),
        stale: estimates.stale(fetched_at),
        bosses,
    })
}
