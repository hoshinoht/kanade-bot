//! The signed-in member's weekly timings and their ownership actions (user
//! decision 2026-10-10): hand off, ask, accept, decline and withdraw. Rules
//! and writes are [`crate::api::ownership`], shared with Discord; on this
//! origin nobody acts as staff. Every write is admitted by [`super::write`]
//! (a member-write token and a required `Idempotency-Key`, request id
//! `public:<key>`) and, when it changes the owner, needs a fresh sign-in.

use std::sync::Arc;

use axum::{
    Json,
    extract::{
        Path as UrlPath, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;

use super::write::{admit, invalid_body, keyed_id, origin, path_id, snowflake};
use crate::{
    api::{
        admin::{
            context::{context, roster, unavailable},
            write::{Refusal, scheduler, state, write_context},
        },
        auth::{audit::AuditContext, member::MemberSession},
        dto::public::{
            MemberOwnerRequest, MemberTiming, MemberTimings, member_owner_request, member_timing,
            member_timings,
        },
        error::ApiError,
        listeners::Site,
        ownership::{self, OwnerDesk, OwnershipError},
        state::ApiState,
    },
    domain::{
        ownership::{OwnerRequest, OwnerRequestStatus, OwnershipRefusal},
        schedule::FixedRun,
        scheduler::Scope,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HandOff {
    to: String,
}

/// Status and code per refusal; the text is the member-facing rule.
fn refusal(error: OwnershipError) -> Refusal {
    use StatusCode as S;
    match error {
        OwnershipError::Refused(refused) => {
            let (status, code) = match refused {
                OwnershipRefusal::NotOwner => (S::FORBIDDEN, "not_owner"),
                OwnershipRefusal::NotRequester => (S::FORBIDDEN, "not_requester"),
                OwnershipRefusal::NotOnParty => (S::CONFLICT, "not_on_party"),
                OwnershipRefusal::AlreadyOwner => (S::CONFLICT, "already_owner"),
                OwnershipRefusal::Closed => (S::CONFLICT, "request_closed"),
                OwnershipRefusal::Expired => (S::CONFLICT, "request_expired"),
            };
            Refusal::new(status, code, refused.to_string())
        }
        OwnershipError::UnknownTiming | OwnershipError::UnknownRequest => {
            Refusal::new(S::NOT_FOUND, "not_found", error.to_string())
        }
        OwnershipError::AlreadyAsked => {
            Refusal::new(S::CONFLICT, "already_asked", error.to_string())
        }
        OwnershipError::Scheduler(error) => scheduler(error),
        OwnershipError::Store(_) => ApiError::UNAVAILABLE.into(),
    }
}

/// Whether `me` owns the timing `fixed_id` (a removed one is nobody's).
async fn owns(state: &ApiState, fixed_id: &str, me: &str) -> Result<bool, Refusal> {
    Ok(state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(unavailable)?
        .fixed_runs
        .iter()
        .any(|fixed| fixed.id == fixed_id && fixed.owner() == me))
}

async fn timing_view(
    site: &Site,
    state: &ApiState,
    fixed: &FixedRun,
    user_id: &str,
) -> Result<Json<MemberTiming>, Refusal> {
    let open = state
        .store
        .open_owner_requests()
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(site, state, roster(&profiles), state.now());
    Ok(Json(member_timing(&ctx, fixed, &open, user_id)))
}

async fn request_view(
    site: &Site,
    state: &ApiState,
    request: &OwnerRequest,
    user_id: &str,
) -> Result<Json<MemberOwnerRequest>, Refusal> {
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(site, state, roster(&profiles), state.now());
    Ok(Json(member_owner_request(&ctx, request, user_id)))
}

/// `GET /api/public/timings`: the weekly timings the member is on.
pub(super) async fn timings(
    State(site): State<Arc<Site>>,
    session: MemberSession,
) -> Result<Json<MemberTimings>, Refusal> {
    let state = state(&site)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(Vec::new()))
        .await
        .map_err(unavailable)?;
    let open = state
        .store
        .open_owner_requests()
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), state.now());
    Ok(Json(member_timings(
        &ctx,
        &snapshot.fixed_runs,
        &open,
        &session.user_id,
    )))
}

/// `POST /api/public/timings/{id}/owner` `{to}`: the owner hands the timing
/// to another party member at once.
pub(super) async fn hand_off(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<HandOff>, JsonRejection>,
) -> Result<Json<MemberTiming>, Refusal> {
    let (member, key) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let fixed_id = path_id(path)?;
    let Ok(Json(HandOff { to })) = body else {
        return Err(invalid_body());
    };
    if !snowflake(&to) {
        return Err(invalid_body());
    }
    let state = state(&site)?;
    let (ctx, _) = write_context(state).await?;
    let desk = OwnerDesk {
        store: &*state.store,
        writer: &*state.writer,
        ctx: &ctx,
    };
    // Only the owner hands off, and a completed hand-off moved the timing
    // away from them: owning it now means this call is the first attempt.
    let first = owns(state, &fixed_id, &session.user_id).await?;
    let change = desk
        .hand_off(
            origin(&session, key),
            &fixed_id,
            &session.user_id,
            false,
            &to,
            state.now(),
        )
        .await
        .map_err(refusal)?;
    // A replay by someone since taken off the party sees no timing view:
    // to them the timing is as unknown as a missing one. A first attempt
    // (an owner staff pinned off the party, say) committed, so it is
    // answered with the timing, never as not found.
    if !first && !change.fixed.participants.contains(&session.user_id) {
        return Err(refusal(OwnershipError::UnknownTiming));
    }
    timing_view(&site, state, &change.fixed, &session.user_id).await
}

/// The request `id` names, when it is this member's ask on `fixed_id`; the
/// key reused for another timing is `idempotency_mismatch`.
async fn asked(
    state: &ApiState,
    id: &str,
    member: &str,
    fixed_id: &str,
) -> Result<Option<OwnerRequest>, Refusal> {
    match state
        .store
        .owner_request(id.to_owned())
        .await
        .map_err(unavailable)?
    {
        Some(found) if found.requester == member && found.fixed_run_id == fixed_id => {
            Ok(Some(found))
        }
        Some(_) => Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency_mismatch",
            "That Idempotency-Key was already used for a different request.",
        )),
        None => Ok(None),
    }
}

/// `POST /api/public/timings/{id}/owner-requests`: a party member asks the
/// owner for the timing. `201` with the new request; a retry with the same
/// key answers `200` with that request as it is now.
pub(super) async fn ask(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<(StatusCode, Json<MemberOwnerRequest>), Refusal> {
    let (_, key) = admit(&site, &audit, &session, &headers)?;
    let fixed_id = path_id(path)?;
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let id = keyed_id(me, key);
    let (status, request) = match asked(state, &id, me, &fixed_id).await? {
        Some(found) => (StatusCode::OK, found),
        None => match ownership::ask(&*state.store, &fixed_id, me, id.clone(), state.now()).await {
            Ok((_, request)) => (StatusCode::CREATED, request),
            // A concurrent retry with the same key stored it first.
            Err(OwnershipError::AlreadyAsked) => match asked(state, &id, me, &fixed_id).await? {
                Some(found) => (StatusCode::OK, found),
                None => return Err(refusal(OwnershipError::AlreadyAsked)),
            },
            Err(error) => return Err(refusal(error)),
        },
    };
    Ok((status, request_view(&site, state, &request, me).await?))
}

/// The refusal for a member who does not own `request`'s timing. Only the
/// owner sees others' asks (the timings read), so to anyone but its
/// requester the request is as unknown as a missing one.
fn not_theirs(request: &OwnerRequest, me: &str) -> Refusal {
    refusal(if request.requester == me {
        OwnershipRefusal::NotOwner.into()
    } else {
        OwnershipError::UnknownRequest
    })
}

/// Accept or decline: the timing's owner only. A closed or expired request
/// answers its state to the owner alone; its requester learns only that they
/// may not decide it, anyone else that there is no such request. A former
/// owner who accepted it here (their lost-response retry) gets it as it now
/// is, after the accept's unfinished supersede; their decline is refused as
/// closed, as the owner's would be.
async fn decide(
    site: &Site,
    audit: &AuditContext,
    session: &MemberSession,
    headers: &HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    accept: bool,
) -> Result<Json<MemberOwnerRequest>, Refusal> {
    let (member, key) = admit(site, audit, session, headers)?;
    if accept {
        session.require_fresh(member.now())?;
    }
    let id = path_id(path)?;
    let state = state(site)?;
    let now = state.now();
    let me = session.user_id.as_str();
    let request = state
        .store
        .owner_request(id.clone())
        .await
        .map_err(unavailable)?
        .ok_or_else(|| refusal(OwnershipError::UnknownRequest))?;
    let (ctx, _) = write_context(state).await?;
    let desk = OwnerDesk {
        store: &*state.store,
        writer: &*state.writer,
        ctx: &ctx,
    };
    let acting = origin(session, key);
    if !request.live(now) && !owns(state, &request.fixed_run_id, me).await? {
        if !desk.accepted(&acting, &request).await.map_err(refusal)? {
            return Err(not_theirs(&request, me));
        }
        // An accept whose close never landed finishes through `decide`, and
        // so does every decline: the request is closed to them as to the
        // owner (`request_closed`), never answered as a success.
        if accept && request.status != OwnerRequestStatus::Open {
            desk.finish_accepted(acting, &request, now)
                .await
                .map_err(refusal)?;
            let current = state
                .store
                .owner_request(id)
                .await
                .map_err(unavailable)?
                .ok_or_else(|| refusal(OwnershipError::UnknownRequest))?;
            return request_view(site, state, &current, me).await;
        }
    }
    let (decided, _) = desk
        .decide(acting, &id, me, false, accept, now)
        .await
        .map_err(|error| match error {
            OwnershipError::Refused(OwnershipRefusal::NotOwner) => not_theirs(&request, me),
            error => refusal(error),
        })?;
    request_view(site, state, &decided, me).await
}

/// `POST /api/public/owner-requests/{id}/accept`: the owner hands the timing
/// to the requester.
pub(super) async fn accept(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Json<MemberOwnerRequest>, Refusal> {
    decide(&site, &audit, &session, &headers, path, true).await
}

/// `POST /api/public/owner-requests/{id}/decline`.
pub(super) async fn decline(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Json<MemberOwnerRequest>, Refusal> {
    decide(&site, &audit, &session, &headers, path, false).await
}

/// `POST /api/public/owner-requests/{id}/withdraw`: the requester only.
pub(super) async fn withdraw(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Json<MemberOwnerRequest>, Refusal> {
    admit(&site, &audit, &session, &headers)?;
    let id = path_id(path)?;
    let state = state(&site)?;
    let me = session.user_id.as_str();
    // Only its requester and the timing's owner see an ask.
    let asked = state
        .store
        .owner_request(id.clone())
        .await
        .map_err(unavailable)?
        .ok_or_else(|| refusal(OwnershipError::UnknownRequest))?;
    if asked.requester != me && !owns(state, &asked.fixed_run_id, me).await? {
        return Err(refusal(OwnershipError::UnknownRequest));
    }
    let request = ownership::withdraw(&*state.store, &id, me, state.now())
        .await
        .map_err(refusal)?;
    request_view(&site, state, &request, me).await
}
