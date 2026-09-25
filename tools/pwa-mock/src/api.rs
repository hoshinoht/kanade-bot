//! JSON handlers for both origins. The public origin only mounts `public_week`
//! and `identity`; authorization is by which router a route is mounted on.

use crate::{
    App,
    mock::{
        MoveError, Store,
        dto::*,
        extractions::RescanRequest,
        history::{Actor, Mode},
        inbox::ApproveRequest,
    },
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct WeekQuery {
    week: Option<String>,
}

impl WeekQuery {
    fn next(&self) -> bool {
        self.week.as_deref() == Some("next")
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

/// While the public portal is closed the public origin serves only the shell,
/// this status, and the bot identity; data and art answer with this.
pub fn closed() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "closed",
        "The schedule is not public right now.",
    )
}

/// Minimal public status: the closed page needs nothing else to render.
pub async fn public_status(State(app): State<App>) -> Response {
    Json(
        json!({ "portal": if app.store.lock().await.public_portal() { "open" } else { "closed" } }),
    )
    .into_response()
}

fn outcome<T: serde::Serialize>(result: Result<T, MoveError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(MoveError::NotFound) => error(
            StatusCode::NOT_FOUND,
            "not_found",
            "That run no longer exists.",
        ),
        Err(MoveError::Stale) => error(
            StatusCode::CONFLICT,
            "stale",
            "The week changed since it was loaded.",
        ),
        Err(MoveError::Invalid(message)) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, "invalid", &message)
        }
        Err(MoveError::Coded(status, code, message)) => error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY),
            code,
            &message,
        ),
    }
}

pub async fn week(State(app): State<App>, Query(q): Query<WeekQuery>) -> Response {
    Json(app.store.lock().await.week(q.next())).into_response()
}

pub async fn public_week(State(app): State<App>, Query(q): Query<WeekQuery>) -> Response {
    let store = app.store.lock().await;
    // The admin's public-portal switch closes the public schedule outright.
    if !store.public_portal() {
        return closed();
    }
    Json(store.public_week(q.next())).into_response()
}

pub async fn stats(State(app): State<App>, Query(q): Query<WeekQuery>) -> Response {
    Json(app.store.lock().await.stats(q.next())).into_response()
}

pub async fn summary(State(app): State<App>) -> Response {
    Json(app.store.lock().await.summary()).into_response()
}

pub async fn members(State(app): State<App>) -> Response {
    Json(app.store.lock().await.member_rows()).into_response()
}

pub async fn channels() -> Response {
    Json(Store::channels()).into_response()
}

/// Guild roles, highest first, `@everyone` left out (the server reads the gateway cache).
pub async fn roles() -> Response {
    Json(json!([
        { "id": "300001", "name": "staff", "color": "#e0a458" },
        { "id": "300003", "name": "bossers", "color": "#5b8def" },
        { "id": "300002", "name": "newbies" },
    ]))
    .into_response()
}

pub async fn personas() -> Response {
    Json(Store::personas()).into_response()
}

pub async fn patch_member(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<MemberPatch>,
) -> Response {
    outcome(app.store.lock().await.patch_member(&id, req))
}

pub async fn add_alias(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<AliasRequest>,
) -> Response {
    outcome(app.store.lock().await.add_alias(&id, &req.alias))
}

pub async fn fixed(State(app): State<App>) -> Response {
    Json(app.store.lock().await.fixed_rows()).into_response()
}

pub async fn create_fixed(State(app): State<App>, Json(req): Json<FixedRequest>) -> Response {
    outcome(app.store.lock().await.portal(|s| s.create_fixed(req)))
}

pub async fn update_fixed(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<FixedRequest>,
) -> Response {
    outcome(app.store.lock().await.portal(|s| s.update_fixed(&id, req)))
}

pub async fn retire_fixed(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .portal(|s| s.retire_fixed(&id))
            .map(|cancelled| json!({ "cancelled": cancelled })),
    )
}

pub async fn validate_bosses(State(app): State<App>, Json(req): Json<ValidateRequest>) -> Response {
    outcome(app.store.lock().await.validate_bosses(&req.text))
}

pub async fn bosses(State(app): State<App>) -> Response {
    Json(app.store.lock().await.boss_rows()).into_response()
}

pub async fn reminders(State(app): State<App>) -> Response {
    Json(app.store.lock().await.reminders()).into_response()
}

#[derive(Deserialize)]
pub struct VersionBody {
    version: u64,
}

pub async fn reset_run(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<VersionBody>,
) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .portal(|s| s.reset_to_fixed(&id, req.version)),
    )
}

/// Mock stand-in for the authenticated caller; carries the CSRF token like the server.
pub async fn session(State(app): State<App>) -> Response {
    let (display, method) = {
        let store = app.store.lock().await;
        (store.session_display(), store.session_method())
    };
    (
        [(crate::writes::CSRF_HEADER, app.writes.token())],
        Json(json!({ "display": display, "method": method })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct SessionMethod {
    method: String,
}

/// `POST /__mock/session {method}`: sign in again as `discord`, `token` or
/// `tailscale` (a new CSRF token, as a real sign-in), or sign out with
/// `none`, so e2e can cover the Discord-only rule and the sign-in flow.
pub async fn switch_session(State(app): State<App>, Json(req): Json<SessionMethod>) -> Response {
    if !app.store.lock().await.set_session(&req.method) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "Method is discord, token, tailscale or none.",
        );
    }
    app.writes.rotate();
    StatusCode::NO_CONTENT.into_response()
}

pub async fn move_run(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<MoveRequest>,
) -> Response {
    outcome(app.store.lock().await.portal(|s| s.move_run(&id, req)))
}

pub async fn status(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<StatusRequest>,
) -> Response {
    outcome(app.store.lock().await.portal(|s| s.set_status(&id, req)))
}

pub async fn rsvp(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<RsvpRequest>,
) -> Response {
    outcome(app.store.lock().await.portal(|s| s.rsvp(&id, req)))
}

pub async fn participants(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<ParticipantsRequest>,
) -> Response {
    outcome(app.store.lock().await.portal(|s| s.participants(&id, req)))
}

pub async fn ping(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .ping(&id)
            .map(|message| json!({ "message": message })),
    )
}

pub async fn reset(State(app): State<App>) -> StatusCode {
    app.store.lock().await.reset();
    app.writes.forget().await;
    StatusCode::NO_CONTENT
}

pub async fn not_found() -> Response {
    error(
        StatusCode::NOT_FOUND,
        "not_found",
        "No such endpoint on this origin.",
    )
}

pub async fn knowledge_v2(State(app): State<App>, Path(key): Path<String>) -> Response {
    outcome(app.store.lock().await.knowledge_v2(&app.knowledge, &key))
}

pub async fn events(State(app): State<App>) -> Response {
    Json(app.knowledge.events()).into_response()
}

pub async fn inbox(State(app): State<App>) -> Response {
    Json(app.store.lock().await.inbox()).into_response()
}

pub async fn approve(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(req): Json<ApproveRequest>,
) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .approve_tracked(&id, req)
            .map(|message| json!({ "message": message })),
    )
}

pub async fn reject(
    State(app): State<App>,
    Path(id): Path<String>,
    body: Option<Json<crate::mock::inbox::RejectRequest>>,
) -> Response {
    let req = body.map(|Json(r)| r).unwrap_or_default();
    outcome(
        app.store
            .lock()
            .await
            .reject(&id, req)
            .map(|message| json!({ "message": message })),
    )
}

pub async fn extractions(
    State(app): State<App>,
    Query(q): Query<crate::mock::logfilter::LogQuery>,
) -> Response {
    outcome(app.store.lock().await.extractions(&q))
}

pub async fn extraction(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.extraction(&id))
}

pub async fn rescan_targets() -> Response {
    Json(Store::rescan_targets()).into_response()
}

pub async fn start_rescan(State(app): State<App>, Json(req): Json<RescanRequest>) -> Response {
    outcome(app.store.lock().await.start_rescan(req))
}

pub async fn poll_rescan(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.poll_rescan(&id))
}

pub async fn cancel_rescan(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.cancel_rescan(&id))
}

pub async fn chat(
    State(app): State<App>,
    Query(q): Query<crate::mock::logfilter::LogQuery>,
) -> Response {
    outcome(app.store.lock().await.chat(&q))
}

pub async fn chat_turn(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.chat_turn(&id))
}

pub async fn limits(State(app): State<App>) -> Response {
    Json(app.store.lock().await.limits()).into_response()
}

pub async fn reset_window(State(app): State<App>, Path(id): Path<String>) -> Response {
    outcome(app.store.lock().await.reset_window(&id))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    week: Option<String>,
    actor: Option<String>,
    before: Option<u64>,
    limit: Option<usize>,
}

fn actor(text: &str) -> Option<Actor> {
    let (kind, id) = text.split_once(':')?;
    matches!(kind, "member" | "admin" | "system").then(|| Actor::new(kind, id))
}

pub async fn history(State(app): State<App>, Query(q): Query<HistoryQuery>) -> Response {
    let actor = q.actor.as_deref().and_then(actor);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    Json(
        app.store
            .lock()
            .await
            .history_page(q.week.as_deref(), actor.as_ref(), q.before, limit),
    )
    .into_response()
}

pub async fn history_record(State(app): State<App>, Path(seq): Path<u64>) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .record(seq)
            .filter(|r| r.seq > 0)
            .ok_or(MoveError::NotFound),
    )
}

#[derive(Deserialize)]
pub struct RevertRequest {
    seqs: Vec<u64>,
    #[serde(flatten)]
    mode: Mode,
}

pub async fn revert(State(app): State<App>, Json(req): Json<RevertRequest>) -> Response {
    outcome(app.store.lock().await.revert_changes(req.seqs, &req.mode))
}

#[derive(Deserialize)]
pub struct RestoreRequest {
    week: String,
    revision: u64,
    #[serde(flatten)]
    mode: Mode,
}

pub async fn restore_week(State(app): State<App>, Json(req): Json<RestoreRequest>) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .restore_week_to(&req.week, req.revision, &req.mode),
    )
}

#[derive(Deserialize)]
pub struct ActorRevertRequest {
    actor: String,
    since: String,
    #[serde(flatten)]
    mode: Mode,
}

pub async fn revert_actor(State(app): State<App>, Json(req): Json<ActorRevertRequest>) -> Response {
    let Some(who) = actor(&req.actor) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "Actor is kind:id, e.g. member:1005.",
        );
    };
    outcome(
        app.store
            .lock()
            .await
            .revert_by_actor(&who, &req.since, &req.mode),
    )
}

pub async fn checkpoints(State(app): State<App>) -> Response {
    Json(app.store.lock().await.checkpoints()).into_response()
}

pub async fn blame(State(app): State<App>, Path(id): Path<String>) -> Response {
    Json(app.store.lock().await.blame(&id)).into_response()
}

pub async fn config(State(app): State<App>) -> Response {
    Json(app.store.lock().await.config_view()).into_response()
}

pub async fn patch_config(
    State(app): State<App>,
    Json(patch): Json<serde_json::Value>,
) -> Response {
    outcome(app.store.lock().await.patch_config(&patch))
}

#[derive(Deserialize)]
pub struct DigestRequest {
    week: String,
    channel_id: Option<String>,
}

pub async fn reload_profiles(State(app): State<App>) -> Response {
    Json(app.store.lock().await.reload_profiles()).into_response()
}

pub async fn digest(State(app): State<App>, Json(req): Json<DigestRequest>) -> Response {
    outcome(
        app.store
            .lock()
            .await
            .post_digest(&req.week, req.channel_id.as_deref()),
    )
}

pub async fn access(State(app): State<App>) -> Response {
    Json(app.store.lock().await.access()).into_response()
}
