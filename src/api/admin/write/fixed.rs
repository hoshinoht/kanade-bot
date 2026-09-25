//! Weekly timings: create (and materialise its runs), edit with per-run
//! update/keep decisions for amended runs, retire; and the boss-text check.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

use super::{
    bad_body, origin,
    precondition::{Explicit, OverrideRef, SeenField, expectations},
    refusal::{Refusal, scheduler},
    state, write_context,
};
use crate::{
    api::{
        admin::context::{context, frames, roster},
        auth::AdminSession,
        dto::{self, Art},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        history::{Actor, BlameTarget, Origin},
        members::MemberProfile,
        schedule::{
            AmendedRunChoice, FixedEdit, FixedEditChoices, FixedEditRequest, NewFixedRun,
            ScheduleError, validate_channel, validate_participants,
        },
        scheduler::{SchedulerError, Scope},
    },
};

type Reply = Result<Response, Refusal>;

/// `FixedRequest` as the PWA sends it (0 = Monday); `decisions` maps each
/// amended run to `update` or `keep`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedRequest {
    weekday: u8,
    time: String,
    bosses: String,
    participants: Vec<String>,
    channel_id: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    decisions: BTreeMap<String, String>,
    /// Optional here: the Fixed page has no week version.
    #[serde(default)]
    version: Option<u64>,
    #[serde(default)]
    expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    overrides: Option<Vec<OverrideRef>>,
}

struct Checked {
    weekday: Weekday,
    time: NaiveTime,
    bosses: Vec<String>,
    participants: Vec<String>,
    channel_id: String,
    note: Option<String>,
}

fn checked(
    state: &ApiState,
    request: &FixedRequest,
    directory: &crate::domain::members::Roster,
) -> Result<Checked, Refusal> {
    let weekday = crate::domain::weeks::weekday_from_index(i64::from(request.weekday))
        .ok()
        .filter(|_| request.weekday <= 6)
        .ok_or_else(|| Refusal::invalid("Pick a weekday."))?;
    let time = (request.time.len() == 5)
        .then(|| NaiveTime::parse_from_str(&request.time, "%H:%M").ok())
        .flatten()
        .ok_or_else(|| Refusal::invalid("Times are HH:MM, 00:00 to 23:59."))?;
    let bosses = state
        .catalog
        .parse(&request.bosses)
        .map_err(|error| Refusal::invalid(error.to_string()))?;
    let participants = validate_participants(directory, &request.participants)
        .map_err(|error| scheduler(error.into()))?;
    if participants.is_empty() {
        return Err(scheduler(ScheduleError::NoParticipants.into()));
    }
    let channel_id = validate_channel(directory, &request.channel_id)
        .map_err(|error| scheduler(error.into()))?;
    Ok(Checked {
        weekday,
        time,
        bosses,
        participants,
        channel_id,
        note: request
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(str::to_owned),
    })
}

async fn row(
    site: &Site,
    state: &ApiState,
    fixed_id: &str,
    profiles: &[MemberProfile],
) -> Result<Response, Refusal> {
    let now = state.now();
    let [this, next] = frames(state, now).map_err(Refusal::from)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start, next.start]))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    let ctx = context(site, state, roster(profiles), now);
    dto::fixed::rows(&ctx, &snapshot, [this.start, next.start])
        .into_iter()
        .find(|row| row.id == fixed_id)
        .map(|row| Json(row).into_response())
        .ok_or_else(|| {
            Refusal::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "That weekly timing no longer exists.",
            )
        })
}

/// The timing a replayed create made, from its recorded change.
async fn created_by(state: &ApiState, origin: &Origin) -> Option<String> {
    let request_id = origin.request_id.as_deref()?;
    state
        .store
        .recorded_fixed(origin.actor.clone(), request_id.to_owned())
        .await
        .ok()
        .flatten()
}

pub async fn create(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<FixedRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let origin = origin(&session, &headers)?;
    let (ctx, profiles) = write_context(state).await?;
    let timing = checked(state, &request, &ctx.directory)?;
    // Discord sessions own what they create; other sign-ins name no Discord user.
    let owner_id = match &session.actor {
        Actor::Admin { id } => id.strip_prefix("discord:").map(str::to_owned),
        _ => None,
    }
    .unwrap_or_else(|| timing.participants[0].clone());
    let new = NewFixedRun {
        owner_id,
        channel_id: Some(timing.channel_id),
        bosses: timing.bosses,
        weekday: timing.weekday,
        time: timing.time,
        participants: timing.participants,
        note: timing.note,
    };
    let fixed_id = match state.writer.add_fixed(origin.clone(), new, &ctx).await {
        Ok(id) => id,
        Err(SchedulerError::AlreadyApplied { .. }) => created_by(state, &origin)
            .await
            .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?,
        Err(error) => return Err(scheduler(error)),
    };
    // Its runs are a separate, idempotent change (materialising twice adds nothing).
    let materialise = Origin {
        request_id: None,
        ..origin
    };
    state
        .writer
        .materialise(materialise, &ctx)
        .await
        .map_err(scheduler)?;
    let mut response = row(&site, state, &fixed_id, &profiles).await?;
    *response.status_mut() = StatusCode::CREATED;
    Ok(response)
}

pub async fn update(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(fixed_id): UrlPath<String>,
    body: Result<Json<FixedRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let origin = origin(&session, &headers)?;
    let (ctx, profiles) = write_context(state).await?;
    let timing = checked(state, &request, &ctx.directory)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    let current = snapshot
        .fixed_runs
        .iter()
        .find(|fixed| fixed.id == fixed_id)
        .ok_or_else(|| {
            Refusal::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "That weekly timing no longer exists.",
            )
        })?;
    // Only fields that differ, so an untouched field never conflicts.
    let mut fields = Vec::new();
    let mut edit = FixedEdit::default();
    if current.weekday != timing.weekday {
        edit.weekday = Some(timing.weekday);
        fields.push("day");
    }
    if current.time != timing.time {
        edit.time = Some(timing.time);
        fields.push("time");
    }
    if current.bosses != timing.bosses {
        edit.bosses = Some(timing.bosses);
        fields.push("bosses");
    }
    if current.participants != timing.participants {
        edit.participants = Some(timing.participants);
        fields.push("participants");
    }
    if current.channel_id.as_deref() != Some(timing.channel_id.as_str()) {
        edit.channel_id = Some(timing.channel_id);
        fields.push("channel");
    }
    if current.note != timing.note {
        edit.note = Some(timing.note.unwrap_or_default());
        fields.push("note");
    }
    if fields.is_empty() {
        return row(&site, state, &fixed_id, &profiles).await;
    }
    let mut choices = BTreeMap::new();
    for (run_id, decision) in &request.decisions {
        let choice = match decision.as_str() {
            "update" => AmendedRunChoice::UpdateToFixed,
            "keep" => AmendedRunChoice::KeepForThisWeek,
            _ => return Err(Refusal::invalid("Each decision is update or keep.")),
        };
        choices.insert(run_id.clone(), choice);
    }
    let fields: Vec<String> = fields.into_iter().map(str::to_owned).collect();
    let expect = expectations(
        state.store.as_ref(),
        BlameTarget::FixedRun(fixed_id.clone()),
        &fields,
        request.version,
        &Explicit {
            expect: request.expect,
            overrides: request.overrides,
        },
        origin.request_id.is_some(),
    )
    .await?;
    let edit = FixedEditRequest {
        fixed_id: fixed_id.clone(),
        edit,
        // Amended runs need an explicit update/keep each; none given refuses
        // with `choices_required` rather than silently moving them (v4).
        choices: FixedEditChoices::PerRun(choices),
    };
    match state.writer.edit_fixed(origin, expect, edit, &ctx).await {
        Ok(()) | Err(SchedulerError::AlreadyApplied { .. }) => {}
        Err(error) => return Err(scheduler(error)),
    }
    row(&site, state, &fixed_id, &profiles).await
}

#[derive(Serialize)]
struct Retired {
    cancelled: usize,
}

pub async fn retire(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(fixed_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let origin = origin(&session, &headers)?;
    let (ctx, _) = write_context(state).await?;
    let exists = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?
        .fixed_runs
        .iter()
        .any(|fixed| fixed.id == fixed_id);
    let cancelled = match state.writer.retire_fixed(origin, &fixed_id, &ctx).await {
        Ok(_) if !exists => {
            return Err(Refusal::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "That weekly timing no longer exists.",
            ));
        }
        Ok(cancelled) => cancelled,
        // The first attempt's count is not recorded; the replay reports none left.
        Err(SchedulerError::AlreadyApplied { .. }) => 0,
        Err(error) => return Err(scheduler(error)),
    };
    Ok(Json(Retired { cancelled }).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateRequest {
    text: String,
}

#[derive(Serialize)]
struct ValidateResult {
    bosses: Vec<dto::Boss>,
}

pub async fn validate_bosses(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    body: Result<Json<ValidateRequest>, JsonRejection>,
) -> Reply {
    let Json(request) = body.map_err(bad_body)?;
    let state = state(&site)?;
    let tokens = state
        .catalog
        .parse(&request.text)
        .map_err(|error| Refusal::invalid(error.to_string()))?;
    let art = Art {
        root: site.boss_dir.as_deref(),
    };
    Ok(Json(ValidateResult {
        bosses: dto::bosses(&state.catalog, &art, &tokens),
    })
    .into_response())
}
