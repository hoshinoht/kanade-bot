//! Run edits: move, status, RSVP, this week's participants, reset to the
//! weekly timing, and the ping preview.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::{NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

use super::{
    bad_body, origin,
    precondition::{Explicit, OverrideRef, SeenField, expectations},
    refusal::{Refusal, scheduler},
    state, write_context,
};
use crate::{
    api::{
        admin::context::{context, roster},
        auth::AdminSession,
        dto::{
            hhmm,
            week::{RunDto, day_index, run_dto, run_time},
        },
        error::ApiError,
        listeners::Site,
        state::ApiState,
        write::RunWrite,
    },
    domain::{
        history::{BlameTarget, rsvp_field},
        members::MemberProfile,
        schedule::{RsvpState, RunStatus, ScheduleSnapshot, StatusChange},
        scheduler::{SchedulerError, Scope},
    },
};

type Reply = Result<Response, Refusal>;

#[derive(Serialize)]
struct RunResult {
    run: RunDto,
    version: u64,
}

#[derive(Serialize)]
struct Previous {
    day: u8,
    time: Option<String>,
}

#[derive(Serialize)]
struct MoveResult {
    run: RunDto,
    previous: Previous,
    version: u64,
}

async fn load_run(state: &ApiState, run_id: &str) -> Result<ScheduleSnapshot, Refusal> {
    let snapshot = state
        .store
        .snapshot(Scope::Run(run_id.to_owned()))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    if snapshot.runs.iter().any(|run| run.id == run_id) {
        Ok(snapshot)
    } else {
        Err(Refusal::new(
            axum::http::StatusCode::NOT_FOUND,
            "not_found",
            "That run no longer exists.",
        ))
    }
}

/// The run as it stands after the write, with the version read first (so it
/// can only lag the data: a later edit at it may 409, never lose an update).
async fn after(
    site: &Site,
    state: &ApiState,
    run_id: &str,
    profiles: &[MemberProfile],
) -> Result<(RunDto, u64), Refusal> {
    let version = state
        .store
        .head()
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?
        .seq;
    let snapshot = load_run(state, run_id).await?;
    let ctx = context(site, state, roster(profiles), state.now());
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    let start = ctx.local_date(run.week_start);
    Ok((run_dto(&ctx, &snapshot, start, run), version))
}

/// Run one edit through the writer; a replayed Idempotency-Key answers the
/// run as it is now, which is the first attempt's result or later.
async fn edit(
    site: &Site,
    session: &AdminSession,
    headers: &HeaderMap,
    run_id: &str,
    fields: &[String],
    (version, explicit): (u64, Explicit),
    write: RunWrite,
) -> Result<(RunDto, u64), Refusal> {
    let state = state(site)?;
    let origin = origin(session, headers)?;
    // A missing run is a plain 404, never a precondition refusal.
    load_run(state, run_id).await?;
    let expect = expectations(
        state.store.as_ref(),
        BlameTarget::Run(run_id.to_owned()),
        fields,
        Some(version),
        &explicit,
        origin.request_id.is_some(),
    )
    .await?;
    let (ctx, profiles) = write_context(state).await?;
    match state.writer.run(origin, expect, run_id, write, &ctx).await {
        Ok(_) | Err(SchedulerError::AlreadyApplied { .. }) => {}
        Err(error) => return Err(scheduler(error)),
    }
    after(site, state, run_id, &profiles).await
}

fn declared(
    version: u64,
    expect: Option<Vec<SeenField>>,
    overrides: Option<Vec<OverrideRef>>,
) -> (u64, Explicit) {
    (version, Explicit { expect, overrides })
}

fn strict_time(text: &str) -> Option<NaiveTime> {
    let bytes = text.as_bytes();
    (bytes.len() == 5 && bytes[2] == b':' && text[..2].bytes().all(|b| b.is_ascii_digit()))
        .then(|| NaiveTime::parse_from_str(text, "%H:%M").ok())
        .flatten()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    day: u8,
    time: Option<String>,
    version: u64,
    #[serde(default)]
    expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    overrides: Option<Vec<OverrideRef>>,
}

pub async fn move_run(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(run_id): UrlPath<String>,
    body: Result<Json<MoveRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let state = state(&site)?;
    if request.day > 6 {
        return Err(Refusal::invalid("A boss week has seven days."));
    }
    let snapshot = load_run(state, &run_id).await?;
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .ok_or_else(|| Refusal::from(ApiError::NOT_FOUND))?;
    // The scheduler would move them; the portal keeps them as the record (mock contract).
    if run.status.is_terminal() {
        return Err(Refusal::invalid(
            "Finished and cancelled runs stay where they were.",
        ));
    }
    let zone = state.policy.zone();
    let current = zone.from_utc_datetime(&run.datetime.naive_utc());
    let time = match (&request.time, run.status) {
        (Some(text), _) => {
            strict_time(text).ok_or_else(|| Refusal::invalid("Times are HH:MM, 00:00 to 23:59."))?
        }
        // Own-time runs keep their clock; only the day moves.
        (None, RunStatus::Otot) => current.time(),
        (None, _) => return Err(Refusal::invalid("A scheduled run needs a time.")),
    };
    let ctx = context(&site, state, roster(&[]), state.now());
    let start = ctx.local_date(run.week_start);
    let previous = Previous {
        day: day_index(&ctx, start, run.datetime),
        time: run_time(&ctx, run),
    };
    let date = start + chrono::Days::new(u64::from(request.day));
    let to = zone
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .ok_or_else(|| Refusal::invalid("That time does not exist on that day."))?
        .with_timezone(&chrono::Utc);
    let (run, version) = edit(
        &site,
        &session,
        &headers,
        &run_id,
        &["slot".to_owned()],
        declared(request.version, request.expect, request.overrides),
        RunWrite::Move { to },
    )
    .await?;
    Ok(Json(MoveResult {
        run,
        previous,
        version,
    })
    .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusRequest {
    status: String,
    version: u64,
    #[serde(default)]
    expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    overrides: Option<Vec<OverrideRef>>,
}

pub async fn status(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(run_id): UrlPath<String>,
    body: Result<Json<StatusRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let status = RunStatus::parse(&request.status).map_err(|_| {
        Refusal::invalid("Pick one of planned, confirmed, own time, done or cancelled.")
    })?;
    let (run, version) = edit(
        &site,
        &session,
        &headers,
        &run_id,
        &["status".to_owned()],
        declared(request.version, request.expect, request.overrides),
        RunWrite::Status(StatusChange {
            status,
            announce: true,
            via_portal: true,
        }),
    )
    .await?;
    Ok(Json(RunResult { run, version }).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsvpRequest {
    member_id: String,
    answer: String,
    version: u64,
    #[serde(default)]
    expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    overrides: Option<Vec<OverrideRef>>,
}

pub async fn rsvp(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(run_id): UrlPath<String>,
    body: Result<Json<RsvpRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let answer = match request.answer.as_str() {
        "yes" => Some(RsvpState::Yes),
        "no" => Some(RsvpState::No),
        "clear" => None,
        _ => return Err(Refusal::invalid("An answer is yes, no or clear.")),
    };
    let snapshot = load_run(state, &run_id).await?;
    let on_run = snapshot
        .runs
        .iter()
        .any(|run| run.id == run_id && run.participants.contains(&request.member_id));
    if !on_run {
        return Err(Refusal::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "not_on_run",
            "That member is not on this run.",
        ));
    }
    let current = snapshot
        .rsvps
        .iter()
        .find(|rsvp| rsvp.run_id == run_id && rsvp.user_id == request.member_id)
        .map(|rsvp| rsvp.state);
    if answer.is_none() && current == Some(RsvpState::Maybe) {
        return Err(Refusal::invalid(
            "A maybe answer can be changed to yes or no, not cleared here.",
        ));
    }
    let (run, version) = edit(
        &site,
        &session,
        &headers,
        &run_id,
        &[rsvp_field(&request.member_id)],
        declared(request.version, request.expect, request.overrides),
        RunWrite::Rsvp {
            user_id: request.member_id,
            answer,
            current,
        },
    )
    .await?;
    Ok(Json(RunResult { run, version }).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantsRequest {
    #[serde(default)]
    add: Option<String>,
    #[serde(default)]
    remove: Option<String>,
    version: u64,
    #[serde(default)]
    expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    overrides: Option<Vec<OverrideRef>>,
}

pub async fn participants(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(run_id): UrlPath<String>,
    body: Result<Json<ParticipantsRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    if request.add.is_none() && request.remove.is_none() {
        return Err(Refusal::invalid("Name someone to add or remove."));
    }
    let (run, version) = edit(
        &site,
        &session,
        &headers,
        &run_id,
        &["participants".to_owned()],
        declared(request.version, request.expect, request.overrides),
        RunWrite::Participants {
            add: request.add.into_iter().collect(),
            remove: request.remove.into_iter().collect(),
        },
    )
    .await?;
    Ok(Json(RunResult { run, version }).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetRequest {
    version: u64,
    #[serde(default)]
    expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    overrides: Option<Vec<OverrideRef>>,
}

pub async fn reset(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(run_id): UrlPath<String>,
    body: Result<Json<ResetRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let fields: Vec<String> = ["slot", "participants", "bosses", "channel"]
        .map(str::to_owned)
        .into();
    let (run, version) = edit(
        &site,
        &session,
        &headers,
        &run_id,
        &fields,
        declared(request.version, request.expect, request.overrides),
        RunWrite::Reset,
    )
    .await?;
    Ok(Json(RunResult { run, version }).into_response())
}

#[derive(Serialize)]
struct Message {
    message: String,
}

/// A preview only: posting belongs to the delivery tick, which the API does
/// not drive, so nothing reaches Discord from here.
pub async fn ping(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(run_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let snapshot = load_run(state, &run_id).await?;
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .ok_or_else(|| Refusal::from(ApiError::NOT_FOUND))?;
    let zone = state.policy.zone();
    let at = hhmm(zone.from_utc_datetime(&run.datetime.naive_utc()));
    let channel = run.channel_id.as_deref().map_or_else(
        || "its channel".to_owned(),
        |id| {
            state
                .channels
                .channels()
                .into_iter()
                .find(|channel| channel.id == id)
                .map_or_else(|| id.to_owned(), |channel| channel.name)
        },
    );
    Ok(Json(Message {
        message: format!(
            "Preview (not posted): the morning card for {} at {at} in {channel}.",
            run.bosses.join(" + ")
        ),
    })
    .into_response())
}
