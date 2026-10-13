//! The member's own MapleScouter lookups (module `character_data` with
//! `KANADE_SCOUTER_URL`): one of their characters at a time, through the
//! one queue the admins use (behind every lookup already waiting). A
//! member starts at most one lookup per 30 minutes; a lookup that fails
//! gives the slot back, and joining a lookup already queued or running (or
//! one answered from the 24-hour memory) never takes it. A member sees
//! only lookups of their own characters: another job is the generic `404`,
//! and the running job is shown only when it is theirs.

use std::sync::Arc;

use axum::{
    Json,
    extract::{
        Path as UrlPath, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::{module, own, roster};
use crate::{
    api::{
        admin::{
            character_data::estimates::lookup_name,
            write::{Refusal, state},
        },
        auth::{
            audit::{AuditContext, AuditEvent},
            member::MemberSession,
            rate::MEMBER_ESTIMATE_ROUTE,
            wire,
        },
        dto::{
            estimates::{EstimateStarted, EstimateStatus, job},
            iso_instant,
            public::MemberEstimateLimit,
        },
        error::ApiError,
        listeners::Site,
        public::write::{invalid_body, path_id},
    },
    domain::history::Actor,
    infrastructure::scouter::{
        Estimates, Job, JobRequest, JobState, MAX_QUEUED, MissingReason, Start, Subject, Target,
    },
};

type Reply = Result<Response, Refusal>;

fn estimates(site: &Site) -> Result<&Estimates, Refusal> {
    module(state(site)?)?
        .estimates()
        .ok_or_else(|| ApiError::NOT_FOUND.into())
}

/// A job of `me`'s own characters only (whoever started it).
fn theirs(job: &Job, me: &str) -> bool {
    matches!(job.target, Target::Characters(_))
        && !job.subjects.is_empty()
        && job.subjects.iter().all(|subject| subject.member_id == me)
}

/// `MemberEstimateRequest`: one of the caller's characters, and `refresh`
/// to skip the 24-hour memory ("Look up again").
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartBody {
    character_id: String,
    #[serde(default)]
    refresh: bool,
}

fn accepted(job_row: crate::api::dto::estimates::EstimateJob) -> Response {
    (StatusCode::ACCEPTED, Json(EstimateStarted { job: job_row })).into_response()
}

/// `POST /api/public/me/estimates` → `202 {job}`; `429 member_limited`
/// (`MemberEstimateLimit`, with `next_at`) within 30 minutes of the
/// caller's last counted start, `429 estimate_queue_full` past 16 waiting.
pub(super) async fn start(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    body: Result<Json<StartBody>, JsonRejection>,
) -> Reply {
    let Ok(Json(body)) = body else {
        return Err(invalid_body());
    };
    let member = site.member.as_deref().ok_or(ApiError::AUTH_UNAVAILABLE)?;
    let state = state(&site)?;
    let estimates = estimates(&site)?;
    let me = session.user_id.as_str();
    let character = own(roster(state)?.as_ref(), me, body.character_id.trim()).await?;
    let ign = lookup_name(&character).map_err(|reason| match reason {
        MissingReason::OtherRegion => Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "region",
            "MapleScouter covers MapleSEA characters only.",
        ),
        _ => Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_ign",
            "MapleScouter can't look up this IGN.",
        ),
    })?;
    let target = Target::Characters(vec![character.id.clone()]);
    // Already waiting or running: that lookup, at no cost.
    if let Some(found) = estimates.latest(&target)
        && matches!(found.state(), JobState::Queued | JobState::Running)
    {
        return Ok(accepted(job(&found, &state.catalog)));
    }
    let now = member.now();
    let failed = |id: &str| {
        estimates
            .job(id)
            .is_some_and(|found| found.state() == JobState::Failed)
    };
    if let Err(next_at) = member.rate().claim_estimate(me, now, failed) {
        member.audit(
            &audit,
            AuditEvent::RateLimited {
                route: MEMBER_ESTIMATE_ROUTE,
            },
        );
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            Json(MemberEstimateLimit {
                error: "member_limited",
                message: "You can start one lookup every 30 minutes.".into(),
                next_at: iso_instant(next_at),
            }),
        )
            .into_response());
    }
    let started = estimates.start(JobRequest {
        target,
        subjects: vec![Subject {
            character_id: character.id.clone(),
            member_id: character.member_id.clone(),
            ign,
        }],
        missing: Vec::new(),
        started_by: Actor::member(me),
        refresh: body.refresh,
    });
    match started {
        Start::Started(found) => {
            // Answered from memory at once: no lookup ran, nothing counts.
            let counted = found.state() != JobState::Done
                || found
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.as_ref().is_ok_and(|done| !done.cache_hit));
            member
                .rate()
                .settle_estimate(me, counted.then_some(found.id.as_str()));
            Ok(accepted(job(&found, &state.catalog)))
        }
        Start::Joined(found) => {
            member.rate().settle_estimate(me, None);
            Ok(accepted(job(&found, &state.catalog)))
        }
        Start::Full => {
            member.rate().settle_estimate(me, None);
            Err(Refusal::new(
                StatusCode::TOO_MANY_REQUESTS,
                "estimate_queue_full",
                format!("{MAX_QUEUED} lookups are waiting already. Try again in a few minutes."),
            ))
        }
    }
}

/// `GET /api/public/me/estimates?character_id=<id>` → `EstimateStatus`:
/// that character's newest lookup, and the running one when it is theirs.
pub(super) async fn status(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    uri: Uri,
) -> Reply {
    let state = state(&site)?;
    let estimates = estimates(&site)?;
    let me = session.user_id.as_str();
    let pairs = wire::query_pairs(uri.query());
    let id = match (pairs.len(), wire::query_value(&pairs, "character_id")) {
        (1, Some(id)) => id,
        _ => return Err(Refusal::invalid("Ask with character_id=<id>.")),
    };
    let character = own(roster(state)?.as_ref(), me, id.trim()).await?;
    let target = Target::Characters(vec![character.id]);
    Ok(Json(EstimateStatus {
        job: estimates
            .latest(&target)
            .map(|found| job(&found, &state.catalog)),
        running: estimates
            .running()
            .filter(|running| theirs(running, me))
            .map(|running| job(&running, &state.catalog)),
    })
    .into_response())
}

/// `GET /api/public/me/estimates/{job}` → `EstimateJob`, only for a
/// lookup of the caller's own characters.
pub(super) async fn read(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Reply {
    let id = path_id(path)?;
    let state = state(&site)?;
    let found = estimates(&site)?
        .job(&id)
        .filter(|found| theirs(found, &session.user_id))
        .ok_or(ApiError::NOT_FOUND)?;
    Ok(Json(job(&found, &state.catalog)).into_response())
}
