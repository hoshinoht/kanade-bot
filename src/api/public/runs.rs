//! The member's own runs: answer, move within the current boss week, and the
//! run deep link Discord posts. Writes take the admin run edits' steps and
//! the one writer as `Surface::PublicPortal`, so notices, history and admin
//! undo match an admin edit; only participants write, nobody acts as staff,
//! and every write needs a fresh sign-in.

use std::sync::Arc;

use axum::{
    Json,
    extract::{
        Path as UrlPath, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::write::{admit, invalid_body, origin, path_id};
use crate::{
    api::{
        admin::{
            context::{context, frames, roster, run_lengths, unavailable},
            write::{
                Answer, Explicit, Previous, Refusal, SlotError, load_run, put_answer, reloaded,
                run_not_found, scheduler, slot, state, write_run,
            },
        },
        auth::{audit::AuditContext, member::MemberSession},
        dto::{
            iso_date, iso_instant,
            public::{
                MemberMoveResult, MemberRemoved, MemberRun, MemberRunLink, MemberRunResult,
                MemberTimingSlot, RunWeek, actor_label, member_run_in,
            },
            week::{Context, WeekFrame},
        },
        error::ApiError,
        listeners::Site,
        state::ApiState,
        write::RunWrite,
    },
    domain::{
        history::{Actor, ChangeFilter, ChangeRecord, MAX_PAGE, Origin, RowKey, RowValue},
        members::MemberProfile,
        schedule::{
            MemberRunRefusal, RsvpState, Run, ScheduleError, ScheduleSnapshot, utc_instant,
        },
        scheduler::Scope,
        settings::RunLengths,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnswerBody {
    answer: String,
    version: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MoveBody {
    day: u8,
    time: Option<String>,
    version: u64,
}

fn refused(status: StatusCode, code: &'static str, message: &'static str) -> Refusal {
    Refusal::new(status, code, message)
}

fn not_in_run() -> Refusal {
    scheduler(ScheduleError::MemberRun(MemberRunRefusal::NotInRun).into())
}

fn find<'a>(snapshot: &'a ScheduleSnapshot, run_id: &str) -> Result<&'a Run, Refusal> {
    snapshot
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .ok_or_else(run_not_found)
}

/// The caller's run in this or next boss week, still open: unknown and
/// out-of-window runs are the same 404; someone else's run is `not_in_run`.
async fn own_run(
    state: &ApiState,
    run_id: &str,
    me: &str,
) -> Result<(ScheduleSnapshot, RunWeek), Refusal> {
    let snapshot = load_run(state, run_id).await?;
    let run = find(&snapshot, run_id)?;
    let week = RunWeek::of(&frames(state, state.now())?, run.week_start);
    if !week.open() {
        return Err(run_not_found());
    }
    if !run.participants.iter().any(|id| id == me) {
        return Err(not_in_run());
    }
    if run.status.is_terminal() {
        return Err(refused(
            StatusCode::CONFLICT,
            "run_closed",
            "This run is finished or cancelled.",
        ));
    }
    Ok((snapshot, week))
}

/// The run as `me` sees it after a write, with the version read first.
async fn after(
    site: &Site,
    state: &ApiState,
    run_id: &str,
    profiles: &[MemberProfile],
    me: &str,
) -> Result<(MemberRun, u64), Refusal> {
    let (snapshot, version) = reloaded(state, run_id).await?;
    let now = state.now();
    let ctx = context(site, state, roster(profiles), now);
    let run = find(&snapshot, run_id)?;
    let week = RunWeek::of(&frames(state, now)?, run.week_start);
    let run_lengths = run_lengths(state).await;
    Ok((
        member_run_in(&ctx, &snapshot, run, week, &run_lengths, me),
        version,
    ))
}

/// `PUT /api/public/runs/{id}/answer` `{answer, version}`: the caller's own
/// answer, preconditioned on it alone (others' edits never make it stale).
pub(super) async fn answer(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<AnswerBody>, JsonRejection>,
) -> Result<Json<MemberRunResult>, Refusal> {
    let (member, key) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let run_id = path_id(path)?;
    let Ok(Json(AnswerBody { answer, version })) = body else {
        return Err(invalid_body());
    };
    let answer = match answer.as_str() {
        "yes" => RsvpState::Yes,
        "maybe" => RsvpState::Maybe,
        "no" => RsvpState::No,
        _ => return Err(invalid_body()),
    };
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let origin = origin(&session, key);
    // A retry answers before any live check; the writer re-checks the rest
    // inside the commit, so these are only the friendly early answers.
    if recorded(state, &origin).await?.is_none() {
        own_run(state, &run_id, me).await?;
    } else {
        load_run(state, &run_id).await?;
    }
    let profiles = put_answer(
        state,
        origin,
        &run_id,
        me,
        Answer::Own(answer),
        version,
        &Explicit::default(),
    )
    .await?;
    let (run, version) = after(&site, state, &run_id, &profiles, me).await?;
    Ok(Json(MemberRunResult { run, version }))
}

/// The change the origin's key recorded, if any (a retry).
async fn recorded(state: &ApiState, origin: &Origin) -> Result<Option<ChangeRecord>, Refusal> {
    let Some(request_id) = origin.request_id.clone() else {
        return Ok(None);
    };
    state
        .store
        .recorded_change(origin.actor.clone(), request_id)
        .await
        .map_err(|error| unavailable(error).into())
}

/// Where the run was before `record` moved it: a retry answers what the
/// first attempt did.
fn recorded_previous(ctx: &Context<'_>, record: ChangeRecord, run_id: &str) -> Option<Previous> {
    record
        .rows
        .into_iter()
        .find_map(|row| match (row.key, row.before) {
            (RowKey::Run(id), Some(RowValue::Run(before))) if id == run_id => {
                Some(Previous::of(ctx, &before))
            }
            _ => None,
        })
}

/// `POST /api/public/runs/{id}/move` `{day, time, version}`: the caller's own
/// run, within the current boss week, before it starts and never into the
/// past; an own-time run may keep its clock (`time: null`) or get one.
pub(super) async fn move_run(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<MoveBody>, JsonRejection>,
) -> Result<Json<MemberMoveResult>, Refusal> {
    let (member, key) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let run_id = path_id(path)?;
    let Ok(Json(MoveBody { day, time, version })) = body else {
        return Err(invalid_body());
    };
    if day > 6 {
        return Err(invalid_body());
    }
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let origin = origin(&session, key);
    let now = state.now();
    let ctx = context(&site, state, roster(&[]), now);
    let record = recorded(state, &origin).await?;
    let snapshot = if record.is_some() {
        load_run(state, &run_id).await?
    } else {
        let (snapshot, week) = own_run(state, &run_id, me).await?;
        let run = find(&snapshot, &run_id)?;
        let refusal = MemberRunRefusal::check(run, me, &[week_start(state, now)?], None, now)
            .err()
            .or((run.datetime <= now).then_some(MemberRunRefusal::Started));
        if week != RunWeek::Current || refusal.is_some() {
            return Err(scheduler(
                ScheduleError::MemberRun(refusal.unwrap_or(MemberRunRefusal::WeekOver)).into(),
            ));
        }
        snapshot
    };
    let run = find(&snapshot, &run_id)?;
    let to = slot(&state.policy, &ctx, run, day, time.as_deref()).map_err(|error| match error {
        SlotError::Time | SlotError::NeedsTime | SlotError::Missing => refused(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_time",
            "Pick a time, HH:MM, that exists on that day.",
        ),
        SlotError::Range | SlotError::OtherWeek => refused(
            StatusCode::UNPROCESSABLE_ENTITY,
            "outside_week",
            "That time is outside this boss week.",
        ),
    })?;
    let previous = match record {
        Some(record) => recorded_previous(&ctx, record, &run_id),
        None if to <= now => {
            return Err(scheduler(
                ScheduleError::MemberRun(MemberRunRefusal::InThePast).into(),
            ));
        }
        // Already there: nothing to write, nobody to tell.
        None if to == run.datetime => {
            let profiles = state.store.members().await.map_err(unavailable)?;
            let (run, version) = after(&site, state, &run_id, &profiles, me).await?;
            let previous = Previous::of(&ctx, find(&snapshot, &run_id)?);
            return Ok(Json(MemberMoveResult {
                run,
                previous,
                version,
            }));
        }
        None => None,
    }
    .unwrap_or_else(|| Previous::of(&ctx, run));
    let profiles = write_run(
        state,
        origin,
        &run_id,
        &["slot".to_owned()],
        (version, Explicit::default()),
        RunWrite::MemberMove { to },
    )
    .await?;
    let (run, version) = after(&site, state, &run_id, &profiles, me).await?;
    Ok(Json(MemberMoveResult {
        run,
        previous,
        version,
    }))
}

/// This boss week's start.
fn week_start(state: &ApiState, now: DateTime<Utc>) -> Result<DateTime<Utc>, Refusal> {
    Ok(frames(state, now)?[0].start)
}

/// The newest change that took `me` off the run, from the run's history.
async fn removal(
    state: &ApiState,
    run_id: &str,
    me: &str,
) -> Result<Option<(Actor, DateTime<Utc>)>, Refusal> {
    let on = |value: &Option<RowValue>| matches!(value, Some(RowValue::Run(run)) if run.participants.iter().any(|id| id == me));
    let key = RowKey::Run(run_id.to_owned());
    let mut before = None;
    loop {
        let page = state
            .store
            .history_page(ChangeFilter::Run(run_id.to_owned()), before, MAX_PAGE)
            .await
            .map_err(unavailable)?;
        let found = page.records.into_iter().find_map(|record| {
            record
                .rows
                .iter()
                .any(|row| row.key == key && on(&row.before) && !on(&row.after))
                .then_some((record.origin.actor, record.at))
        });
        match (found, page.next_before) {
            (Some(found), _) => return Ok(Some(found)),
            (None, Some(next)) => before = Some(next),
            (None, None) => return Ok(None),
        }
    }
}

/// This boss week's run of the same weekly timing.
async fn this_week(
    state: &ApiState,
    ctx: &Context<'_>,
    frames: &[WeekFrame; 2],
    fixed_id: &str,
    run_lengths: &RunLengths,
    me: &str,
) -> Result<Option<MemberRun>, Refusal> {
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![frames[0].start]))
        .await
        .map_err(unavailable)?;
    Ok(snapshot
        .runs
        .iter()
        .find(|run| run.fixed_run_id.as_deref() == Some(fixed_id))
        .map(|run| member_run_in(ctx, &snapshot, run, RunWeek::Current, run_lengths, me)))
}

/// `GET /api/public/runs/{id}`: the run a Discord link opens. Runs of this
/// and next boss week are open to every member; other weeks only to whoever
/// is or was on the run; anything else is the same 404 as an unknown id.
pub(super) async fn link(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Json<MemberRunLink>, Refusal> {
    let run_id = path_id(path)?;
    let state = state(&site)?;
    let now = state.now();
    let frames = frames(state, now)?;
    let snapshot = load_run(state, &run_id).await?;
    let run = find(&snapshot, &run_id)?;
    let me = session.user_id.as_str();
    let week = RunWeek::of(&frames, run.week_start);
    let on_run = run.participants.iter().any(|id| id == me);
    let removed = if on_run {
        None
    } else {
        removal(state, &run_id, me).await?
    };
    if !(week.open() || on_run || removed.is_some()) {
        return Err(run_not_found());
    }
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    let run_lengths = run_lengths(state).await;
    let fixed = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| snapshot.fixed_runs.iter().find(|fixed| fixed.id == id));
    let this_week = match fixed {
        Some(fixed) if week == RunWeek::Past => {
            this_week(state, &ctx, &frames, &fixed.id, &run_lengths, me).await?
        }
        _ => None,
    };
    let ends = state
        .policy
        .materialised_weeks(run.week_start)
        .ok()
        .and_then(|weeks| utc_instant(&weeks[1]).ok())
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    Ok(Json(MemberRunLink {
        run: member_run_in(&ctx, &snapshot, run, week, &run_lengths, me),
        week: week.as_str(),
        week_starts: iso_date(ctx.local_date(run.week_start)),
        week_ends_at: iso_instant(ends),
        started: run.datetime <= now,
        removed: removed.map(|(actor, at)| MemberRemoved {
            by: actor_label(&ctx, &actor),
            at: iso_instant(at),
        }),
        this_week,
        timing: fixed.map(MemberTimingSlot::of),
        generated_at: iso_instant(now),
    }))
}
