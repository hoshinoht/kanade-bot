//! Limits and the delivery-owned manual digest trigger. The API projects live
//! state and delegates effects; it never talks to Discord itself.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, TimeDelta, Timelike, Utc};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use super::{
    context::{frames, state},
    replay::{Keyed, replayed},
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
    chat::pilot::{Allowance, AllowanceSnapshot, MemberUsage},
    domain::{
        members::MemberProfile,
        settings::{RowDiff, SettingsChange},
    },
    infrastructure::store::replays::{ReplayScope, StoredReplay},
};

/// One lock for the keyed effects that are not schedule writes (window
/// clears, the manual digest and header rewrite), held across the replay
/// lookup, the effect and its record. Their replays live in the store
/// (scope `limits`); the allowance itself is deliberately in-memory, and
/// delivery owns the durable no-duplicate guarantee for digest sends.
#[derive(Default)]
pub struct LimitsDesk {
    pub(super) lock: Mutex<()>,
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

pub(super) fn actor(session: &AdminSession) -> String {
    format!("{}:{}", session.actor.kind(), session.actor.id())
}

/// Record a keyed answer whose effect already happened outside the store.
/// A failed write cannot undo the effect, so the answer still goes out and
/// only a retry of this key would apply again.
pub(super) async fn record_after_effect(state: &ApiState, replay: StoredReplay) {
    // Store error text may carry paths, so only the event is logged.
    if state.store.put_replay(replay).await.is_err() {
        crate::runtime::logging::event("WARN", "idempotency_replay_unrecorded", json!({}));
    }
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

/// One member's window; the default allowance, unused, when it has no answers.
fn allowance(snapshot: &AllowanceSnapshot, member_id: &str) -> MemberUsage {
    snapshot
        .members
        .iter()
        .find(|usage| usage.member_id == member_id)
        .cloned()
        .unwrap_or_else(|| MemberUsage {
            member_id: member_id.to_owned(),
            used: 0,
            limit: snapshot.member_default.0,
            window_s: snapshot.member_default.1,
            resets_in_s: 0.0,
            overridden: false,
        })
}

/// The chat pilot's live allowance snapshot (the default allowance offline)
/// and the wall clock read right after it. The snapshot runs on monotonic
/// seconds; its offsets become instants only against this one pair, so a
/// system clock change cannot skew `resets_at`.
pub(super) fn allowance_snapshot(state: &ApiState) -> (AllowanceSnapshot, DateTime<Utc>) {
    let snapshot = state.chat.as_ref().map_or_else(
        || Allowance::default().snapshot(0.0),
        |chat| chat.allowance(),
    );
    (snapshot, state.now())
}

/// `at` plus `seconds`, rounded up to a whole second so a countdown to it
/// never reaches zero before the answer has actually left the window.
fn instant_after(at: DateTime<Utc>, seconds: f64) -> DateTime<Utc> {
    // `as` saturates; an absurd override window lands on the last instant.
    let millis = (seconds.max(0.0) * 1000.0).ceil() as i64;
    let later = TimeDelta::try_milliseconds(millis)
        .and_then(|offset| at.checked_add_signed(offset))
        .unwrap_or(DateTime::<Utc>::MAX_UTC);
    match later.with_nanosecond(0) {
        Some(whole) if whole < later => whole
            .checked_add_signed(TimeDelta::seconds(1))
            .unwrap_or(later),
        _ => later,
    }
}

/// One member's Limits allowance row; `None` without chatbot access (and for
/// bots). `at` is the wall clock paired with `snapshot`.
pub(super) fn allowance_row(
    state: &ApiState,
    snapshot: &AllowanceSnapshot,
    at: DateTime<Utc>,
    profile: &MemberProfile,
) -> Option<AllowanceRow> {
    let access = state.access.access(profile);
    (!profile.member.is_bot && access != "none").then(|| {
        let member = &profile.member;
        let member_id = member.user_id.clone();
        let member_name = member.name().unwrap_or(&member_id).to_owned();
        let staff = access == "staff";
        let usage = allowance(snapshot, &member_id);
        let used = if staff { 0 } else { usage.used };
        AllowanceRow {
            member: Named {
                id: member_id,
                name: member_name,
            },
            staff,
            allowance: (!staff).then_some(Quota {
                count: usage.limit,
                per_s: usage.window_s,
            }),
            used,
            overridden: usage.overridden,
            resets_at: (used > 0).then(|| iso_instant(instant_after(at, usage.resets_in_s))),
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
    let (snapshot, now) = allowance_snapshot(state);
    let allowances: Vec<AllowanceRow> = profiles
        .iter()
        .filter_map(|profile| allowance_row(state, &snapshot, now, profile))
        .collect();
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
    // Held across lookup, clear and record, keyed or not: two concurrent
    // clears of one window can never both see answers and both record.
    let _held = state.limits.lock.lock().await;
    let now = state.now();
    let keyed = key.map(|key| {
        let request = format!("limits.reset\u{1f}{member_id}");
        Keyed::new(ReplayScope::Limits, &session, key, &request)
    });
    if let Some(keyed) = &keyed
        && let Some(found) = keyed.recall(state.store.as_ref(), now).await?
    {
        return replayed(&found);
    }
    let profile = state
        .store
        .member(member_id.clone())
        .await
        .map_err(super::context::unavailable)?
        .ok_or(ApiError::NOT_FOUND)?;
    let name = profile.member.name().unwrap_or(&member_id).to_owned();
    let body = json!({"message": format!("{name}'s window is reset.")});
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::OK, &body, now));
    clear_window(state, &session, &member_id, &name, replay).await?;
    Ok(Json(body).into_response())
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

/// Clear the member's live window, then record it: History gets a row only
/// for an effective clear (answers in the window), so a refused reset or an
/// empty window records nothing. The row and the key's replay commit in one
/// transaction; the allowance itself is process memory outside the store,
/// so a write that fails after the reset leaves the window cleared and
/// unrecorded (a retry then finds it empty and records no History row).
async fn clear_window(
    state: &ApiState,
    session: &AdminSession,
    member_id: &str,
    name: &str,
    replay: Option<StoredReplay>,
) -> Result<(), Refusal> {
    let chat = state.chat.as_ref().ok_or(ApiError::UNAVAILABLE)?;
    let view = chat.limits().ok_or(ApiError::UNAVAILABLE)?;
    let MemberUsage {
        used,
        limit,
        window_s: per_s,
        overridden,
        ..
    } = allowance(&view.allowance, member_id);
    if !chat.reset_allowance(member_id) {
        return Err(ApiError::UNAVAILABLE.into());
    }
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
            .record_settings_change(change, replay)
            .await
            .map_err(super::context::unavailable)?;
    } else if let Some(replay) = replay {
        state
            .store
            .put_replay(replay)
            .await
            .map_err(super::context::unavailable)?;
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
    let now = state.now();
    // Held across the post, so a concurrent retry with this key replays.
    let mut keyed = None;
    let _held = match key {
        Some(key) => {
            let held = state.limits.lock.lock().await;
            let request = format!(
                "digest.post\u{1f}{}",
                json!({"week": body.week, "channel_id": body.channel_id})
            );
            let this = Keyed::new(ReplayScope::Limits, &session, key, &request);
            if let Some(found) = this.recall(state.store.as_ref(), now).await? {
                return replayed(&found);
            }
            keyed = Some(this);
            Some(held)
        }
        None => None,
    };
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
    let answer = json!({"message": format!(
        "Posted {label} digest in {}; people are named, not pinged.",
        channel.name
    )});
    digest_result(post(request).await)?;
    if let Some(keyed) = keyed {
        record_after_effect(state, keyed.answered(StatusCode::OK, &answer, now)).await;
    }
    Ok(Json(answer).into_response())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{instant_after, iso_instant};

    #[test]
    fn reset_instants_round_up_to_the_whole_second() {
        let at = Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap();
        let reset = |seconds: f64| iso_instant(instant_after(at, seconds));
        assert_eq!(reset(210.0), "2026-09-29T04:03:30Z");
        assert_eq!(reset(0.5), "2026-09-29T04:00:01Z");
        assert_eq!(reset(18_720.001), "2026-09-29T09:12:01Z");
        let mid = at + chrono::TimeDelta::milliseconds(400);
        assert_eq!(iso_instant(instant_after(mid, 1.0)), "2026-09-29T04:00:02Z");
        assert_eq!(iso_instant(instant_after(mid, 0.6)), "2026-09-29T04:00:01Z");
        // An absurd override window saturates instead of panicking.
        assert_eq!(instant_after(at, f64::MAX), DateTime::<Utc>::MAX_UTC);
    }
}
