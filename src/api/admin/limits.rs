//! Limits and the delivery-owned manual digest trigger. The API projects live
//! state and delegates effects; it never talks to Discord itself.

use std::{collections::VecDeque, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use super::{
    context::{frames, state},
    write::{Refusal, bad_body, origin},
};
use crate::{
    api::{
        auth::AdminSession,
        dto::{
            Named, iso_instant,
            limits::{AdmissionWindow, Allowance as AllowanceRow, Limits, Quota, group},
        },
        error::ApiError,
        listeners::Site,
        state::{ApiState, DigestPostRequest, DigestPostResult},
    },
    chat::pilot::{Allowance, AllowanceSnapshot},
    domain::{
        members::MemberProfile,
        settings::{RowDiff, SettingsChange},
    },
};

const REMEMBERED_KEYS: usize = 256;

#[derive(Clone)]
struct Remembered {
    actor: String,
    key: String,
    digest: String,
    message: String,
}

/// Small process-local replay memory for effects that are not schedule writes.
/// The underlying allowance is deliberately in-memory, and delivery owns the
/// durable no-duplicate guarantee for digest sends.
#[derive(Default)]
pub struct LimitsDesk {
    keys: Mutex<VecDeque<Remembered>>,
}

impl LimitsDesk {
    fn recall(keys: &VecDeque<Remembered>, actor: &str, key: &str) -> Option<Remembered> {
        keys.iter()
            .find(|entry| entry.actor == actor && entry.key == key)
            .cloned()
    }

    fn remember(keys: &mut VecDeque<Remembered>, entry: Remembered) {
        keys.push_back(entry);
        while keys.len() > REMEMBERED_KEYS {
            keys.pop_front();
        }
    }
}

type Reply = Result<axum::response::Response, Refusal>;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/limits", get(read))
        .route(
            "/api/admin/limits/windows/{id}",
            axum::routing::delete(reset_window),
        )
        .route("/api/admin/digest", post(digest))
}

fn actor(session: &AdminSession) -> String {
    format!("{}:{}", session.actor.kind(), session.actor.id())
}

fn mismatch() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "idempotency_mismatch",
        "That Idempotency-Key was already used for a different request.",
    )
}

fn message(message: String) -> axum::response::Response {
    Json(json!({"message": message})).into_response()
}

fn digest_result(result: DigestPostResult) -> Result<(), Refusal> {
    match result {
        DigestPostResult::Completed => Ok(()),
        DigestPostResult::NewerWeekAlreadyPosted => {
            Err(Refusal::invalid("A newer week's digest is already posted."))
        }
        DigestPostResult::Unavailable => Err(ApiError::UNAVAILABLE.into()),
    }
}

fn allowance(snapshot: &AllowanceSnapshot, member_id: &str) -> (usize, usize, f64, bool) {
    snapshot
        .members
        .iter()
        .find(|usage| usage.member_id == member_id)
        .map(|usage| (usage.used, usage.limit, usage.window_s, usage.overridden))
        .unwrap_or((
            0,
            snapshot.member_default.0,
            snapshot.member_default.1,
            false,
        ))
}

/// The chat pilot's live allowance snapshot (the default allowance offline).
pub(super) fn allowance_snapshot(state: &ApiState) -> AllowanceSnapshot {
    state.chat.as_ref().map_or_else(
        || Allowance::default().snapshot(0.0),
        |chat| chat.allowance(),
    )
}

/// One member's Limits allowance row; `None` without chatbot access (and for bots).
pub(super) fn allowance_row(
    state: &ApiState,
    snapshot: &AllowanceSnapshot,
    profile: &MemberProfile,
) -> Option<AllowanceRow> {
    let access = state.access.access(profile);
    (!profile.member.is_bot && access != "none").then(|| {
        let member = &profile.member;
        let member_id = member.user_id.clone();
        let member_name = member.name().unwrap_or(&member_id).to_owned();
        let staff = access == "staff";
        let (used, count, per_s, overridden) = allowance(snapshot, &member_id);
        AllowanceRow {
            member: Named {
                id: member_id,
                name: member_name,
            },
            staff,
            allowance: (!staff).then_some(Quota { count, per_s }),
            used: if staff { 0 } else { used },
            overridden,
        }
    })
}

async fn read(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let profiles = state
        .store
        .members()
        .await
        .map_err(super::context::unavailable)?;
    let snapshot = allowance_snapshot(state);
    let allowances: Vec<AllowanceRow> = profiles
        .iter()
        .filter_map(|profile| allowance_row(state, &snapshot, profile))
        .collect();
    let now = state.now();
    let groups = state
        .model_limits
        .as_ref()
        .map(|limits| limits(now).into_iter().map(group).collect())
        .unwrap_or_default();
    Ok(Json(Limits {
        groups,
        admission: AdmissionWindow::last_hour(),
        allowances,
        generated_at: iso_instant(now),
    })
    .into_response())
}

async fn reset_window(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(member_id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let actor = actor(&session);
    let digest = format!("limits.reset\u{1f}{member_id}");
    let profile = state
        .store
        .member(member_id.clone())
        .await
        .map_err(super::context::unavailable)?
        .ok_or(ApiError::NOT_FOUND)?;
    let name = profile.member.name().unwrap_or(&member_id).to_owned();
    let applied = format!("{name}'s window is reset.");
    // Held across snapshot, record and reset, keyed or not: two concurrent
    // clears of one window can never both see answers and both record.
    let mut keys = state.limits.keys.lock().await;
    let Some(key) = key else {
        clear_window(state, &session, &member_id, &name).await?;
        return Ok(message(applied));
    };
    if let Some(entry) = LimitsDesk::recall(&keys, &actor, &key) {
        return if entry.digest == digest {
            Ok(message(entry.message))
        } else {
            Err(mismatch())
        };
    }
    clear_window(state, &session, &member_id, &name).await?;
    LimitsDesk::remember(
        &mut keys,
        Remembered {
            actor,
            key,
            digest,
            message: applied.clone(),
        },
    );
    Ok(message(applied))
}

/// The History settings section of a cleared Limits window.
const CLEARED_SECTION: &str = "limits";

/// One window's facts as History stores them (`used` is what changes).
fn window_text(name: &str, used: usize, limit: usize, per_s: f64, overridden: bool) -> String {
    // Whole seconds read as integers, not `3600.0`.
    let per_s = if per_s.fract() == 0.0 && per_s.abs() < 1e15 {
        json!(per_s as i64)
    } else {
        json!(per_s)
    };
    json!({"member": name, "used": used, "limit": limit, "per_s": per_s, "overridden": overridden})
        .to_string()
}

/// Clear the member's live window. An effective clear (answers in the
/// window) is recorded in History first, so a failed record clears nothing;
/// an empty window is a no-op with no record.
async fn clear_window(
    state: &ApiState,
    session: &AdminSession,
    member_id: &str,
    name: &str,
) -> Result<(), Refusal> {
    let chat = state.chat.as_ref().ok_or(ApiError::UNAVAILABLE)?;
    let view = chat.limits().ok_or(ApiError::UNAVAILABLE)?;
    let (used, limit, per_s, overridden) = allowance(&view.allowance, member_id);
    if used > 0 {
        let origin = session.origin();
        let change = SettingsChange {
            id: 0,
            at: state.now(),
            actor: origin.actor,
            surface: origin.surface,
            section: CLEARED_SECTION.into(),
            revision: 0,
            values: [(
                format!("window.{member_id}"),
                RowDiff {
                    from: window_text(name, used, limit, per_s, overridden),
                    to: window_text(name, 0, limit, per_s, overridden),
                },
            )]
            .into(),
        };
        state
            .store
            .record_settings_change(change)
            .await
            .map_err(super::context::unavailable)?;
    }
    if !chat.reset_allowance(member_id) {
        return Err(ApiError::UNAVAILABLE.into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DigestBody {
    week: String,
    channel_id: Option<String>,
}

async fn digest(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<DigestBody>, JsonRejection>,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let Json(body) = body.map_err(bad_body)?;
    let actor = actor(&session);
    let digest = format!(
        "digest.post\u{1f}{}\u{1f}{}",
        body.week,
        body.channel_id.as_deref().unwrap_or_default()
    );
    let now = state.now();
    let [this, next] = frames(state, now)?;
    let (week_start, label) = match body.week.as_str() {
        "this" => (this.start, "this week's"),
        "next" => (next.start, "next week's"),
        _ => return Err(Refusal::invalid("Week is this or next.")),
    };
    let channel_id = match body.channel_id {
        Some(channel) => channel,
        None => state
            .config
            .as_ref()
            .ok_or(ApiError::UNAVAILABLE)?
            .settings()
            .await
            .posting
            .channel_id
            .ok_or_else(|| Refusal::invalid("Choose a digest channel."))?,
    };
    let channel = state
        .channels
        .channels()
        .into_iter()
        .find(|channel| channel.id == channel_id)
        .ok_or_else(|| Refusal::invalid("Choose a current Discord channel."))?;
    let post = state.digest_post.as_ref().ok_or(ApiError::UNAVAILABLE)?;
    let request = DigestPostRequest {
        week_start,
        channel_id: Some(channel_id),
        at: now,
    };
    let applied = format!(
        "Posted {label} digest in {}; people are named, not pinged.",
        channel.name
    );
    if let Some(key) = key {
        let mut keys = state.limits.keys.lock().await;
        if let Some(entry) = LimitsDesk::recall(&keys, &actor, &key) {
            return if entry.digest == digest {
                Ok(message(entry.message))
            } else {
                Err(mismatch())
            };
        }
        digest_result(post(request).await)?;
        LimitsDesk::remember(
            &mut keys,
            Remembered {
                actor,
                key,
                digest,
                message: applied.clone(),
            },
        );
        return Ok(message(applied));
    }
    digest_result(post(request).await)?;
    Ok(message(applied))
}
