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
        write::{FixedPatchReplay, WriteContext},
    },
    domain::{
        history::{Actor, BlameTarget, ChangeRecord, Origin, RowKey},
        members::{Member, MemberProfile, Roster},
        schedule::{
            AmendedRunChoice, FixedEdit, FixedEditChoices, FixedEditRequest, NewFixedRun,
            ScheduleError, validate_channel, validate_participants,
        },
        scheduler::{SchedulerError, Scope},
    },
};

mod replay;

use replay::{FixedPatchIdentity, RefusalReplay};

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
    /// Pins the owner. Omitted or `""`: the default (first participant) on
    /// create, unchanged on edit unless `""` unpins a pinned owner.
    #[serde(default)]
    owner_id: Option<String>,
    #[serde(default)]
    decisions: BTreeMap<String, String>,
    /// The week version the timing was loaded at: required to edit, ignored
    /// on create (a new timing has nothing to be stale against).
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
    owner_id: Option<String>,
}

fn checked(
    state: &ApiState,
    request: &FixedRequest,
    directory: &Roster,
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
        owner_id: request
            .owner_id
            .as_deref()
            .map(str::trim)
            .map(str::to_owned),
    })
}

/// The participants' roster rule (a known, non-bot member with the bossing
/// role), but the owner need not be in the party. Checked only when the
/// owner is set or changed, so a since-demoted owner never blocks an edit
/// of other fields.
fn rostered_owner(directory: &Roster, owner_id: &str) -> Result<String, Refusal> {
    validate_participants(directory, &[owner_id.to_owned()])
        .ok()
        .and_then(|mut owner| owner.pop())
        .ok_or_else(|| Refusal::invalid("Pick an owner from the roster."))
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

/// The change already recorded for this request's `Idempotency-Key`, if any.
async fn recorded(state: &ApiState, origin: &Origin) -> Result<Option<ChangeRecord>, Refusal> {
    let Some(request_id) = origin.request_id.clone() else {
        return Ok(None);
    };
    state
        .store
        .recorded_change(origin.actor.clone(), request_id)
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))
}

/// A replay is checked against the roster as the first attempt saw it, not
/// as it is now: a member who has since lost the bossing role, or a channel
/// no longer watched, must not turn a replay into a 422. The scheduler still
/// compares the request digest, so a different request under the key is
/// `idempotency_mismatch`.
fn as_first_seen(ctx: &mut WriteContext, request: &FixedRequest) {
    for id in request.participants.iter().chain(&request.owner_id) {
        let id = id.trim();
        if ctx
            .directory
            .get(id)
            .is_none_or(|member| !member.has_role || member.is_bot)
        {
            let existing = ctx.directory.get(id).cloned().unwrap_or_default();
            ctx.directory.upsert(Member {
                user_id: id.to_owned(),
                has_role: true,
                is_bot: false,
                ..existing
            });
        }
    }
    ctx.directory.watch(&request.channel_id);
}

/// The timing a recorded create made.
fn created_id(record: &ChangeRecord) -> Option<String> {
    record
        .rows
        .iter()
        .find_map(|row| match (&row.key, &row.before) {
            (RowKey::FixedRun(id), None) => Some(id.clone()),
            _ => None,
        })
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
    let first = recorded(state, &origin).await?;
    let (mut ctx, profiles) = write_context(state).await?;
    if first.is_some() {
        as_first_seen(&mut ctx, &request);
    }
    let timing = checked(state, &request, &ctx.directory)?;
    // An explicit owner is pinned. Otherwise the first participant owns it;
    // the stored owner (the Discord admin who made it, else that participant)
    // only stands in for an empty party.
    let pinned = timing.owner_id.as_deref().filter(|owner| !owner.is_empty());
    let owner_id = match pinned {
        Some(owner_id) => rostered_owner(&ctx.directory, owner_id)?,
        None => match &session.actor {
            Actor::Admin { id } => id.strip_prefix("discord:").map(str::to_owned),
            _ => None,
        }
        .unwrap_or_else(|| timing.participants[0].clone()),
    };
    let new = NewFixedRun {
        owner_id,
        channel_id: Some(timing.channel_id),
        bosses: timing.bosses,
        weekday: timing.weekday,
        time: timing.time,
        participants: timing.participants,
        note: timing.note,
        owner_pinned: pinned.is_some(),
    };
    // One commit: the timing and its runs in the materialised weeks.
    let fixed_id = match state.writer.add_fixed(origin.clone(), new, &ctx).await {
        Ok(id) => id,
        Err(SchedulerError::AlreadyApplied { .. }) => recorded(state, &origin)
            .await?
            .as_ref()
            .and_then(created_id)
            .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?,
        Err(error) => return Err(scheduler(error)),
    };
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
    let first = recorded(state, &origin).await?.is_some();
    #[cfg(any(test, feature = "test-support"))]
    let lost_channel = crate::api::write::hold_fixed_patch_after_lookup(&origin).await;
    // Keep the established fresh-request refusal order, while letting a used
    // key compare its complete identity before version-dependent handling.
    let version = if first {
        None
    } else {
        Some(request.version.ok_or_else(|| {
            Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "version_required",
                "Send the week version the timing was loaded at.",
            )
        })?)
    };
    let (mut ctx, profiles) = write_context(state).await?;
    if first {
        as_first_seen(&mut ctx, &request);
    }
    let refusal_replay = RefusalReplay {
        site: &site,
        state,
        origin: &origin,
        fixed_id: &fixed_id,
        request: &request,
        profiles: &profiles,
    };
    let strict = {
        #[cfg(any(test, feature = "test-support"))]
        if lost_channel.as_deref() == Some(request.channel_id.as_str()) {
            Err(scheduler(
                ScheduleError::ChannelNotWatched(request.channel_id.clone()).into(),
            ))
        } else {
            checked(state, &request, &ctx.directory)
        }
        #[cfg(not(any(test, feature = "test-support")))]
        checked(state, &request, &ctx.directory)
    };
    let timing = match strict {
        Ok(timing) => timing,
        Err(refusal) => return refusal_replay.recover(&mut ctx, refusal).await,
    };
    let replay = FixedPatchReplay::from_normalized(FixedPatchIdentity::new(
        fixed_id.clone(),
        &timing,
        &request,
    ));
    if first {
        return match state.writer.verify_fixed_patch_replay(origin, replay).await {
            Err(SchedulerError::AlreadyApplied { .. }) => {
                row(&site, state, &fixed_id, &profiles).await
            }
            Err(error) => Err(scheduler(error)),
            Ok(()) => Err(Refusal::from(ApiError::UNAVAILABLE)),
        };
    }
    // The week version the screen loaded: without it a stale full-body edit
    // would silently revert fields someone else changed (user decision).
    let version = version.expect("fresh request checked above");
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    let Some(current) = snapshot
        .fixed_runs
        .iter()
        .find(|fixed| fixed.id == fixed_id)
    else {
        return refusal_replay
            .recover(
                &mut ctx,
                Refusal::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "That weekly timing no longer exists.",
                ),
            )
            .await;
    };
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
    // `""` unpins a pinned owner (no change when already the default); a
    // member is pinned unless they already are (the form sends the choice on
    // every save).
    match timing.owner_id.as_deref() {
        Some("") if current.owner_pinned => {
            edit.owner_id = Some(String::new());
            fields.push("owner");
        }
        Some(owner_id)
            if !owner_id.is_empty() && !(current.owner_pinned && owner_id == current.owner_id) =>
        {
            edit.owner_id = Some(match rostered_owner(&ctx.directory, owner_id) {
                Ok(owner_id) => owner_id,
                Err(refusal) => return refusal_replay.recover(&mut ctx, refusal).await,
            });
            fields.push("owner");
        }
        _ => {}
    }
    if fields.is_empty() {
        if origin.request_id.is_some() {
            match state
                .writer
                .verify_fixed_patch_replay(origin.clone(), replay.clone())
                .await
            {
                Ok(()) => {}
                Err(SchedulerError::AlreadyApplied { .. }) => {
                    return row(&site, state, &fixed_id, &profiles).await;
                }
                Err(error) => return Err(scheduler(error)),
            }
        }
        return row(&site, state, &fixed_id, &profiles).await;
    }
    let mut choices = BTreeMap::new();
    for (run_id, decision) in &request.decisions {
        let choice = match decision.as_str() {
            "update" => AmendedRunChoice::UpdateToFixed,
            "keep" => AmendedRunChoice::KeepForThisWeek,
            _ => {
                return refusal_replay
                    .recover(
                        &mut ctx,
                        Refusal::invalid("Each decision is update or keep."),
                    )
                    .await;
            }
        };
        choices.insert(run_id.clone(), choice);
    }
    let fields: Vec<String> = fields.into_iter().map(str::to_owned).collect();
    let expect = match expectations(
        state.store.as_ref(),
        BlameTarget::FixedRun(fixed_id.clone()),
        &fields,
        Some(version),
        &Explicit {
            expect: request.expect.clone(),
            overrides: request.overrides.clone(),
        },
        origin.request_id.is_some(),
    )
    .await
    {
        Ok(expect) => expect,
        Err(refusal) => return refusal_replay.recover(&mut ctx, refusal).await,
    };
    let edit = FixedEditRequest {
        fixed_id: fixed_id.clone(),
        edit,
        // Amended runs need an explicit update/keep each; none given refuses
        // with `choices_required` rather than silently moving them (v4).
        choices: FixedEditChoices::PerRun(choices),
    };
    match state
        .writer
        .edit_fixed_patch(origin, expect, edit, replay, &ctx)
        .await
    {
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
    // Replays are recognised here, not by the scheduler: its retire digest
    // names the materialised weeks, which move at the weekly reset, so a
    // retry after it would otherwise read as a different request.
    if let Some(first) = recorded(state, &origin).await? {
        let retired_this = first.rows.iter().any(|row| {
            row.key == RowKey::FixedRun(fixed_id.clone())
                && row.before.is_some()
                && row.after.is_none()
        });
        return if retired_this {
            // The first attempt's count is not recorded; the replay reports none left.
            Ok(Json(Retired { cancelled: 0 }).into_response())
        } else {
            Err(scheduler(SchedulerError::IdempotencyMismatch {
                seq: first.seq,
            }))
        };
    }
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
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ValidateResult {
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
