//! `GuideStore` over `guides`, `guide_slots`, `guide_publishes` and
//! `guide_publish_ops` (0042): constant SQL, reads on a reader connection,
//! one `BEGIN IMMEDIATE` per write. Create, rename, channel change, delete,
//! publish requests, "Mark as not posted" and run outcomes append their
//! History row (`settings_changes`, section `guides`) in the same
//! transaction; draft saves and refused writes record nothing. The keyed
//! forms are in `replays.rs`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteRow};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use super::settings;
use crate::domain::guides::{
    CancelRequest, Confirmations, GUIDES_SECTION, Guide, GuideChange, GuideRecord, GuideStore,
    GuideWrite, NotPosted, OpKind, OpOutcome, PlanOp, Publish, PublishKind, PublishOp,
    PublishRequest, PublishRequested, PublishState, PublishedSlot, SlotEffect, SlotState,
    StopReason, StoreFuture,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{RowDiff, SettingsChange};
use crate::infrastructure::store::Written;

const GUIDE_COLUMNS: &str =
    "id, title, channel_id, blocks, version, created_at, created_by, updated_at, updated_by";
const PUBLISH_COLUMNS: &str = "id, guide_id, version, plan_hash, channel_id, kind, confirmed, \
     state, stop_reason, requested_by, requested_at, started_at, finished_at, cancel_requested_at";
const SLOT_COLUMNS: &str =
    "channel_id, position, message_id, hash, uploads, state, source, publish_id, updated_at";
const OP_COLUMNS: &str =
    "ordinal, kind, position, channel_id, message_id, hash, source, outcome, detail, at";

/// `guide_publish_ops.detail`: what a settled op found; NULL when nothing.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Detail {
    #[serde(default)]
    candidates: Vec<String>,
}

/// The component run outcomes are recorded as (decision 12).
const SYSTEM_COMPONENT: &str = "guides";

/// `guide_publishes.confirmed`: the admin's confirmations plus `delete_old`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmed {
    repost_from: Option<u32>,
    interleaved: bool,
    channel_move: bool,
    delete_old: bool,
}

/// One row's columns, decoded with the table named in any error.
struct Columns<'a> {
    row: &'a SqliteRow,
    table: &'static str,
}

impl Columns<'_> {
    fn corrupt(&self, column: &str, detail: impl std::fmt::Display) -> StoreError {
        StoreError::Backend(format!("{}.{column} is unreadable: {detail}", self.table))
    }

    fn optional(&self, column: &str) -> Result<Option<String>, StoreError> {
        self.row
            .try_get(column)
            .map_err(|error| self.corrupt(column, error))
    }

    fn text(&self, column: &str) -> Result<String, StoreError> {
        self.optional(column)?
            .ok_or_else(|| self.corrupt(column, "missing"))
    }

    fn at(&self, column: &str) -> Result<DateTime<Utc>, StoreError> {
        crate::domain::time::from_iso(&self.text(column)?)
            .map_err(|error| self.corrupt(column, error))
    }

    fn optional_at(&self, column: &str) -> Result<Option<DateTime<Utc>>, StoreError> {
        self.optional(column)?
            .map(|text| {
                crate::domain::time::from_iso(&text).map_err(|error| self.corrupt(column, error))
            })
            .transpose()
    }

    fn number(&self, column: &str) -> Result<u32, StoreError> {
        let value: i64 = self
            .row
            .try_get(column)
            .map_err(|error| self.corrupt(column, error))?;
        u32::try_from(value).map_err(|error| self.corrupt(column, error))
    }

    fn json<T: DeserializeOwned>(&self, column: &str) -> Result<T, StoreError> {
        serde_json::from_str(&self.text(column)?).map_err(|error| self.corrupt(column, error))
    }

    fn spelled<T>(&self, column: &str, parse: fn(&str) -> Option<T>) -> Result<T, StoreError> {
        let text = self.text(column)?;
        parse(&text).ok_or_else(|| self.corrupt(column, text))
    }
}

fn guide(row: &SqliteRow) -> Result<Guide, StoreError> {
    let cols = Columns {
        row,
        table: "guides",
    };
    Ok(Guide {
        id: cols.text("id")?,
        title: cols.text("title")?,
        channel_id: cols.optional("channel_id")?,
        blocks: cols.json("blocks")?,
        version: cols.number("version")?,
        created_at: cols.at("created_at")?,
        created_by: cols.text("created_by")?,
        updated_at: cols.at("updated_at")?,
        updated_by: cols.text("updated_by")?,
    })
}

fn publish(row: &SqliteRow) -> Result<Publish, StoreError> {
    let cols = Columns {
        row,
        table: "guide_publishes",
    };
    let confirmed: Confirmed = cols.json("confirmed")?;
    Ok(Publish {
        id: cols.text("id")?,
        guide_id: cols.text("guide_id")?,
        version: cols.number("version")?,
        plan_hash: cols.text("plan_hash")?,
        channel_id: cols.text("channel_id")?,
        kind: cols.spelled("kind", PublishKind::parse)?,
        confirmed: Confirmations {
            repost_from: confirmed.repost_from,
            interleaved: confirmed.interleaved,
            channel_move: confirmed.channel_move,
        },
        delete_old: confirmed.delete_old,
        state: cols.spelled("state", PublishState::parse)?,
        stop_reason: cols
            .optional("stop_reason")?
            .map(|text| StopReason::parse(&text).ok_or_else(|| cols.corrupt("stop_reason", text)))
            .transpose()?,
        requested_by: cols.text("requested_by")?,
        requested_at: cols.at("requested_at")?,
        started_at: cols.optional_at("started_at")?,
        finished_at: cols.optional_at("finished_at")?,
        cancel_requested_at: cols.optional_at("cancel_requested_at")?,
    })
}

fn slot(row: &SqliteRow) -> Result<PublishedSlot, StoreError> {
    let cols = Columns {
        row,
        table: "guide_slots",
    };
    Ok(PublishedSlot {
        position: cols.number("position")?,
        channel_id: cols.text("channel_id")?,
        message_id: cols.optional("message_id")?,
        hash: cols.text("hash")?,
        uploads: cols.json("uploads")?,
        state: cols.spelled("state", SlotState::parse)?,
        source: cols.optional("source")?,
        publish_id: cols.text("publish_id")?,
        updated_at: cols.at("updated_at")?,
    })
}

fn op(row: &SqliteRow) -> Result<PublishOp, StoreError> {
    let cols = Columns {
        row,
        table: "guide_publish_ops",
    };
    let position = cols.number("position")?;
    let channel_id = cols.text("channel_id")?;
    let op = match cols.spelled("kind", OpKind::parse)? {
        OpKind::Edit => PlanOp::Edit {
            position,
            channel_id,
            message_id: cols.text("message_id")?,
            hash: cols.text("hash")?,
        },
        OpKind::Create => PlanOp::Create {
            position,
            channel_id,
            hash: cols.text("hash")?,
        },
        OpKind::Delete => PlanOp::Delete {
            position,
            channel_id,
            message_id: cols.text("message_id")?,
        },
        OpKind::Forget => PlanOp::Forget {
            position,
            channel_id,
        },
    };
    Ok(PublishOp {
        ordinal: cols.number("ordinal")?,
        op,
        source: cols.optional("source")?,
        outcome: cols.spelled("outcome", OpOutcome::parse)?,
        candidates: cols
            .optional("detail")?
            .map(|text| {
                serde_json::from_str::<Detail>(&text)
                    .map(|detail| detail.candidates)
                    .map_err(|error| cols.corrupt("detail", error))
            })
            .transpose()?
            .unwrap_or_default(),
        at: cols.at("at")?,
    })
}

fn actor_text(actor: &Actor) -> String {
    format!("{}:{}", actor.kind(), actor.id())
}

fn json_text<T: Serialize>(value: &T, what: &str) -> Result<String, StoreError> {
    serde_json::to_string(value)
        .map_err(|error| StoreError::Constraint(format!("guide {what} does not encode: {error}")))
}

/// One History row (section `guides`) naming `key`.
fn history(
    at: DateTime<Utc>,
    actor: Actor,
    surface: Surface,
    key: String,
    from: String,
    to: String,
) -> SettingsChange {
    SettingsChange {
        id: 0,
        at,
        actor,
        surface,
        section: GUIDES_SECTION.into(),
        revision: 0,
        values: BTreeMap::from([(key, RowDiff { from, to })]),
    }
}

/// A guide's History side: the fields History lists, never the draft.
fn guide_side(guide: Option<&Guide>) -> String {
    guide.map_or_else(String::new, |guide| {
        json!({"title": guide.title, "channel": guide.channel_id}).to_string()
    })
}

async fn load(conn: &mut SqliteConnection, id: &str) -> Result<Option<Guide>, StoreError> {
    sqlx::query(&format!("SELECT {GUIDE_COLUMNS} FROM guides WHERE id = ?1"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?
        .as_ref()
        .map(guide)
        .transpose()
}

async fn all(conn: &mut SqliteConnection) -> Result<Vec<Guide>, StoreError> {
    sqlx::query(&format!(
        "SELECT {GUIDE_COLUMNS} FROM guides ORDER BY title, id"
    ))
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?
    .iter()
    .map(guide)
    .collect()
}

/// Another guide than `id` targets `channel`.
async fn channel_taken(
    conn: &mut SqliteConnection,
    channel: Option<&str>,
    id: &str,
) -> Result<bool, StoreError> {
    let Some(channel) = channel else {
        return Ok(false);
    };
    Ok(
        sqlx::query("SELECT 1 FROM guides WHERE channel_id = ?1 AND id <> ?2")
            .bind(channel)
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(store_error)?
            .is_some(),
    )
}

async fn insert_guide(conn: &mut SqliteConnection, row: &Guide) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO guides (id, title, channel_id, blocks, version, created_at, created_by, \
         updated_at, updated_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&row.id)
    .bind(&row.title)
    .bind(&row.channel_id)
    .bind(json_text(&row.blocks, "blocks")?)
    .bind(i64::from(row.version))
    .bind(instant(&row.created_at)?)
    .bind(&row.created_by)
    .bind(instant(&row.updated_at)?)
    .bind(&row.updated_by)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

async fn update_guide(
    conn: &mut SqliteConnection,
    before: &Guide,
    after: &Guide,
) -> Result<(), StoreError> {
    let written = sqlx::query(
        "UPDATE guides SET title = ?2, channel_id = ?3, blocks = ?4, version = ?5, \
         updated_at = ?6, updated_by = ?7 WHERE id = ?1 AND version = ?8",
    )
    .bind(&after.id)
    .bind(&after.title)
    .bind(&after.channel_id)
    .bind(json_text(&after.blocks, "blocks")?)
    .bind(i64::from(after.version))
    .bind(instant(&after.updated_at)?)
    .bind(&after.updated_by)
    .bind(i64::from(before.version))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?
    .rows_affected();
    if written != 1 {
        return Err(StoreError::Backend(
            "guides row moved inside its write".into(),
        ));
    }
    Ok(())
}

async fn slot_rows(
    conn: &mut SqliteConnection,
    guide_id: &str,
) -> Result<Vec<PublishedSlot>, StoreError> {
    sqlx::query(&format!(
        "SELECT {SLOT_COLUMNS} FROM guide_slots WHERE guide_id = ?1 ORDER BY channel_id, position"
    ))
    .bind(guide_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?
    .iter()
    .map(slot)
    .collect()
}

async fn load_slot(
    conn: &mut SqliteConnection,
    guide_id: &str,
    channel_id: &str,
    position: u32,
) -> Result<Option<PublishedSlot>, StoreError> {
    sqlx::query(&format!(
        "SELECT {SLOT_COLUMNS} FROM guide_slots \
         WHERE guide_id = ?1 AND channel_id = ?2 AND position = ?3"
    ))
    .bind(guide_id)
    .bind(channel_id)
    .bind(i64::from(position))
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(slot)
    .transpose()
}

async fn load_publish(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<Publish>, StoreError> {
    sqlx::query(&format!(
        "SELECT {PUBLISH_COLUMNS} FROM guide_publishes WHERE id = ?1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(publish)
    .transpose()
}

/// The guide's queued or running run (at most one, by index).
async fn live_run(
    conn: &mut SqliteConnection,
    guide_id: &str,
) -> Result<Option<Publish>, StoreError> {
    sqlx::query(&format!(
        "SELECT {PUBLISH_COLUMNS} FROM guide_publishes \
         WHERE guide_id = ?1 AND state IN ('queued', 'running')"
    ))
    .bind(guide_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(publish)
    .transpose()
}

async fn latest(
    conn: &mut SqliteConnection,
    guide_id: &str,
) -> Result<Option<Publish>, StoreError> {
    sqlx::query(&format!(
        "SELECT {PUBLISH_COLUMNS} FROM guide_publishes WHERE guide_id = ?1 \
         ORDER BY requested_at DESC, rowid DESC LIMIT 1"
    ))
    .bind(guide_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(publish)
    .transpose()
}

async fn ops(conn: &mut SqliteConnection, publish_id: &str) -> Result<Vec<PublishOp>, StoreError> {
    sqlx::query(&format!(
        "SELECT {OP_COLUMNS} FROM guide_publish_ops WHERE publish_id = ?1 ORDER BY ordinal"
    ))
    .bind(publish_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?
    .iter()
    .map(op)
    .collect()
}

async fn with_ops(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<(Publish, Vec<PublishOp>)>, StoreError> {
    let Some(run) = load_publish(conn, id).await? else {
        return Ok(None);
    };
    let ops = ops(conn, id).await?;
    Ok(Some((run, ops)))
}

/// The run as stored; a constraint error when it is not running.
async fn running(conn: &mut SqliteConnection, publish_id: &str) -> Result<Publish, StoreError> {
    match load_publish(conn, publish_id).await? {
        Some(run) if run.state == PublishState::Running => Ok(run),
        Some(run) => Err(StoreError::Constraint(format!(
            "guide publish {publish_id} is {}, not running",
            run.state
        ))),
        None => Err(StoreError::Constraint(format!(
            "no guide publish {publish_id}"
        ))),
    }
}

/// One guide write and whether it appended a History row, inside the
/// caller's transaction.
pub(super) async fn apply(
    conn: &mut SqliteConnection,
    change: GuideChange,
    record: &GuideRecord,
) -> Result<(GuideWrite, bool), StoreError> {
    let by = actor_text(&record.actor);
    let recorded = |before: Option<&Guide>, after: Option<&Guide>, id: &str| {
        history(
            record.at,
            record.actor.clone(),
            record.surface,
            format!("guide.{id}"),
            guide_side(before),
            guide_side(after),
        )
    };
    match change {
        GuideChange::Create {
            id,
            title,
            channel_id,
        } => {
            if load(conn, &id).await?.is_some() {
                return Err(StoreError::Constraint(format!("guide {id} already exists")));
            }
            if channel_taken(conn, channel_id.as_deref(), &id).await? {
                return Ok((GuideWrite::ChannelTaken, false));
            }
            let row = Guide {
                id,
                title,
                channel_id,
                blocks: Vec::new(),
                version: 1,
                created_at: record.at,
                created_by: by.clone(),
                updated_at: record.at,
                updated_by: by,
            };
            insert_guide(conn, &row).await?;
            settings::append(conn, &recorded(None, Some(&row), &row.id)).await?;
            Ok((GuideWrite::Created(row), true))
        }
        GuideChange::Save {
            id,
            version,
            title,
            channel_id,
            blocks,
        } => {
            let Some(current) = load(conn, &id).await? else {
                return Ok((GuideWrite::Missing, false));
            };
            if current.version != version {
                return Ok((GuideWrite::Stale(current), false));
            }
            if current.title == title
                && current.channel_id == channel_id
                && current.blocks == blocks
            {
                return Ok((GuideWrite::Saved(current), false));
            }
            if channel_taken(conn, channel_id.as_deref(), &id).await? {
                return Ok((GuideWrite::ChannelTaken, false));
            }
            let next = current
                .version
                .checked_add(1)
                .ok_or_else(|| StoreError::Constraint("guide version overflows".into()))?;
            let after = Guide {
                title,
                channel_id,
                blocks,
                version: next,
                updated_at: record.at,
                updated_by: by,
                ..current.clone()
            };
            update_guide(conn, &current, &after).await?;
            // Draft-only saves are not listed in History (decision 12).
            let listed = guide_side(Some(&current)) != guide_side(Some(&after));
            if listed {
                settings::append(conn, &recorded(Some(&current), Some(&after), &id)).await?;
            }
            Ok((GuideWrite::Saved(after), listed))
        }
        GuideChange::Delete { id, version } => {
            let Some(current) = load(conn, &id).await? else {
                return Ok((GuideWrite::Missing, false));
            };
            if current.version != version {
                return Ok((GuideWrite::Stale(current), false));
            }
            // A live run may still post: wait for it or cancel it first.
            if live_run(conn, &id).await?.is_some() {
                return Ok((GuideWrite::Busy, false));
            }
            // Posted or unknown slots are (or may be) in the channel:
            // unpublish first.
            let published = sqlx::query(
                "SELECT 1 FROM guide_slots WHERE guide_id = ?1 \
                 AND state IN ('posted', 'unknown') LIMIT 1",
            )
            .bind(&id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(store_error)?
            .is_some();
            if published {
                return Ok((GuideWrite::Published, false));
            }
            sqlx::query("DELETE FROM guides WHERE id = ?1")
                .bind(&id)
                .execute(&mut *conn)
                .await
                .map_err(store_error)?;
            settings::append(conn, &recorded(Some(&current), None, &id)).await?;
            Ok((GuideWrite::Deleted(current), true))
        }
    }
}

/// Queue a run (one live run per guide) with its History row.
pub(super) async fn request(
    conn: &mut SqliteConnection,
    request: &PublishRequest,
    record: &GuideRecord,
) -> Result<(PublishRequested, bool), StoreError> {
    let run = &request.publish;
    if run.state != PublishState::Queued
        || run.stop_reason.is_some()
        || run.started_at.is_some()
        || run.finished_at.is_some()
        || run.cancel_requested_at.is_some()
    {
        return Err(StoreError::Constraint(
            "a guide publish request starts queued".into(),
        ));
    }
    let Some(guide) = load(conn, &run.guide_id).await? else {
        return Ok((PublishRequested::Missing, false));
    };
    if guide.version != run.version {
        return Ok((PublishRequested::Stale(guide), false));
    }
    if let Some(live) = live_run(conn, &guide.id).await? {
        return Ok((PublishRequested::Busy(live), false));
    }
    let confirmed = Confirmed {
        repost_from: run.confirmed.repost_from,
        interleaved: run.confirmed.interleaved,
        channel_move: run.confirmed.channel_move,
        delete_old: run.delete_old,
    };
    let confirmed = json_text(&confirmed, "confirmations")?;
    sqlx::query(
        "INSERT INTO guide_publishes (id, guide_id, version, plan_hash, channel_id, kind, \
         confirmed, state, requested_by, requested_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9)",
    )
    .bind(&run.id)
    .bind(&run.guide_id)
    .bind(i64::from(run.version))
    .bind(&run.plan_hash)
    .bind(&run.channel_id)
    .bind(run.kind.as_str())
    .bind(&confirmed)
    .bind(&run.requested_by)
    .bind(instant(&run.requested_at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    let slots = slot_rows(conn, &guide.id).await?.len();
    let counts = &request.counts;
    let to = json!({
        "kind": run.kind.as_str(),
        "version": run.version,
        "channel": run.channel_id,
        "plan": run.plan_hash,
        "ops": {
            "keep": counts.keep,
            "edit": counts.edit,
            "create": counts.create,
            "delete": counts.delete,
        },
        "confirmed": serde_json::from_str::<serde_json::Value>(&confirmed)
            .map_err(|error| StoreError::Backend(error.to_string()))?,
    });
    let change = history(
        record.at,
        record.actor.clone(),
        record.surface,
        format!("publish.{}", run.id),
        json!({"slots": slots}).to_string(),
        to.to_string(),
    );
    settings::append(conn, &change).await?;
    Ok((PublishRequested::Queued(run.clone()), true))
}

/// End a live run with its outcome History row (`system:guides`).
async fn end(
    conn: &mut SqliteConnection,
    run: &Publish,
    state: PublishState,
    stop_reason: Option<StopReason>,
    at: DateTime<Utc>,
) -> Result<Publish, StoreError> {
    if !matches!(
        state,
        PublishState::Done | PublishState::Stopped | PublishState::Cancelled
    ) {
        return Err(StoreError::Constraint(format!(
            "a guide publish cannot end {state}"
        )));
    }
    if !run.live() {
        return Err(StoreError::Constraint(format!(
            "guide publish {} already ended {}",
            run.id, run.state
        )));
    }
    sqlx::query(
        "UPDATE guide_publishes SET state = ?2, stop_reason = ?3, finished_at = ?4 WHERE id = ?1",
    )
    .bind(&run.id)
    .bind(state.as_str())
    .bind(stop_reason.map(StopReason::as_str))
    .bind(instant(&at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    let outcomes: Vec<(String, i64)> = sqlx::query_as(
        "SELECT outcome, COUNT(*) FROM guide_publish_ops WHERE publish_id = ?1 \
         GROUP BY outcome ORDER BY outcome",
    )
    .bind(&run.id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    let outcomes: BTreeMap<String, i64> = outcomes.into_iter().collect();
    let change = history(
        at,
        Actor::system(SYSTEM_COMPONENT),
        Surface::DeliveryTick,
        format!("publish.{}", run.id),
        json!({"state": run.state.as_str()}).to_string(),
        json!({
            "state": state.as_str(),
            "stop_reason": stop_reason.map(StopReason::as_str),
            "ops": outcomes,
        })
        .to_string(),
    );
    settings::append(conn, &change).await?;
    load_publish(conn, &run.id)
        .await?
        .ok_or_else(|| StoreError::Backend("guide publish vanished inside its write".into()))
}

/// Ask a live run to stop between ops; an earlier request's instant stays.
pub(super) async fn cancel(
    conn: &mut SqliteConnection,
    guide_id: &str,
    publish_id: &str,
    at: DateTime<Utc>,
) -> Result<CancelRequest, StoreError> {
    let Some(run) = load_publish(conn, publish_id)
        .await?
        .filter(|run| run.guide_id == guide_id)
    else {
        return Ok(CancelRequest::Missing);
    };
    if !run.live() {
        return Ok(CancelRequest::Finished(run));
    }
    sqlx::query(
        "UPDATE guide_publishes SET cancel_requested_at = coalesce(cancel_requested_at, ?2) \
         WHERE id = ?1",
    )
    .bind(publish_id)
    .bind(instant(&at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    let run = load_publish(conn, publish_id)
        .await?
        .ok_or_else(|| StoreError::Backend("guide publish vanished inside its write".into()))?;
    Ok(CancelRequest::Requested(run))
}

/// "Mark as not posted": drop an unknown slot with its History row.
pub(super) async fn not_posted(
    conn: &mut SqliteConnection,
    guide_id: &str,
    channel_id: &str,
    position: u32,
    record: &GuideRecord,
) -> Result<(NotPosted, bool), StoreError> {
    let Some(found) = load_slot(conn, guide_id, channel_id, position).await? else {
        return Ok((NotPosted::Missing, false));
    };
    if live_run(conn, guide_id).await?.is_some() {
        return Ok((NotPosted::Busy, false));
    }
    if found.state != SlotState::Unknown {
        return Ok((NotPosted::NotUnknown, false));
    }
    sqlx::query(
        "DELETE FROM guide_slots WHERE guide_id = ?1 AND channel_id = ?2 AND position = ?3",
    )
    .bind(guide_id)
    .bind(channel_id)
    .bind(i64::from(position))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    let change = history(
        record.at,
        record.actor.clone(),
        record.surface,
        format!("slot.{guide_id}.{position}"),
        json!({"channel": channel_id, "state": found.state.as_str(), "source": found.source})
            .to_string(),
        String::new(),
    );
    settings::append(conn, &change).await?;
    Ok((NotPosted::Removed(found), true))
}

async fn put_slot(
    conn: &mut SqliteConnection,
    guide_id: &str,
    row: &PublishedSlot,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO guide_slots (guide_id, channel_id, position, message_id, hash, uploads, \
         state, source, publish_id, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT (guide_id, channel_id, position) DO UPDATE SET \
         message_id = excluded.message_id, hash = excluded.hash, uploads = excluded.uploads, \
         state = excluded.state, source = excluded.source, publish_id = excluded.publish_id, \
         updated_at = excluded.updated_at",
    )
    .bind(guide_id)
    .bind(&row.channel_id)
    .bind(i64::from(row.position))
    .bind(&row.message_id)
    .bind(&row.hash)
    .bind(json_text(&row.uploads, "uploads")?)
    .bind(row.state.as_str())
    .bind(&row.source)
    .bind(&row.publish_id)
    .bind(instant(&row.updated_at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

impl SqliteStore {
    /// Notify History readers after a write that appended a row.
    fn guide_written<T>(&self, (value, recorded): (T, bool)) -> T {
        if recorded {
            self.written().notify(Written::Settings);
        }
        value
    }
}

impl GuideStore for SqliteStore {
    fn guides(&self) -> StoreFuture<'_, Vec<Guide>> {
        Box::pin(async move { read_txn!(self, tx, all(&mut tx)) })
    }

    fn guide(&self, id: String) -> StoreFuture<'_, Option<Guide>> {
        Box::pin(async move { read_txn!(self, tx, load(&mut tx, &id)) })
    }

    fn apply_guide(&self, change: GuideChange, record: GuideRecord) -> StoreFuture<'_, GuideWrite> {
        Box::pin(async move {
            let written = write_txn!(self, tx, apply(&mut tx, change, &record))?;
            Ok(self.guide_written(written))
        })
    }

    fn slots(&self, guide_id: String) -> StoreFuture<'_, Vec<PublishedSlot>> {
        Box::pin(async move { read_txn!(self, tx, slot_rows(&mut tx, &guide_id)) })
    }

    fn request_publish(
        &self,
        request: PublishRequest,
        record: GuideRecord,
    ) -> StoreFuture<'_, PublishRequested> {
        Box::pin(async move {
            let written = write_txn!(self, tx, self::request(&mut tx, &request, &record))?;
            Ok(self.guide_written(written))
        })
    }

    fn publish(&self, id: String) -> StoreFuture<'_, Option<(Publish, Vec<PublishOp>)>> {
        Box::pin(async move { read_txn!(self, tx, with_ops(&mut tx, &id)) })
    }

    fn latest_publish(&self, guide_id: String) -> StoreFuture<'_, Option<Publish>> {
        Box::pin(async move { read_txn!(self, tx, latest(&mut tx, &guide_id)) })
    }

    fn claim_next(&self, at: DateTime<Utc>) -> StoreFuture<'_, Option<Publish>> {
        Box::pin(async move {
            write_txn!(self, tx, async {
                let next: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM guide_publishes WHERE state = 'queued' \
                     ORDER BY requested_at, rowid LIMIT 1",
                )
                .fetch_optional(&mut *tx)
                .await
                .map_err(store_error)?;
                let Some(id) = next else {
                    return Ok(None);
                };
                sqlx::query(
                    "UPDATE guide_publishes SET state = 'running', started_at = ?2 WHERE id = ?1",
                )
                .bind(&id)
                .bind(instant(&at)?)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
                load_publish(&mut tx, &id).await
            })
        })
    }

    fn record_op(&self, publish_id: String, op: PublishOp) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            if op.outcome != OpOutcome::Pending || !op.candidates.is_empty() {
                return Err(StoreError::Constraint(
                    "a guide publish op is recorded pending, with no candidates".into(),
                ));
            }
            write_txn!(self, tx, async {
                running(&mut tx, &publish_id).await?;
                sqlx::query(
                    "INSERT INTO guide_publish_ops (publish_id, ordinal, kind, position, \
                     channel_id, message_id, hash, source, outcome, at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9)",
                )
                .bind(&publish_id)
                .bind(i64::from(op.ordinal))
                .bind(op.op.kind().as_str())
                .bind(i64::from(op.op.position()))
                .bind(op.op.channel_id())
                .bind(op.op.message_id())
                .bind(op.op.hash())
                .bind(&op.source)
                .bind(instant(&op.at)?)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
                Ok(())
            })
        })
    }

    fn apply_op_result(
        &self,
        publish_id: String,
        ordinal: u32,
        outcome: OpOutcome,
        effect: SlotEffect,
        candidates: Vec<String>,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            if outcome == OpOutcome::Pending {
                return Err(StoreError::Constraint(
                    "a guide publish op settles to a final outcome".into(),
                ));
            }
            if let SlotEffect::Put(row) = &effect
                && row.publish_id != publish_id
            {
                return Err(StoreError::Constraint(
                    "a slot is written by the run settling it".into(),
                ));
            }
            let detail = if candidates.is_empty() {
                None
            } else {
                Some(json_text(&Detail { candidates }, "detail")?)
            };
            write_txn!(self, tx, async {
                let run = running(&mut tx, &publish_id).await?;
                let settled = sqlx::query(
                    "UPDATE guide_publish_ops SET outcome = ?3, detail = ?4, at = ?5 \
                     WHERE publish_id = ?1 AND ordinal = ?2 AND outcome = 'pending'",
                )
                .bind(&publish_id)
                .bind(i64::from(ordinal))
                .bind(outcome.as_str())
                .bind(&detail)
                .bind(instant(&at)?)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?
                .rows_affected();
                if settled != 1 {
                    return Err(StoreError::Constraint(format!(
                        "guide publish {publish_id} has no pending op {ordinal}"
                    )));
                }
                match &effect {
                    SlotEffect::Unchanged => {}
                    SlotEffect::Put(row) => put_slot(&mut tx, &run.guide_id, row).await?,
                    SlotEffect::Remove {
                        channel_id,
                        position,
                    } => {
                        sqlx::query(
                            "DELETE FROM guide_slots \
                             WHERE guide_id = ?1 AND channel_id = ?2 AND position = ?3",
                        )
                        .bind(&run.guide_id)
                        .bind(channel_id)
                        .bind(i64::from(*position))
                        .execute(&mut *tx)
                        .await
                        .map_err(store_error)?;
                    }
                }
                Ok(())
            })
        })
    }

    fn finish(
        &self,
        publish_id: String,
        state: PublishState,
        stop_reason: Option<StopReason>,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, Publish> {
        Box::pin(async move {
            let ended = write_txn!(self, tx, async {
                let run = load_publish(&mut tx, &publish_id).await?.ok_or_else(|| {
                    StoreError::Constraint(format!("no guide publish {publish_id}"))
                })?;
                end(&mut tx, &run, state, stop_reason, at).await
            })?;
            self.written().notify(Written::Settings);
            Ok(ended)
        })
    }

    fn request_cancel(
        &self,
        guide_id: String,
        publish_id: String,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, CancelRequest> {
        Box::pin(async move { write_txn!(self, tx, cancel(&mut tx, &guide_id, &publish_id, at)) })
    }

    fn mark_not_posted(
        &self,
        guide_id: String,
        channel_id: String,
        position: u32,
        record: GuideRecord,
    ) -> StoreFuture<'_, NotPosted> {
        Box::pin(async move {
            let written = write_txn!(
                self,
                tx,
                not_posted(&mut tx, &guide_id, &channel_id, position, &record)
            )?;
            Ok(self.guide_written(written))
        })
    }

    fn interrupted_on_start(
        &self,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, Vec<(Publish, Vec<PublishOp>)>> {
        Box::pin(async move {
            let (running, ended) = write_txn!(self, tx, async {
                let live = sqlx::query(&format!(
                    "SELECT {PUBLISH_COLUMNS} FROM guide_publishes \
                     WHERE state IN ('queued', 'running') ORDER BY requested_at, rowid"
                ))
                .fetch_all(&mut *tx)
                .await
                .map_err(store_error)?
                .iter()
                .map(publish)
                .collect::<Result<Vec<_>, _>>()?;
                let mut running = Vec::new();
                let mut ended = false;
                for run in live {
                    if run.state == PublishState::Queued {
                        end(
                            &mut tx,
                            &run,
                            PublishState::Stopped,
                            Some(StopReason::Interrupted),
                            at,
                        )
                        .await?;
                        ended = true;
                    } else {
                        let ops = ops(&mut tx, &run.id).await?;
                        running.push((run, ops));
                    }
                }
                Ok((running, ended))
            })?;
            if ended {
                self.written().notify(Written::Settings);
            }
            Ok(running)
        })
    }
}
