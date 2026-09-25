//! Approve and reject, by source: member requests as the session's admin,
//! proposals as the signed-in Discord user with admin authority (the same
//! record their ✅ writes). Every decision is naturally repeatable; the
//! `Idempotency-Key` is validated, not stored.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Days, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{
    super::write::{Refusal, bad_body, origin, state, strict_time, write_context},
    refusal::{
        self, discord_session_required, edit_not_applicable, not_found, stale, unprocessable,
    },
};
use crate::{
    api::{auth::AdminSession, dto::inbox::kind_label, listeners::Site, state::ApiState},
    domain::{
        drafts::{DraftKind, DraftOp, DraftStatus, LoadedDraft},
        history::Actor,
        ids::short_id,
        members::MemberProfile,
        proposals::{Approver, ProposalSubject},
        requests::{RequestType, check_reason},
        schedule::{AmendedRunChoice, FixedEditChoices, utc_instant},
        scheduler::{DraftError, ProposalError, RequestError},
    },
};

type Reply = Result<Response, Refusal>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveBody {
    version: Option<u64>,
    /// Amended run id → `update` | `keep` (`change_fixed` requests).
    choices: Option<BTreeMap<String, String>>,
    /// Edit, then approve: a day (0–6) of the proposed time's boss week.
    day: Option<u8>,
    time: Option<String>,
    /// Refused when true: conflicts always block.
    force: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectBody {
    version: Option<u64>,
    reason: Option<String>,
}

fn message(text: String) -> Response {
    Json(json!({ "message": text })).into_response()
}

fn kind_of(loaded: &LoadedDraft) -> &'static str {
    let draft = &loaded.draft;
    match draft.kind {
        DraftKind::Request => draft
            .request_type
            .as_deref()
            .and_then(RequestType::parse)
            .map_or("change", RequestType::as_str),
        _ => draft
            .subject
            .as_deref()
            .and_then(ProposalSubject::parse)
            .map_or("change", |subject| subject.kind.as_str()),
    }
}

fn decided(verb: &str, loaded: &LoadedDraft) -> String {
    format!(
        "{verb}: {} #{}.",
        kind_label(kind_of(loaded)).to_lowercase(),
        short_id(&loaded.draft.id)
    )
}

async fn load(state: &ApiState, id: &str) -> Result<LoadedDraft, Refusal> {
    match state.store.draft(id.to_owned()).await {
        Ok(Some(loaded)) if loaded.draft.kind != DraftKind::Admin => Ok(loaded),
        Ok(_) => Err(not_found()),
        Err(_) => Err(crate::api::error::ApiError::UNAVAILABLE.into()),
    }
}

fn version_required() -> Refusal {
    unprocessable(
        "version_required",
        "Name the version you reviewed (`version`).",
    )
}

/// The admin as a proposal approver: their ✅ authority, by Discord id.
fn approver(session: &AdminSession, profiles: &[MemberProfile]) -> Result<Approver, Refusal> {
    let user_id = session
        .discord_user()
        .ok_or_else(discord_session_required)?;
    Ok(Approver {
        user_id: user_id.to_owned(),
        has_role: profiles.iter().any(|profile| {
            profile.member.user_id == user_id && profile.member.has_role && !profile.member.is_bot
        }),
        is_admin: true,
    })
}

fn choices(given: BTreeMap<String, String>) -> Result<FixedEditChoices, Refusal> {
    let mut picks = BTreeMap::new();
    for (run_id, pick) in given {
        let pick = match pick.as_str() {
            "update" => AmendedRunChoice::UpdateToFixed,
            "keep" => AmendedRunChoice::KeepForThisWeek,
            _ => return Err(Refusal::invalid("Each choice is update or keep.")),
        };
        picks.insert(run_id, pick);
    }
    Ok(FixedEditChoices::PerRun(picks))
}

/// `day`/`time` in the boss week of the proposal's scheduled instant.
fn edited_time(
    state: &ApiState,
    loaded: &LoadedDraft,
    day: u8,
    time: &str,
) -> Result<DateTime<Utc>, Refusal> {
    let proposed = loaded
        .draft_ops()
        .iter()
        .find_map(|op| match op {
            DraftOp::AmendRun { to, .. } => Some(*to),
            DraftOp::CreateRun { datetime, .. } => Some(*datetime),
            _ => None,
        })
        .ok_or_else(edit_not_applicable)?;
    if day > 6 {
        return Err(Refusal::invalid("A boss week has seven days."));
    }
    let time =
        strict_time(time).ok_or_else(|| Refusal::invalid("Times are HH:MM, 00:00 to 23:59."))?;
    let out_of_range = |_| Refusal::invalid("That date is out of range.");
    let policy = &state.policy;
    let zone = policy.zone();
    let week = policy
        .week_of(&proposed)
        .and_then(|start| utc_instant(&start))
        .map_err(out_of_range)?;
    let date = zone.from_utc_datetime(&week.naive_utc()).date_naive() + Days::new(u64::from(day));
    let to = zone
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .ok_or_else(|| Refusal::invalid("That time does not exist on that day."))?
        .with_timezone(&Utc);
    let landed = policy
        .week_of(&to)
        .and_then(|start| utc_instant(&start))
        .map_err(out_of_range)?;
    if landed != week {
        return Err(Refusal::invalid(
            "That time falls in another boss week; pick a day of the proposed week.",
        ));
    }
    Ok(to)
}

pub async fn approve(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ApproveBody>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    origin(&session, &headers)?;
    if body.force == Some(true) {
        return Err(unprocessable(
            "force_unsupported",
            "Conflicts cannot be approved over; reject it or change the schedule first.",
        ));
    }
    let loaded = load(state, &id).await?;
    let edited = body.day.is_some() || body.time.is_some();
    let (ctx, profiles) = write_context(state).await?;
    if loaded.draft.kind == DraftKind::Request {
        if edited {
            return Err(edit_not_applicable());
        }
        let version = body.version.ok_or_else(version_required)?;
        let choices = body.choices.map(choices).transpose()?;
        return match state
            .writer
            .approve_request(&session.actor, &id, version, choices, &ctx)
            .await
        {
            // The store enqueued the merge and requester notices with the merge.
            Ok(_) | Err(RequestError::Draft(DraftError::AlreadyApplied { .. })) => {
                Ok(message(decided("Approved", &loaded)))
            }
            Err(error) => Err(refusal::request(error)),
        };
    }
    let approver = approver(&session, &profiles)?;
    if body.choices.is_some() {
        return Err(unprocessable(
            "choices_not_applicable",
            "Only weekly-timing change requests take per-run choices.",
        ));
    }
    // A closed proposal is left to the domain: a replay answers the first result.
    if loaded.draft.status.is_live() && body.version.is_some_and(|v| v != loaded.draft.version) {
        return Err(stale());
    }
    let edit = match (body.day, body.time.as_deref()) {
        (None, None) => None,
        (Some(day), Some(time)) => Some(edited_time(state, &loaded, day, time)?),
        _ => return Err(Refusal::invalid("An edit needs a day and a time.")),
    };
    match state
        .writer
        .approve_proposal(&id, &approver, edit, &ctx)
        .await
    {
        Ok(done) if !done.follow_up_errors.is_empty() => Ok(message(format!(
            "{} A follow-up did not finish; approve again to retry it.",
            decided("Approved", &loaded)
        ))),
        Ok(_) | Err(ProposalError::Draft(DraftError::AlreadyApplied { .. })) => {
            Ok(message(decided("Approved", &loaded)))
        }
        Err(error) => Err(refusal::proposal(error)),
    }
}

pub async fn reject(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<RejectBody>, JsonRejection>,
) -> Reply {
    let Json(body) = body.map_err(bad_body)?;
    let state = state(&site)?;
    origin(&session, &headers)?;
    let loaded = load(state, &id).await?;
    let draft = &loaded.draft;
    if draft.kind == DraftKind::Request {
        let version = body.version.ok_or_else(version_required)?;
        let reason = check_reason(body.reason.as_deref().unwrap_or_default())
            .map_err(|refused| refusal::request(refused.into()))?;
        // A retry of this admin's own rejection answers as the first did.
        if draft.status == DraftStatus::Rejected && draft.closed_by.as_ref() == Some(&session.actor)
        {
            return if draft.close_reason.as_deref() == Some(reason.as_str()) {
                Ok(message(decided("Rejected", &loaded)))
            } else {
                Err(refusal::draft(DraftError::IdempotencyMismatch { seq: 0 }))
            };
        }
        return match state
            .writer
            .reject_request(&session.actor, &id, version, &reason)
            .await
        {
            Ok(_) => Ok(message(decided("Rejected", &loaded))),
            Err(error) => Err(refusal::request(error)),
        };
    }
    let (_, profiles) = write_context(state).await?;
    let approver = approver(&session, &profiles)?;
    if body
        .reason
        .as_deref()
        .is_some_and(|reason| !reason.trim().is_empty())
    {
        return Err(unprocessable(
            "reason_not_applicable",
            "Kanade's proposals are rejected without a reason; nothing would keep it.",
        ));
    }
    if draft.status.is_live() && body.version.is_some_and(|v| v != draft.version) {
        return Err(stale());
    }
    if draft.status == DraftStatus::Rejected
        && draft.closed_by.as_ref() == Some(&Actor::member(approver.user_id.clone()))
    {
        return Ok(message(decided("Rejected", &loaded)));
    }
    match state.writer.reject_proposal(&id, &approver).await {
        Ok(_) => Ok(message(decided("Rejected", &loaded))),
        // Past its TTL: it is closed now, which is what rejecting wanted.
        Err(ProposalError::Expired) => Ok(message(format!(
            "That proposal had expired; it is closed (#{}).",
            short_id(&id)
        ))),
        Err(error) => Err(refusal::proposal(error)),
    }
}
