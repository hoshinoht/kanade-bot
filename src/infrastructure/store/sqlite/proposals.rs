//! Proposal drafts: a `drafts` row (stored kind `admin`) plus its
//! `draft_proposals` facts (migration 0007), written in one transaction with
//! the supersede closes. Merges go through the draft `commit_merge`.

use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::drafts::{
    DRAFT_COLUMNS, append_event, draft_of, execute, insert_draft, insert_ops, instant, load_in,
    update_in,
};
use super::rows;
use super::schedule::store_error;
use crate::domain::drafts::{
    DraftChange, DraftEventKind, DraftKind, DraftStatus, DraftUpdate, DraftWrite, LoadedDraft,
    NewDraft, NewProposal, ProposalCreated, ProposalInfo, ProposalSource, SUPERSEDED,
    StoredProposal, check_new_proposal,
};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

const INFO_COLUMNS: &str = "source, source_id, supersede_key, expires_at";

fn info_of(row: &SqliteRow) -> Result<ProposalInfo, StoreError> {
    let source: String = row.try_get("source").map_err(store_error)?;
    let expires_at: String = row.try_get("expires_at").map_err(store_error)?;
    Ok(ProposalInfo {
        source: ProposalSource::parse(&source)
            .ok_or_else(|| StoreError::Backend(format!("proposal source {source}")))?,
        source_id: row.try_get("source_id").map_err(store_error)?,
        supersede_key: row.try_get("supersede_key").map_err(store_error)?,
        expires_at: instant(&expires_at, "expires_at")?,
    })
}

async fn info_in(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ProposalInfo>, StoreError> {
    sqlx::query(&format!(
        "SELECT {INFO_COLUMNS} FROM draft_proposals WHERE draft_id = ?1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(info_of)
    .transpose()
}

/// Close one live draft as `actor` (no version bump, as expiry).
async fn close_in(
    conn: &mut SqliteConnection,
    id: &str,
    status: DraftStatus,
    reason: Option<&str>,
    actor: &Actor,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), StoreError> {
    let Some(loaded) = load_in(conn, id).await? else {
        return Err(StoreError::Backend(format!("proposal {id} vanished")));
    };
    match update_in(
        conn,
        &DraftUpdate {
            draft_id: id.to_owned(),
            expected_version: loaded.draft.version,
            actor: actor.clone(),
            at,
            change: DraftChange::Close {
                status,
                reason: reason.map(str::to_owned),
                notices: Vec::new(),
            },
        },
    )
    .await?
    {
        DraftWrite::Written(_) => Ok(()),
        DraftWrite::Stale(stale) => Err(StoreError::Backend(format!(
            "proposal {id} moved inside its transaction: {stale:?}"
        ))),
    }
}

async fn create_in(
    conn: &mut SqliteConnection,
    new: &NewProposal,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<ProposalCreated, StoreError> {
    if let Some(existing) = load_in(conn, &new.id).await? {
        let info = info_in(conn, &new.id).await?;
        return match info {
            Some(info) if info.source == new.source && info.source_id == new.source_id => {
                Ok(ProposalCreated::Replayed(existing.draft))
            }
            _ => Err(StoreError::Constraint(format!("draft {} exists", new.id))),
        };
    }
    let superseded: Vec<String> = match &new.supersede_key {
        None => Vec::new(),
        Some(key) => sqlx::query_scalar(
            "SELECT d.id FROM draft_proposals p JOIN drafts d ON d.id = p.draft_id \
             WHERE p.supersede_key = ?1 AND d.status IN ('open', 'submitted') ORDER BY d.rowid",
        )
        .bind(key)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?,
    };
    let draft = NewDraft {
        id: new.id.clone(),
        kind: DraftKind::Proposal,
        title: new.title.clone(),
        author: new.author.clone(),
        base: new.base.clone(),
        base_revision: new.base_revision,
        request_type: None,
        subject: new.subject.clone(),
        at: new.at,
        request: None,
        submit: None,
    };
    let stored = insert_draft(conn, &draft, DraftStatus::Submitted, new.expires_week).await?;
    execute(
        conn,
        sqlx::query(
            "INSERT INTO draft_proposals (draft_id, source, source_id, supersede_key, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&new.id)
        .bind(new.source.as_str())
        .bind(&new.source_id)
        .bind(&new.supersede_key)
        .bind(rows::instant(&expires_at)?),
    )
    .await?;
    insert_ops(conn, &new.id, &new.ops).await?;
    for kind in [DraftEventKind::Created, DraftEventKind::Submitted] {
        append_event(conn, &new.id, 1, kind, &new.author, &new.at, None).await?;
    }
    for id in &superseded {
        close_in(
            conn,
            id,
            DraftStatus::Discarded,
            Some(SUPERSEDED),
            &new.author,
            new.at,
        )
        .await?;
    }
    Ok(ProposalCreated::Created {
        draft: stored,
        superseded,
    })
}

async fn expire_in(
    conn: &mut SqliteConnection,
    now: chrono::DateTime<chrono::Utc>,
    actor: &Actor,
) -> Result<Vec<String>, StoreError> {
    let due: Vec<String> = sqlx::query_scalar(
        "SELECT d.id FROM draft_proposals p JOIN drafts d ON d.id = p.draft_id \
         WHERE d.status IN ('open', 'submitted') AND p.expires_at <= ?1 ORDER BY d.id",
    )
    .bind(rows::instant(&now)?)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    for id in &due {
        close_in(conn, id, DraftStatus::Expired, None, actor, now).await?;
    }
    Ok(due)
}

impl crate::domain::drafts::ProposalStore for SqliteStore {
    async fn create_proposal(&self, new: NewProposal) -> Result<ProposalCreated, StoreError> {
        let expires_at = check_new_proposal(&new)?;
        write_txn!(self, tx, create_in(&mut tx, &new, expires_at))
    }

    async fn load_proposal(
        &self,
        id: &str,
    ) -> Result<Option<(LoadedDraft, ProposalInfo)>, StoreError> {
        read_txn!(self, tx, async {
            let Some(info) = info_in(&mut tx, id).await? else {
                return Ok(None);
            };
            Ok(load_in(&mut tx, id).await?.map(|loaded| (loaded, info)))
        })
    }

    async fn list_proposals(&self, live_only: bool) -> Result<Vec<StoredProposal>, StoreError> {
        read_txn!(self, tx, async {
            let rows = sqlx::query(&format!(
                "SELECT {DRAFT_COLUMNS}, p.source, p.source_id, p.supersede_key, p.expires_at \
                 FROM drafts JOIN draft_proposals p ON p.draft_id = drafts.id \
                 WHERE ?1 = 0 OR drafts.status IN ('open', 'submitted') ORDER BY drafts.rowid"
            ))
            .bind(live_only)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            rows.iter()
                .map(|row| {
                    Ok(StoredProposal {
                        draft: draft_of(row)?,
                        info: info_of(row)?,
                    })
                })
                .collect()
        })
    }

    async fn expire_proposals(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        actor: &Actor,
    ) -> Result<Vec<String>, StoreError> {
        write_txn!(self, tx, expire_in(&mut tx, now, actor))
    }
}
