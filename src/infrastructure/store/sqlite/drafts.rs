//! Draft rows: reads, writes and the merge commit (`drafts`, `draft_ops`,
//! `draft_events`, `draft_requests` from migration 0005). Write operations
//! run on the writer lease in one `BEGIN IMMEDIATE` transaction each, as
//! schedule commits do.

use sqlx::sqlite::{Sqlite, SqliteRow};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::history::{self, COLUMNS};
use super::rows;
use super::schedule::{Collapsed, load, read_revision, store_error, write};
use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEvent, DraftEventKind, DraftKind, DraftScope, DraftStale,
    DraftStatus, DraftUpdate, DraftWrite, LoadedDraft, MergeCommit, NewDraft, RequestLimit,
    RequestLimits, StagedOp, StoredDraft, decode, encode,
};
use crate::domain::history::{Actor, ChangeMeta, ChangeRecord, ChangeRef};
use crate::domain::notify::draft_source;
use crate::domain::schedule::Notice;
use crate::domain::scheduler::{Committed, StoreError};
use crate::infrastructure::store::history::touched_keys;

/// Selected `FROM drafts` (unaliased): a `draft_proposals` row (0007) makes
/// a stored `admin` draft a proposal.
pub(super) const DRAFT_COLUMNS: &str = "id, kind, title, author_kind, author_id, base_seq, \
    base_hash, base_revision, version, status, request_type, subject, merged_seq, \
    closed_by_kind, closed_by_id, close_reason, created_at, updated_at, expires_week, \
    EXISTS (SELECT 1 FROM draft_proposals WHERE draft_proposals.draft_id = drafts.id) \
    AS proposal";

/// The `drafts.kind` spelling: proposals are stored as `admin` rows.
pub(super) fn stored_kind(kind: DraftKind) -> &'static str {
    match kind {
        DraftKind::Proposal => DraftKind::Admin.as_str(),
        other => other.as_str(),
    }
}

fn backend(_row: &SqliteRow, column: &str) -> StoreError {
    StoreError::Backend(format!("drafts.{column} missing"))
}

fn unsigned(value: i64, column: &str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Backend(format!("drafts.{column} negative")))
}

pub(super) fn signed(value: u64, column: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Backend(format!("drafts.{column} too large")))
}

fn actor(kind: &str, id: &str) -> Result<Actor, StoreError> {
    Actor::from_parts(kind, id).ok_or_else(|| StoreError::Backend(format!("actor {kind}")))
}

pub(super) fn instant(
    text: &str,
    column: &str,
) -> Result<chrono::DateTime<chrono::Utc>, StoreError> {
    crate::domain::time::from_iso(text)
        .map_err(|error| StoreError::Backend(format!("drafts.{column}: {error}")))
}

pub(super) fn draft_of(row: &SqliteRow) -> Result<StoredDraft, StoreError> {
    let text = |column: &str| {
        row.try_get::<String, _>(column)
            .map_err(|_| backend(row, column))
    };
    let optional = |column: &str| {
        row.try_get::<Option<String>, _>(column)
            .map_err(|_| backend(row, column))
    };
    let kind = text("kind")?;
    let status = text("status")?;
    let author = actor(&text("author_kind")?, &text("author_id")?)?;
    let closed = match (optional("closed_by_kind")?, optional("closed_by_id")?) {
        (Some(kind), Some(id)) => Some(actor(&kind, &id)?),
        (None, None) => None,
        _ => return Err(StoreError::Backend("drafts.closed_by is half set".into())),
    };
    let proposal: bool = row
        .try_get("proposal")
        .map_err(|_| backend(row, "proposal"))?;
    let kind = match (DraftKind::parse(&kind), proposal) {
        (Some(DraftKind::Admin), true) => DraftKind::Proposal,
        (Some(kind @ (DraftKind::Admin | DraftKind::Request)), false) => kind,
        _ => return Err(StoreError::Backend(format!("draft kind {kind}"))),
    };
    Ok(StoredDraft {
        id: text("id")?,
        kind,
        title: text("title")?,
        author,
        base: ChangeRef {
            seq: unsigned(
                row.try_get("base_seq")
                    .map_err(|_| backend(row, "base_seq"))?,
                "base_seq",
            )?,
            hash: text("base_hash")?,
        },
        base_revision: unsigned(
            row.try_get("base_revision")
                .map_err(|_| backend(row, "base_revision"))?,
            "base_revision",
        )?,
        version: unsigned(
            row.try_get("version")
                .map_err(|_| backend(row, "version"))?,
            "version",
        )?,
        status: DraftStatus::parse(&status)
            .ok_or_else(|| StoreError::Backend(format!("draft status {status}")))?,
        request_type: optional("request_type")?,
        subject: optional("subject")?,
        merged_seq: row
            .try_get::<Option<i64>, _>("merged_seq")
            .map_err(|_| backend(row, "merged_seq"))?
            .map(|seq| unsigned(seq, "merged_seq"))
            .transpose()?,
        closed_by: closed,
        close_reason: optional("close_reason")?,
        created_at: instant(&text("created_at")?, "created_at")?,
        updated_at: instant(&text("updated_at")?, "updated_at")?,
        scope: match optional("expires_week")? {
            None => DraftScope::Weekly,
            Some(week) => DraftScope::Week(instant(&week, "expires_week")?),
        },
    })
}

fn event_of(row: &SqliteRow) -> Result<DraftEvent, StoreError> {
    let text = |column: &str| {
        row.try_get::<String, _>(column)
            .map_err(|_| StoreError::Backend(format!("draft_events.{column} missing")))
    };
    let kind = text("kind")?;
    Ok(DraftEvent {
        draft_id: text("draft_id")?,
        version: unsigned(
            row.try_get("version")
                .map_err(|_| StoreError::Backend("draft_events.version missing".into()))?,
            "version",
        )?,
        kind: DraftEventKind::parse(&kind)
            .ok_or_else(|| StoreError::Backend(format!("draft event {kind}")))?,
        actor: actor(&text("actor_kind")?, &text("actor_id")?)?,
        at: instant(&text("at")?, "at")?,
        detail: row
            .try_get::<Option<String>, _>("detail")
            .map_err(|_| StoreError::Backend("draft_events.detail missing".into()))?,
    })
}

async fn staged_ops(
    conn: &mut SqliteConnection,
    draft_id: &str,
) -> Result<Vec<StagedOp>, StoreError> {
    let rows = sqlx::query(
        "SELECT ord, op, author_kind, author_id, added_at FROM draft_ops \
         WHERE draft_id = ?1 ORDER BY ord",
    )
    .bind(draft_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    let mut staged = Vec::with_capacity(rows.len());
    for row in &rows {
        let ord: i64 = row.try_get("ord").map_err(store_error)?;
        let op: String = row.try_get("op").map_err(store_error)?;
        let author_kind: String = row.try_get("author_kind").map_err(store_error)?;
        let author_id: String = row.try_get("author_id").map_err(store_error)?;
        let added_at: String = row.try_get("added_at").map_err(store_error)?;
        staged.push(StagedOp {
            ord: unsigned(ord, "ord")? as usize,
            op: decode(&op).map_err(|error| StoreError::Backend(format!("draft op: {error}")))?,
            author: actor(&author_kind, &author_id)?,
            added_at: instant(&added_at, "added_at")?,
        });
    }
    Ok(staged)
}

/// Inside a transaction: the draft with its operations in position order.
pub(super) async fn load_in(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<LoadedDraft>, StoreError> {
    let row = sqlx::query(&format!("SELECT {DRAFT_COLUMNS} FROM drafts WHERE id = ?1"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(LoadedDraft {
        draft: draft_of(&row)?,
        ops: staged_ops(conn, id).await?,
    }))
}

fn stale_of(draft: &StoredDraft) -> DraftStale {
    DraftStale::Moved {
        status: draft.status,
        version: draft.version,
        merged_seq: draft.merged_seq,
    }
}

pub(super) async fn execute<'q>(
    tx: &mut SqliteConnection,
    query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
) -> Result<(), StoreError> {
    query.execute(tx).await.map(|_| ()).map_err(store_error)
}

pub(super) async fn append_event(
    conn: &mut SqliteConnection,
    draft_id: &str,
    version: u64,
    kind: DraftEventKind,
    actor: &Actor,
    at: &chrono::DateTime<chrono::Utc>,
    detail: Option<&str>,
) -> Result<(), StoreError> {
    execute(
        conn,
        sqlx::query(
            "INSERT INTO draft_events (draft_id, version, kind, actor_kind, actor_id, at, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(draft_id)
        .bind(signed(version, "id")?)
        .bind(kind.as_str())
        .bind(actor.kind())
        .bind(actor.id())
        .bind(rows::instant(at)?)
        .bind(detail),
    )
    .await
}

async fn create_in(
    conn: &mut SqliteConnection,
    new: &NewDraft,
) -> Result<DraftCreated, StoreError> {
    if let Some(request) = &new.request {
        let found = sqlx::query(
            "SELECT digest, draft_id FROM draft_requests \
             WHERE actor_kind = ?1 AND actor_id = ?2 AND request_id = ?3",
        )
        .bind(new.author.kind())
        .bind(new.author.id())
        .bind(&request.request_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?;
        if let Some(found) = found {
            let digest: String = found.try_get("digest").map_err(store_error)?;
            let draft_id: String = found.try_get("draft_id").map_err(store_error)?;
            let Some(loaded) = load_in(conn, &draft_id).await? else {
                return Err(StoreError::Backend(format!(
                    "draft_requests points at missing draft {draft_id}"
                )));
            };
            return if digest == request.digest {
                Ok(DraftCreated::Replayed(loaded.draft))
            } else {
                Ok(DraftCreated::Mismatch { draft_id })
            };
        }
    }
    if let Some(submit) = &new.submit
        && let Some(limit) = over_limit(conn, new, &submit.limits).await?
    {
        return Ok(DraftCreated::Limited(limit));
    }
    let status = if new.submit.is_some() {
        DraftStatus::Submitted
    } else {
        DraftStatus::Open
    };
    let expires_week = new.submit.as_ref().and_then(|submit| submit.expires_week);
    let stored = insert_draft(conn, new, status, expires_week).await?;
    if let Some(submit) = &new.submit {
        insert_ops(conn, &stored.id, &submit.ops).await?;
    }
    if let Some(request) = &new.request {
        execute(
            conn,
            sqlx::query(
                "INSERT INTO draft_requests \
                 (actor_kind, actor_id, request_id, digest, draft_id, recorded_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(new.author.kind())
            .bind(new.author.id())
            .bind(&request.request_id)
            .bind(&request.digest)
            .bind(&new.id)
            .bind(rows::instant(&new.at)?),
        )
        .await?;
    }
    append_event(
        conn,
        &stored.id,
        1,
        DraftEventKind::Created,
        &stored.author,
        &stored.created_at,
        None,
    )
    .await?;
    if new.submit.is_some() {
        append_event(
            conn,
            &stored.id,
            1,
            DraftEventKind::Submitted,
            &stored.author,
            &stored.created_at,
            None,
        )
        .await?;
    }
    Ok(DraftCreated::Created(stored))
}

/// Inside a transaction: the `drafts` row alone, at version 1.
pub(super) async fn insert_draft(
    conn: &mut SqliteConnection,
    new: &NewDraft,
    status: DraftStatus,
    expires_week: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<StoredDraft, StoreError> {
    let stored = StoredDraft {
        id: new.id.clone(),
        kind: new.kind,
        title: new.title.clone(),
        author: new.author.clone(),
        base: new.base.clone(),
        base_revision: new.base_revision,
        version: 1,
        status,
        request_type: new.request_type.clone(),
        subject: new.subject.clone(),
        merged_seq: None,
        closed_by: None,
        close_reason: None,
        created_at: new.at,
        updated_at: new.at,
        scope: match expires_week {
            None => DraftScope::Weekly,
            Some(week) => DraftScope::Week(week),
        },
    };
    execute(
        conn,
        sqlx::query(
            "INSERT INTO drafts (id, kind, title, author_kind, author_id, base_seq, base_hash, \
             base_revision, version, status, request_type, subject, merged_seq, closed_by_kind, \
             closed_by_id, close_reason, created_at, updated_at, expires_week) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?13, ?9, ?10, NULL, NULL, NULL, NULL, \
             ?11, ?11, ?12)",
        )
        .bind(&stored.id)
        .bind(stored_kind(stored.kind))
        .bind(&stored.title)
        .bind(stored.author.kind())
        .bind(stored.author.id())
        .bind(signed(stored.base.seq, "id")?)
        .bind(&stored.base.hash)
        .bind(signed(stored.base_revision, "id")?)
        .bind(&stored.request_type)
        .bind(&stored.subject)
        .bind(rows::instant(&stored.created_at)?)
        .bind(expires_week.as_ref().map(rows::instant).transpose()?)
        .bind(stored.status.as_str()),
    )
    .await?;
    Ok(stored)
}

/// Inside the insert transaction: the author's pending requests and the
/// requests they submitted within the rolling window.
async fn over_limit(
    conn: &mut SqliteConnection,
    new: &NewDraft,
    limits: &RequestLimits,
) -> Result<Option<RequestLimit>, StoreError> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts WHERE kind = 'request' AND author_kind = ?1 \
         AND author_id = ?2 AND status = 'submitted'",
    )
    .bind(new.author.kind())
    .bind(new.author.id())
    .fetch_one(&mut *conn)
    .await
    .map_err(store_error)?;
    let pending = unsigned(pending, "pending")?;
    if pending >= limits.max_pending {
        return Ok(Some(RequestLimit::Pending {
            count: pending,
            max: limits.max_pending,
        }));
    }
    let since = rows::instant(&(new.at - limits.window))?;
    let recent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts WHERE kind = 'request' AND author_kind = ?1 \
         AND author_id = ?2 AND created_at > ?3",
    )
    .bind(new.author.kind())
    .bind(new.author.id())
    .bind(since)
    .fetch_one(&mut *conn)
    .await
    .map_err(store_error)?;
    let recent = unsigned(recent, "recent")?;
    if recent >= limits.max_per_window {
        return Ok(Some(RequestLimit::Rate {
            count: recent,
            max: limits.max_per_window,
        }));
    }
    Ok(None)
}

pub(super) async fn insert_ops(
    conn: &mut SqliteConnection,
    draft_id: &str,
    ops: &[StagedOp],
) -> Result<(), StoreError> {
    for (position, staged) in ops.iter().enumerate() {
        let encoded = encode(&staged.op)
            .map_err(|error| StoreError::Constraint(format!("draft op: {error}")))?;
        execute(
            conn,
            sqlx::query(
                "INSERT INTO draft_ops \
                 (draft_id, ord, op, author_kind, author_id, added_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(draft_id)
            .bind(signed(
                u64::try_from(position)
                    .map_err(|_| StoreError::Backend("drafts.ord too large".into()))?,
                "id",
            )?)
            .bind(encoded)
            .bind(staged.author.kind())
            .bind(staged.author.id())
            .bind(rows::instant(&staged.added_at)?),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn update_in(
    conn: &mut SqliteConnection,
    update: &DraftUpdate,
) -> Result<crate::domain::drafts::DraftWrite, StoreError> {
    let Some(mut loaded) = load_in(conn, &update.draft_id).await? else {
        return Ok(DraftWrite::Stale(DraftStale::Missing));
    };
    if !loaded.draft.status.is_live() || loaded.draft.version != update.expected_version {
        return Ok(DraftWrite::Stale(stale_of(&loaded.draft)));
    }
    match &update.change {
        DraftChange::ReplaceOps {
            ops,
            event,
            ord,
            expires_week,
        } => {
            execute(
                conn,
                sqlx::query("DELETE FROM draft_ops WHERE draft_id = ?1").bind(&loaded.draft.id),
            )
            .await?;
            insert_ops(conn, &loaded.draft.id, ops).await?;
            loaded.draft.version += 1;
            loaded.draft.updated_at = update.at;
            loaded.draft.scope = match expires_week {
                None => DraftScope::Weekly,
                Some(week) => DraftScope::Week(*week),
            };
            execute(
                conn,
                sqlx::query(
                    "UPDATE drafts SET version = ?1, updated_at = ?2, expires_week = ?3 WHERE id = ?4",
                )
                .bind(signed(loaded.draft.version, "id")?)
                .bind(rows::instant(&update.at)?)
                .bind(
                    expires_week
                        .as_ref()
                        .map(rows::instant)
                        .transpose()?,
                )
                .bind(&loaded.draft.id),
            )
            .await?;
            append_event(
                conn,
                &loaded.draft.id,
                loaded.draft.version,
                *event,
                &update.actor,
                &update.at,
                Some(&ord.to_string()),
            )
            .await?;
        }
        DraftChange::Rebase {
            base,
            base_revision,
            expires_week,
        } => {
            loaded.draft.base = base.clone();
            loaded.draft.base_revision = *base_revision;
            loaded.draft.version += 1;
            loaded.draft.updated_at = update.at;
            loaded.draft.scope = match expires_week {
                None => DraftScope::Weekly,
                Some(week) => DraftScope::Week(*week),
            };
            execute(
                conn,
                sqlx::query(
                    "UPDATE drafts SET base_seq = ?1, base_hash = ?2, base_revision = ?3, \
                     version = ?4, updated_at = ?5, expires_week = ?6 WHERE id = ?7",
                )
                .bind(signed(base.seq, "id")?)
                .bind(&base.hash)
                .bind(signed(*base_revision, "id")?)
                .bind(signed(loaded.draft.version, "id")?)
                .bind(rows::instant(&update.at)?)
                .bind(expires_week.as_ref().map(rows::instant).transpose()?)
                .bind(&loaded.draft.id),
            )
            .await?;
            append_event(
                conn,
                &loaded.draft.id,
                loaded.draft.version,
                DraftEventKind::Rebased,
                &update.actor,
                &update.at,
                Some(&format!("{} {}", base.seq, base.hash)),
            )
            .await?;
        }
        DraftChange::Close {
            status,
            reason,
            notices,
        } => {
            let kind = match status {
                DraftStatus::Discarded => DraftEventKind::Discarded,
                DraftStatus::Rejected => DraftEventKind::Rejected,
                DraftStatus::Withdrawn => DraftEventKind::Withdrawn,
                DraftStatus::Expired => DraftEventKind::Expired,
                _ => {
                    return Err(StoreError::Constraint(format!(
                        "cannot close a draft as {status}"
                    )));
                }
            };
            loaded.draft.status = *status;
            loaded.draft.closed_by = Some(update.actor.clone());
            loaded.draft.close_reason = reason.clone();
            loaded.draft.updated_at = update.at;
            execute(
                conn,
                sqlx::query(
                    "UPDATE drafts SET status = ?1, closed_by_kind = ?2, closed_by_id = ?3, \
                     close_reason = ?4, updated_at = ?5 WHERE id = ?6",
                )
                .bind(status.as_str())
                .bind(update.actor.kind())
                .bind(update.actor.id())
                .bind(reason.as_deref())
                .bind(rows::instant(&update.at)?)
                .bind(&loaded.draft.id),
            )
            .await?;
            append_event(
                conn,
                &loaded.draft.id,
                loaded.draft.version,
                kind,
                &update.actor,
                &update.at,
                reason.as_deref(),
            )
            .await?;
            super::journal::enqueue(conn, &draft_source(&loaded.draft.id), notices, &update.at)
                .await?;
        }
    }
    Ok(DraftWrite::Written(loaded.draft))
}

async fn commit_merge_in(
    conn: &mut SqliteConnection,
    expected_revision: u64,
    changes: crate::domain::schedule::ChangeSet,
    meta: ChangeMeta,
    draft_id: &str,
    expected_version: u64,
    note: Option<&str>,
) -> Result<MergeCommit, StoreError> {
    if changes.is_empty() {
        return Err(StoreError::Constraint("a merge must change rows".into()));
    }
    if let Some(earlier) = history::replayed(conn, &meta).await? {
        return Ok(MergeCommit::Committed(earlier));
    }
    let Some(mut loaded) = load_in(conn, draft_id).await? else {
        return Ok(MergeCommit::Stale(DraftStale::Missing));
    };
    if !loaded.draft.status.is_live() || loaded.draft.version != expected_version {
        return Ok(MergeCommit::Stale(stale_of(&loaded.draft)));
    }
    let found = read_revision(conn).await?;
    if found != expected_revision {
        return Err(StoreError::Conflict {
            expected: expected_revision,
            found,
        });
    }
    let keys = touched_keys(&changes);
    let before = history::row_values(conn, &keys).await?;
    write(conn, Collapsed::new(changes)).await?;
    let after = history::row_values(conn, &keys).await?;
    let committed: Committed =
        history::append(conn, found + 1, meta.clone(), &before, &after).await?;
    loaded.draft.status = DraftStatus::Merged;
    loaded.draft.version += 1;
    loaded.draft.merged_seq = Some(committed.seq);
    loaded.draft.closed_by = Some(meta.origin.actor.clone());
    loaded.draft.updated_at = meta.at;
    execute(
        conn,
        sqlx::query(
            "UPDATE drafts SET status = 'merged', version = ?1, merged_seq = ?2, \
             closed_by_kind = ?3, closed_by_id = ?4, updated_at = ?5 WHERE id = ?6",
        )
        .bind(signed(loaded.draft.version, "id")?)
        .bind(signed(committed.seq, "id")?)
        .bind(meta.origin.actor.kind())
        .bind(meta.origin.actor.id())
        .bind(rows::instant(&meta.at)?)
        .bind(&loaded.draft.id),
    )
    .await?;
    append_event(
        conn,
        &loaded.draft.id,
        loaded.draft.version,
        DraftEventKind::Merged,
        &meta.origin.actor,
        &meta.at,
        Some(&match note {
            Some(note) => format!("{} {note}", committed.seq),
            None => committed.seq.to_string(),
        }),
    )
    .await?;
    Ok(MergeCommit::Committed(committed))
}

async fn expire_in(
    conn: &mut SqliteConnection,
    week: &chrono::DateTime<chrono::Utc>,
    at: &chrono::DateTime<chrono::Utc>,
    actor: &Actor,
    notices: &[(String, Notice)],
) -> Result<Vec<String>, StoreError> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM drafts WHERE status IN ('open', 'submitted') \
         AND expires_week IS NOT NULL AND expires_week < ?1 ORDER BY id",
    )
    .bind(rows::instant(week)?)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    for id in &rows {
        execute(
            conn,
            sqlx::query(
                "UPDATE drafts SET status = 'expired', closed_by_kind = ?1, closed_by_id = ?2, \
                 updated_at = ?3 WHERE id = ?4",
            )
            .bind(actor.kind())
            .bind(actor.id())
            .bind(rows::instant(at)?)
            .bind(id),
        )
        .await?;
        let version: i64 = sqlx::query("SELECT version FROM drafts WHERE id = ?1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .map_err(store_error)?
            .try_get("version")
            .map_err(store_error)?;
        append_event(
            conn,
            id,
            unsigned(version, "version")?,
            DraftEventKind::Expired,
            actor,
            at,
            None,
        )
        .await?;
        let planned: Vec<Notice> = notices
            .iter()
            .filter(|(draft, _)| draft == id)
            .map(|(_, notice)| notice.clone())
            .collect();
        super::journal::enqueue(conn, &draft_source(id), &planned, at).await?;
    }
    Ok(rows)
}

impl crate::domain::drafts::DraftStore for SqliteStore {
    async fn snapshot_with_head(
        &self,
    ) -> Result<(crate::domain::schedule::ScheduleSnapshot, ChangeRef), StoreError> {
        read_txn!(self, tx, async {
            let snapshot = load(&mut tx, &crate::domain::scheduler::Scope::All).await?;
            let head = history::head(&mut tx)
                .await?
                .ok_or_else(|| StoreError::Backend("the history has no genesis record".into()))?;
            Ok((
                snapshot,
                ChangeRef {
                    seq: head.0,
                    hash: head.1,
                },
            ))
        })
    }

    async fn records_after(&self, base: &ChangeRef) -> Result<Vec<ChangeRecord>, StoreError> {
        read_txn!(self, tx, async {
            let rows = sqlx::query(&format!(
                "SELECT {COLUMNS} FROM change_log WHERE seq > ?1 ORDER BY seq"
            ))
            .bind(signed(base.seq, "id")?)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            let mut records = Vec::with_capacity(rows.len());
            for row in &rows {
                // An unparseable row is a tampered link: its stored bytes,
                // hash or indexed columns disagree.
                let (record, _) = history::stored(row, None).map_err(|(seq, _)| {
                    StoreError::HistoryGap(crate::domain::history::HistoryGap::Tampered { seq })
                })?;
                records.push(record);
            }
            Ok(records)
        })
    }

    async fn create_draft(&self, new: NewDraft) -> Result<DraftCreated, StoreError> {
        if new.kind == DraftKind::Proposal {
            return Err(StoreError::Constraint(
                "proposals are created with create_proposal".into(),
            ));
        }
        write_txn!(self, tx, create_in(&mut tx, &new))
    }

    async fn load_draft(&self, id: &str) -> Result<Option<LoadedDraft>, StoreError> {
        read_txn!(self, tx, load_in(&mut tx, id))
    }

    async fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> Result<Option<(String, StoredDraft)>, StoreError> {
        read_txn!(self, tx, async {
            let found = sqlx::query(
                "SELECT digest, draft_id FROM draft_requests \
                 WHERE actor_kind = ?1 AND actor_id = ?2 AND request_id = ?3",
            )
            .bind(author.kind())
            .bind(author.id())
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?;
            let Some(found) = found else {
                return Ok(None);
            };
            let digest: String = found.try_get("digest").map_err(store_error)?;
            let draft_id: String = found.try_get("draft_id").map_err(store_error)?;
            Ok(load_in(&mut tx, &draft_id)
                .await?
                .map(|loaded| (digest, loaded.draft)))
        })
    }

    async fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> Result<Vec<StoredDraft>, StoreError> {
        read_txn!(self, tx, async {
            let rows = match status {
                None => {
                    sqlx::query(&format!(
                        "SELECT {DRAFT_COLUMNS} FROM drafts ORDER BY rowid"
                    ))
                    .fetch_all(&mut *tx)
                    .await
                }
                Some(status) => {
                    sqlx::query(&format!(
                        "SELECT {DRAFT_COLUMNS} FROM drafts WHERE status = ?1 ORDER BY rowid"
                    ))
                    .bind(status.as_str())
                    .fetch_all(&mut *tx)
                    .await
                }
            }
            .map_err(store_error)?;
            rows.iter().map(draft_of).collect()
        })
    }

    async fn draft_events(
        &self,
        id: &str,
    ) -> Result<Vec<crate::domain::drafts::DraftEvent>, StoreError> {
        read_txn!(self, tx, async {
            let rows = sqlx::query(
                "SELECT id, draft_id, version, kind, actor_kind, actor_id, at, detail \
                 FROM draft_events WHERE draft_id = ?1 ORDER BY id",
            )
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            rows.iter().map(event_of).collect()
        })
    }

    async fn update_draft(
        &self,
        update: DraftUpdate,
    ) -> Result<crate::domain::drafts::DraftWrite, StoreError> {
        write_txn!(self, tx, update_in(&mut tx, &update))
    }

    async fn commit_merge(
        &self,
        expected_revision: u64,
        changes: crate::domain::schedule::ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> Result<MergeCommit, StoreError> {
        write_txn!(
            self,
            tx,
            commit_merge_in(
                &mut tx,
                expected_revision,
                changes,
                meta,
                draft_id,
                expected_version,
                note.as_deref()
            )
        )
    }

    async fn expire_drafts(
        &self,
        week: chrono::DateTime<chrono::Utc>,
        at: chrono::DateTime<chrono::Utc>,
        actor: &Actor,
        notices: Vec<(String, Notice)>,
    ) -> Result<Vec<String>, StoreError> {
        write_txn!(self, tx, expire_in(&mut tx, &week, &at, actor, &notices))
    }
}
