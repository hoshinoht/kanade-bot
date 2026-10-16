//! Guide writes: create, save and delete the draft; publish, unpublish and
//! search (each queues a run for the worker); cancel a run; "Mark as not
//! posted". Each answer is known before its write, so a keyed request
//! stores exactly what it answered (in the write's transaction) and a
//! retry replays it; a refused write records nothing.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    Guides, Refusal, Renderer, Reply, answer, blocks_refused, channel_taken, not_found, publishing,
    read::{PlanKind, Planned},
    stale,
};
use crate::{
    api::{
        admin::{
            replay::{Keyed, mismatch, replayed},
            write::Body,
        },
        auth::AdminSession,
        dto::{
            guides::{self as dto, GuideBlock, GuideConfirmations, GuideNeedsConfirmation},
            iso_instant,
        },
        listeners::Site,
    },
    domain::{
        guides::{
            Block, Confirmations, Guide, GuideChange, GuideRecord, GuideWrite, NotPosted,
            PlanCounts, Publish, PublishKind, PublishRequest, PublishRequested, PublishState,
            SlotState, plan_hash,
        },
        ids::{IdGenerator, RandomIds},
    },
    infrastructure::store::replays::KeyedWrite,
};

/// Who writes at `now`, for the History record.
fn record(session: &AdminSession, now: DateTime<Utc>) -> GuideRecord {
    let origin = session.origin();
    GuideRecord {
        at: now,
        actor: origin.actor,
        surface: origin.surface,
    }
}

fn actor_text(session: &AdminSession) -> String {
    format!("{}:{}", session.actor.kind(), session.actor.id())
}

/// The request's `Idempotency-Key`, if any, for the normalized `request`,
/// and the answer it already has.
async fn recall(
    guides: &Guides,
    session: &AdminSession,
    headers: &HeaderMap,
    request: &str,
) -> Result<(Option<Keyed>, Option<Response>), Refusal> {
    super::super::replay::recall(&guides.state, session, headers, request).await
}

/// A keyed write's outcome: `Ok(Some)` is the stored answer to replay.
fn settled<T>(written: KeyedWrite<T>) -> Result<Result<T, Response>, Refusal> {
    match written {
        KeyedWrite::Applied(outcome) => Ok(Ok(outcome)),
        KeyedWrite::Replayed(found) => replayed(&found).map(Err),
        KeyedWrite::KeyTaken => Err(mismatch()),
    }
}

fn encoded(body: &impl Serialize) -> Result<Value, Refusal> {
    serde_json::to_value(body).map_err(Refusal::unavailable)
}

fn discord_offline() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "discord_offline",
        "Discord is not connected; publish once the bot is back online.",
    )
}

fn confirm_required(message: &'static str) -> Refusal {
    Refusal::unprocessable("confirm_required", message)
}

/// 1 to 100 characters, trimmed.
fn title(raw: &str) -> Result<String, Refusal> {
    let title = raw.trim();
    if title.is_empty() || title.chars().count() > 100 {
        return Err(Refusal::invalid("A guide title is 1 to 100 characters."));
    }
    Ok(title.to_owned())
}

/// A channel the bot lists now (a snowflake), unless it is `kept`.
fn channel(
    guides: &Guides,
    wanted: Option<String>,
    kept: Option<&str>,
) -> Result<Option<String>, Refusal> {
    let Some(wanted) = wanted else {
        return Ok(None);
    };
    if kept == Some(wanted.as_str()) {
        return Ok(Some(wanted));
    }
    let snowflake = (17..=20).contains(&wanted.len()) && wanted.bytes().all(|b| b.is_ascii_digit());
    let listed = guides
        .state
        .channels
        .channels()
        .iter()
        .any(|channel| channel.id == wanted);
    if !snowflake || !listed {
        return Err(Refusal::invalid("Choose a current Discord channel."));
    }
    Ok(Some(wanted))
}

fn written(write: GuideWrite) -> Refusal {
    match write {
        GuideWrite::Stale(_) => stale(),
        GuideWrite::ChannelTaken => channel_taken(),
        GuideWrite::Published => Refusal::conflict(
            "unpublish_first",
            "This guide has messages in Discord: unpublish it first.",
        ),
        GuideWrite::Busy => publishing(),
        GuideWrite::Missing
        | GuideWrite::Created(_)
        | GuideWrite::Saved(_)
        | GuideWrite::Deleted(_) => not_found(),
    }
}

/// Write `change`, answering `status` with `body` (stored as the keyed
/// request's replay in the same transaction).
async fn apply(
    guides: &Guides,
    keyed: Option<Keyed>,
    change: GuideChange,
    record: GuideRecord,
    status: StatusCode,
    body: Value,
) -> Reply {
    let replay = keyed.map(|keyed| keyed.answered(status, &body, record.at));
    match settled(
        guides
            .book()
            .write_guide(change, record, replay)
            .await
            .map_err(Refusal::unavailable)?,
    )? {
        Ok(write) if write.applied() => Ok((status, Json(body)).into_response()),
        Ok(write) => Err(written(write)),
        Err(replay) => Ok(replay),
    }
}

/// `GuideCreate`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuideCreate {
    title: String,
    #[serde(default)]
    channel_id: Option<String>,
}

/// `POST /api/admin/guides` → `201 GuideSaved`: a guide at version 1 with
/// no blocks; 409 `channel_taken`.
pub(super) async fn create(
    guides: Guides,
    session: AdminSession,
    headers: HeaderMap,
    Body(body): Body<GuideCreate>,
) -> Reply {
    let title = title(&body.title)?;
    let request = format!(
        "guide.create\u{1f}{}",
        json!({"title": title, "channel_id": body.channel_id})
    );
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let channel_id = channel(&guides, body.channel_id, None)?;
    let now = guides.state.now();
    let id = RandomIds.new_id();
    let body = encoded(&dto::GuideSaved {
        id: id.clone(),
        version: 1,
        updated_at: iso_instant(now),
    })?;
    apply(
        &guides,
        keyed,
        GuideChange::Create {
            id,
            title,
            channel_id,
        },
        record(&session, now),
        StatusCode::CREATED,
        body,
    )
    .await
}

/// `GuideSave`: the full form.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuideSave {
    version: u32,
    title: String,
    channel_id: Option<String>,
    blocks: Vec<GuideBlock>,
}

/// `PATCH /api/admin/guides/{id}` → `GuideSaved`: the whole draft at the
/// version read; 409 `stale`, `channel_taken`; 422 `invalid_blocks` (every
/// problem, block and field named). Saves stay open during a run (the
/// worker stops a stale publish with `plan_changed`); an unchanged form
/// answers the stored version and writes nothing.
pub(super) async fn save(
    guides: Guides,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<GuideSave>,
) -> Reply {
    let title = title(&body.title)?;
    let request = format!(
        "guide.save\u{1f}{id}\u{1f}{}",
        json!({
            "version": body.version,
            "title": title,
            "channel_id": body.channel_id,
            "blocks": body.blocks,
        })
    );
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let current = guides.guide(&id).await?;
    if current.version != body.version {
        return Err(stale());
    }
    let channel_id = channel(&guides, body.channel_id, current.channel_id.as_deref())?;
    let blocks: Vec<Block> = body.blocks.into_iter().map(Block::from).collect();
    if let Err(errors) = Renderer::new(&site, &guides.state)
        .render(blocks.clone())
        .await?
    {
        return Ok(blocks_refused("invalid_blocks", &errors));
    }
    let now = guides.state.now();
    let unchanged =
        current.title == title && current.channel_id == channel_id && current.blocks == blocks;
    let body = encoded(&if unchanged {
        dto::GuideSaved {
            id: id.clone(),
            version: current.version,
            updated_at: iso_instant(current.updated_at),
        }
    } else {
        dto::GuideSaved {
            id: id.clone(),
            version: current.version + 1,
            updated_at: iso_instant(now),
        }
    })?;
    apply(
        &guides,
        keyed,
        GuideChange::Save {
            id,
            version: current.version,
            title,
            channel_id,
            blocks,
        },
        record(&session, now),
        StatusCode::OK,
        body,
    )
    .await
}

/// `GuideDelete`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuideDelete {
    version: u32,
}

/// `DELETE /api/admin/guides/{id}` → `Message`: 409 `stale`,
/// `unpublish_first` while messages are (or may be) in Discord,
/// `publishing` during a run.
pub(super) async fn delete(
    guides: Guides,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<GuideDelete>,
) -> Reply {
    let request = format!("guide.delete\u{1f}{id}\u{1f}{}", body.version);
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let current = guides.guide(&id).await?;
    let answer = json!({"message": format!("{} is deleted.", current.title)});
    apply(
        &guides,
        keyed,
        GuideChange::Delete {
            id,
            version: body.version,
        },
        record(&session, guides.state.now()),
        StatusCode::OK,
        answer,
    )
    .await
}

/// Queue `run`, answering `202 GuidePublishResult` and waking the worker.
async fn queue(
    guides: &Guides,
    session: &AdminSession,
    keyed: Option<Keyed>,
    run: Publish,
    counts: PlanCounts,
) -> Reply {
    let planned = counts.edit + counts.create + counts.delete;
    let body = encoded(&dto::GuidePublishResult {
        no_op: false,
        publish: Some(dto::run(&run, &[], Some(planned))),
    })?;
    let now = run.requested_at;
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::ACCEPTED, &body, now));
    let outcome = guides
        .book()
        .queue_publish(
            PublishRequest {
                publish: run,
                counts,
            },
            record(session, now),
            replay,
        )
        .await
        .map_err(Refusal::unavailable)?;
    match settled(outcome)? {
        Ok(PublishRequested::Queued(_)) => {
            guides.port.wake();
            Ok((StatusCode::ACCEPTED, Json(body)).into_response())
        }
        Ok(PublishRequested::Stale(_)) => Err(stale()),
        Ok(PublishRequested::Busy(_)) => Err(publishing()),
        Ok(PublishRequested::Missing) => Err(not_found()),
        Err(replay) => Ok(replay),
    }
}

/// A queued run of `kind` for `guide`, requested now by the session, with
/// nothing confirmed and the old messages kept (a publish sets both).
fn new_run(
    guides: &Guides,
    session: &AdminSession,
    guide: &Guide,
    kind: PublishKind,
    plan_hash: String,
    channel_id: String,
) -> Publish {
    Publish {
        id: RandomIds.new_id(),
        guide_id: guide.id.clone(),
        version: guide.version,
        plan_hash,
        channel_id,
        kind,
        confirmed: Confirmations::default(),
        delete_old: false,
        state: PublishState::Queued,
        stop_reason: None,
        requested_by: actor_text(session),
        requested_at: guides.state.now(),
        started_at: None,
        finished_at: None,
        cancel_requested_at: None,
    }
}

fn no_op() -> Result<Response, Refusal> {
    Ok(answer(
        StatusCode::OK,
        &dto::GuidePublishResult {
            no_op: true,
            publish: None,
        },
    ))
}

/// Refuse a plan that cannot run, or that changed since the preview.
fn runnable(planned: &Planned, plan_hash: &str) -> Result<(), Refusal> {
    let plan = &planned.plan;
    if plan.hash != plan_hash {
        return Err(Refusal::conflict(
            "plan_changed",
            "The guide or its channel changed since you previewed; check the new plan.",
        ));
    }
    if let Some(blocked) = &plan.blocked {
        let message = match blocked {
            crate::domain::guides::Blocked::UnknownSlot { position, .. } => format!(
                "Message {} may or may not have posted: settle it first.",
                position + 1
            ),
            crate::domain::guides::Blocked::ChannelMissing => {
                "The channel is gone or the bot cannot see it; choose another.".to_owned()
            }
            crate::domain::guides::Blocked::Grants(grants) => format!(
                "The bot is missing permissions in the channel: {}.",
                grants
                    .iter()
                    .map(|grant| grant.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        return Err(Refusal::conflict("blocked", message));
    }
    Ok(())
}

/// `GuidePublish`: what the admin previewed and confirmed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuidePublish {
    version: u32,
    plan_hash: String,
    #[serde(default)]
    confirm: GuideConfirmations,
    #[serde(default = "yes")]
    delete_old: bool,
}

fn yes() -> bool {
    true
}

/// `POST /api/admin/guides/{id}/publish` → `202 GuidePublishResult` (a
/// queued run), `200` with `no_op`; 409 `stale`, `plan_changed` (the plan
/// recomputed now differs from `plan_hash`), `publishing`, `blocked`,
/// `no_channel`; 422 `needs_confirmation` (with `needs`); 503
/// `discord_offline`.
pub(super) async fn publish(
    guides: Guides,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<GuidePublish>,
) -> Reply {
    let request = format!(
        "guide.publish\u{1f}{id}\u{1f}{}",
        json!({
            "version": body.version,
            "plan_hash": body.plan_hash,
            "confirm": body.confirm,
            "delete_old": body.delete_old,
        })
    );
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    if !guides.state.channels.connected() {
        return Err(discord_offline());
    }
    let guide = guides.guide(&id).await?;
    if guide.version != body.version {
        return Err(stale());
    }
    live_run(&guides, &guide).await?;
    let kind = PlanKind::Publish {
        delete_old: body.delete_old,
    };
    let planned = match guides.planned(&site, guide, kind).await? {
        Ok(planned) => planned,
        Err(refused) => return Ok(refused),
    };
    runnable(&planned, &body.plan_hash)?;
    let plan = &planned.plan;
    if plan.no_op() {
        return no_op();
    }
    let confirmed = Confirmations::from(body.confirm);
    if !confirmed.covers(&plan.needs) {
        return Ok(answer(
            StatusCode::UNPROCESSABLE_ENTITY,
            &GuideNeedsConfirmation {
                error: "needs_confirmation",
                message: "Confirm what this publish deletes, moves or posts below other \
                          messages first."
                    .into(),
                needs: (&plan.needs).into(),
            },
        ));
    }
    let run = Publish {
        confirmed,
        delete_old: body.delete_old,
        ..new_run(
            &guides,
            &session,
            &planned.guide,
            PublishKind::Publish,
            plan.hash.clone(),
            plan.channel_id.clone(),
        )
    };
    queue(&guides, &session, keyed, run, plan.counts).await
}

/// 409 `publishing` while the guide has a queued or running run.
async fn live_run(guides: &Guides, guide: &Guide) -> Result<(), Refusal> {
    let latest = guides
        .book()
        .latest_publish(guide.id.clone())
        .await
        .map_err(Refusal::unavailable)?;
    if latest.is_some_and(|run| run.live()) {
        return Err(publishing());
    }
    Ok(())
}

/// `GuideUnpublish`: the unpublish plan's hash, confirmed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuideUnpublish {
    plan_hash: String,
    #[serde(default)]
    confirm: bool,
}

/// `POST /api/admin/guides/{id}/unpublish` → `202 GuidePublishResult`: a
/// run deleting every published message, newest first (`200` with `no_op`
/// when none is left); 422 `confirm_required`; refusals as publish.
pub(super) async fn unpublish(
    guides: Guides,
    State(site): State<Arc<Site>>,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<GuideUnpublish>,
) -> Reply {
    let request = format!(
        "guide.unpublish\u{1f}{id}\u{1f}{}",
        json!({"plan_hash": body.plan_hash, "confirm": body.confirm})
    );
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    if !body.confirm {
        return Err(confirm_required(
            "Confirm deleting this guide's messages from Discord.",
        ));
    }
    if !guides.state.channels.connected() {
        return Err(discord_offline());
    }
    let guide = guides.guide(&id).await?;
    live_run(&guides, &guide).await?;
    let planned = match guides.planned(&site, guide, PlanKind::Unpublish).await? {
        Ok(planned) => planned,
        Err(refused) => return Ok(refused),
    };
    runnable(&planned, &body.plan_hash)?;
    let plan = &planned.plan;
    if plan.no_op() {
        return no_op();
    }
    let run = new_run(
        &guides,
        &session,
        &planned.guide,
        PublishKind::Unpublish,
        plan.hash.clone(),
        plan.channel_id.clone(),
    );
    queue(&guides, &session, keyed, run, plan.counts).await
}

/// `POST /api/admin/guides/{id}/search` → `202 GuidePublishResult`: a
/// "Search again" run for the guide's unconfirmed posts (5.3); 409
/// `nothing_unknown`, `publishing`; 503 `discord_offline`.
pub(super) async fn search(
    guides: Guides,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Reply {
    let request = format!("guide.search\u{1f}{id}");
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    if !guides.state.channels.connected() {
        return Err(discord_offline());
    }
    let guide = guides.guide(&id).await?;
    live_run(&guides, &guide).await?;
    let slots = guides
        .book()
        .slots(id)
        .await
        .map_err(Refusal::unavailable)?;
    let channel = slots
        .iter()
        .find(|slot| slot.state == SlotState::Unknown)
        .map(|slot| slot.channel_id.clone())
        .ok_or_else(|| {
            Refusal::conflict(
                "nothing_unknown",
                "Every message of this guide is confirmed; there is nothing to search for.",
            )
        })?;
    // A search runs no plan: its hash names the empty one in that channel.
    let confirmed = Confirmations::default();
    let hash = plan_hash(&channel, &[], &confirmed);
    let run = new_run(
        &guides,
        &session,
        &guide,
        PublishKind::Search,
        hash,
        channel,
    );
    queue(&guides, &session, keyed, run, PlanCounts::default()).await
}

/// `POST /api/admin/guides/{id}/publishes/{run}/cancel` → `GuideRun` with
/// `cancel_requested_at`: the worker stops between ops; 409 `finished`.
pub(super) async fn cancel(
    guides: Guides,
    session: AdminSession,
    UrlPath((id, run_id)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Reply {
    let request = format!("guide.cancel\u{1f}{id}\u{1f}{run_id}");
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    let gone = || Refusal::not_found("not_found", "That publish run is gone.");
    let finished = || Refusal::conflict("finished", "That publish run has already finished.");
    let (run, ops) = guides
        .book()
        .publish(run_id.clone())
        .await
        .map_err(Refusal::unavailable)?
        .filter(|(run, _)| run.guide_id == id)
        .ok_or_else(gone)?;
    if !run.live() {
        return Err(finished());
    }
    let now = guides.state.now();
    let total = guides.planned_total(&run).await?;
    let asked = Publish {
        cancel_requested_at: run.cancel_requested_at.or(Some(now)),
        ..run
    };
    let body = encoded(&dto::run(&asked, &ops, total))?;
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::OK, &body, now));
    let outcome = guides
        .book()
        .cancel_publish(id, run_id, now, replay)
        .await
        .map_err(Refusal::unavailable)?;
    match settled(outcome)? {
        Ok(crate::domain::guides::CancelRequest::Requested(_)) => Ok(Json(body).into_response()),
        Ok(crate::domain::guides::CancelRequest::Finished(_)) => Err(finished()),
        Ok(crate::domain::guides::CancelRequest::Missing) => Err(gone()),
        Err(replay) => Ok(replay),
    }
}

/// `GuideNotPosted`: the unconfirmed slot, confirmed ("If it did post, a
/// copy may stay in the channel").
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuideNotPosted {
    channel_id: String,
    position: u32,
    #[serde(default)]
    confirm: bool,
}

/// `POST /api/admin/guides/{id}/slots/not-posted` → `Message`: forget an
/// unconfirmed post so the next publish posts it again (never automatic;
/// History row `slot.<guide>.<position>`); 422 `confirm_required`; 409
/// `not_unknown`, `publishing`.
pub(super) async fn not_posted(
    guides: Guides,
    session: AdminSession,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Body(body): Body<GuideNotPosted>,
) -> Reply {
    let request = format!(
        "guide.not_posted\u{1f}{id}\u{1f}{}",
        json!({"channel_id": body.channel_id, "position": body.position, "confirm": body.confirm})
    );
    let (keyed, answered) = recall(&guides, &session, &headers, &request).await?;
    if let Some(answer) = answered {
        return Ok(answer);
    }
    if !body.confirm {
        return Err(confirm_required(
            "Confirm marking it as not posted: if it did post, a copy may stay in the channel.",
        ));
    }
    guides.guide(&id).await?;
    let now = guides.state.now();
    let answer = json!({"message": format!(
        "Message {} is marked as not posted; the next publish posts it again.",
        body.position + 1
    )});
    let replay = keyed.map(|keyed| keyed.answered(StatusCode::OK, &answer, now));
    let outcome = guides
        .book()
        .not_posted(
            id,
            body.channel_id,
            body.position,
            record(&session, now),
            replay,
        )
        .await
        .map_err(Refusal::unavailable)?;
    match settled(outcome)? {
        Ok(NotPosted::Removed(_)) => Ok(Json(answer).into_response()),
        Ok(NotPosted::Missing) => Err(Refusal::not_found(
            "not_found",
            "That message is not part of this guide.",
        )),
        Ok(NotPosted::NotUnknown) => Err(Refusal::conflict(
            "not_unknown",
            "That message is confirmed, not waiting to be settled.",
        )),
        Ok(NotPosted::Busy) => Err(publishing()),
        Err(replay) => Ok(replay),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_trimmed_and_bounded() {
        assert_eq!(title("  Kalos basics ").unwrap(), "Kalos basics");
        assert!(title("   ").is_err());
        assert!(title(&"x".repeat(101)).is_err());
        assert!(title(&"x".repeat(100)).is_ok());
    }
}
