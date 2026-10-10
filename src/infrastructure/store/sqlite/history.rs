//! The change history in `change_log` / `change_log_weeks` (migration 0003).
//! Records are appended inside the commit's transaction; triggers refuse any
//! UPDATE or DELETE.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::{self, FIXED_COLUMNS, REMINDER_COLUMNS, RSVP_COLUMNS, RUN_COLUMNS};
use crate::domain::history::{
    Actor, ChangeFilter, ChangeHistory, ChangeMeta, ChangePage, ChangeQuery, ChangeRecord,
    ChangeRef, CheckedChange, HistoryVerification, RowKey, RowValue, verify_chain,
};
use crate::domain::history::{
    BlameIndex, BlameTarget, BrokenLink, Checkpoint, CheckpointCreated, CheckpointKind,
    Checkpoints, FieldNow, LastChange, NewCheckpoint, PreconditionError, Versioned, changed_fields,
    check_checkpoint, check_field, target_key, validate_fields,
};
use crate::domain::notify::change_source;
use crate::domain::scheduler::{Committed, RecordedRequest, StoreError};
use crate::domain::time::from_iso;
use crate::infrastructure::store::history::{changed_rows, touched_weeks};

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

async fn one_row(
    conn: &mut SqliteConnection,
    sql: &str,
    keys: &[&str],
) -> Result<Option<SqliteRow>, StoreError> {
    let mut query = sqlx::query(sql);
    for key in keys {
        query = query.bind(*key);
    }
    query.fetch_optional(conn).await.map_err(backend)
}

/// One row's current value.
async fn row_value(
    conn: &mut SqliteConnection,
    key: &RowKey,
) -> Result<Option<RowValue>, StoreError> {
    Ok(match key {
        RowKey::FixedRun(id) => {
            let row = one_row(
                conn,
                &format!("SELECT {FIXED_COLUMNS} FROM fixed_runs WHERE id = ?1"),
                &[id],
            )
            .await?
            .map(|row| rows::fixed_run(&row))
            .transpose()?;
            match row {
                Some(row) => {
                    let mut rows_found = [row];
                    rows::attach_standing(conn, &mut rows_found, Some(id)).await?;
                    let [row] = rows_found;
                    Some(RowValue::FixedRun(row))
                }
                None => None,
            }
        }
        RowKey::Run(id) => {
            let row = one_row(
                conn,
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"),
                &[id],
            )
            .await?
            .map(|row| rows::run(&row))
            .transpose()?;
            match row {
                Some(row) => {
                    let mut found = [row];
                    rows::attach_status_pins(
                        conn,
                        &mut found,
                        &rows::list(std::slice::from_ref(id)),
                    )
                    .await?;
                    rows::attach_attendance(
                        conn,
                        &mut found,
                        &rows::list(std::slice::from_ref(id)),
                    )
                    .await?;
                    let [row] = found;
                    Some(RowValue::Run(row))
                }
                None => None,
            }
        }
        RowKey::Reminder(id) => one_row(
            conn,
            &format!("SELECT {REMINDER_COLUMNS} FROM reminders WHERE id = ?1"),
            &[id],
        )
        .await?
        .map(|row| rows::reminder(&row).map(RowValue::Reminder))
        .transpose()?,
        RowKey::Rsvp { run_id, user_id } => one_row(
            conn,
            &format!("SELECT {RSVP_COLUMNS} FROM rsvps WHERE run_id = ?1 AND user_id = ?2"),
            &[run_id, user_id],
        )
        .await?
        .map(|row| rows::rsvp(&row).map(RowValue::Rsvp))
        .transpose()?,
    })
}

/// The current values of `keys`.
pub(super) async fn row_values(
    conn: &mut SqliteConnection,
    keys: &BTreeSet<RowKey>,
) -> Result<BTreeMap<RowKey, Option<RowValue>>, StoreError> {
    let mut values = BTreeMap::new();
    for key in keys {
        values.insert(key.clone(), row_value(conn, key).await?);
    }
    Ok(values)
}

pub(super) async fn head(conn: &mut SqliteConnection) -> Result<Option<(u64, String)>, StoreError> {
    let row = sqlx::query("SELECT seq, hash FROM change_log ORDER BY seq DESC LIMIT 1")
        .fetch_optional(conn)
        .await
        .map_err(backend)?;
    row.map(|row| {
        let seq: i64 = row.try_get("seq").map_err(backend)?;
        let hash: String = row.try_get("hash").map_err(backend)?;
        Ok((u64::try_from(seq).map_err(backend)?, hash))
    })
    .transpose()
}

async fn insert(
    conn: &mut SqliteConnection,
    record: &ChangeRecord,
    request_digest: Option<&str>,
) -> Result<(), StoreError> {
    let at = rows::instant(&record.at)?;
    sqlx::query(
        "INSERT INTO change_log
         (seq, id, revision, at, actor_kind, actor_id, surface, request_id, request_digest,
          body, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(i64::try_from(record.seq).map_err(backend)?)
    .bind(&record.id)
    .bind(i64::try_from(record.revision).map_err(backend)?)
    .bind(at)
    .bind(record.origin.actor.kind())
    .bind(record.origin.actor.id())
    .bind(record.origin.surface.as_str())
    .bind(&record.origin.request_id)
    .bind(request_digest)
    .bind(record.canonical().map_err(backend)?)
    .bind(&record.prev_hash)
    .bind(&record.hash)
    .execute(&mut *conn)
    .await
    .map_err(backend)?;
    for week in &record.weeks {
        sqlx::query("INSERT INTO change_log_weeks (seq, week_start) VALUES (?1, ?2)")
            .bind(i64::try_from(record.seq).map_err(backend)?)
            .bind(rows::instant(week)?)
            .execute(&mut *conn)
            .await
            .map_err(backend)?;
    }
    Ok(())
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(backend)
}

/// The record of `actor`'s `request_id`, if any, with its request digest.
pub(super) async fn recorded_request(
    conn: &mut SqliteConnection,
    actor: &Actor,
    request_id: &str,
) -> Result<Option<RecordedRequest>, StoreError> {
    let row = sqlx::query(
        "SELECT seq, revision, request_digest FROM change_log
         WHERE actor_kind = ?1 AND actor_id = ?2 AND request_id = ?3",
    )
    .bind(actor.kind())
    .bind(actor.id())
    .bind(request_id)
    .fetch_optional(conn)
    .await
    .map_err(backend)?;
    row.map(|row| {
        Ok(RecordedRequest {
            committed: Committed {
                seq: unsigned(row.try_get("seq").map_err(backend)?)?,
                revision: unsigned(row.try_get("revision").map_err(backend)?)?,
                replayed: true,
            },
            digest: row.try_get("request_digest").map_err(backend)?,
        })
    })
    .transpose()
}

/// Inside the commit: the earlier record of this request, or a mismatch.
pub(super) async fn replayed(
    conn: &mut SqliteConnection,
    meta: &ChangeMeta,
) -> Result<Option<Committed>, StoreError> {
    let Some(request_id) = &meta.origin.request_id else {
        return Ok(None);
    };
    match recorded_request(conn, &meta.origin.actor, request_id).await? {
        None => Ok(None),
        Some(recorded) if recorded.digest == meta.request_digest => Ok(Some(recorded.committed)),
        Some(recorded) => Err(StoreError::IdempotencyMismatch {
            seq: recorded.committed.seq,
        }),
    }
}

/// Inside the commit: the preconditions against `change_fields`, in the
/// same `BEGIN IMMEDIATE` transaction as the write.
pub(super) async fn check_expect(
    conn: &mut SqliteConnection,
    meta: &ChangeMeta,
) -> Result<(), StoreError> {
    let expect = &meta.expect;
    validate_fields(&expect.fields).map_err(StoreError::Precondition)?;
    expect
        .check_override_actor(&meta.origin.actor)
        .map_err(StoreError::Precondition)?;
    let mut stale = Vec::new();
    for precondition in &expect.fields {
        let seq: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(seq) FROM change_fields
             WHERE target_kind = ?1 AND target_id = ?2 AND field = ?3",
        )
        .bind(precondition.target.kind())
        .bind(precondition.target.id())
        .bind(&precondition.field)
        .fetch_one(&mut *conn)
        .await
        .map_err(backend)?;
        let last = match seq {
            None => None,
            Some(seq) => Some(
                last_change(conn, unsigned(seq)?)
                    .await?
                    .ok_or_else(|| backend(format!("change_fields names missing change {seq}")))?,
            ),
        };
        let ever: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM change_fields WHERE target_kind = ?1 AND target_id = ?2 LIMIT 1",
        )
        .bind(precondition.target.kind())
        .bind(precondition.target.id())
        .fetch_optional(&mut *conn)
        .await
        .map_err(backend)?;
        let now = FieldNow {
            last,
            target_exists: row_value(conn, &precondition.target_key()).await?.is_some(),
            ever_recorded: ever.is_some(),
            value: row_value(conn, &precondition.value_key()).await?,
        };
        stale.extend(check_field(precondition, now).map_err(StoreError::Precondition)?);
    }
    if !stale.is_empty() {
        return Err(StoreError::StaleEdit(stale));
    }
    for reference in &expect.overrides {
        let known = last_change(conn, reference.seq)
            .await?
            .is_some_and(|last| last.change.hash == reference.hash);
        if !known {
            return Err(StoreError::Precondition(
                PreconditionError::UnknownOverride { seq: reference.seq },
            ));
        }
    }
    Ok(())
}

/// The reference, actor and instant of change `seq`.
async fn last_change(
    conn: &mut SqliteConnection,
    seq: u64,
) -> Result<Option<LastChange>, StoreError> {
    let row = sqlx::query("SELECT hash, actor_kind, actor_id, at FROM change_log WHERE seq = ?1")
        .bind(i64::try_from(seq).map_err(backend)?)
        .fetch_optional(&mut *conn)
        .await
        .map_err(backend)?;
    row.map(|row| {
        let kind: String = row.try_get("actor_kind").map_err(backend)?;
        let id: String = row.try_get("actor_id").map_err(backend)?;
        let at: String = row.try_get("at").map_err(backend)?;
        Ok(LastChange {
            change: ChangeRef {
                seq,
                hash: row.try_get("hash").map_err(backend)?,
            },
            actor: Actor::from_parts(&kind, &id)
                .ok_or_else(|| backend(format!("unknown actor kind {kind}")))?,
            at: from_iso(&at).map_err(backend)?,
        })
    })
    .transpose()
}

async fn insert_fields(
    conn: &mut SqliteConnection,
    record: &ChangeRecord,
) -> Result<(), StoreError> {
    for (target, field) in changed_fields(record) {
        sqlx::query(
            "INSERT OR IGNORE INTO change_fields (target_kind, target_id, field, seq)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(target.kind())
        .bind(target.id())
        .bind(field)
        .bind(i64::try_from(record.seq).map_err(backend)?)
        .execute(&mut *conn)
        .await
        .map_err(backend)?;
    }
    Ok(())
}

/// Migration step: index every stored record's fields. A record that no
/// longer parses is skipped; history verification reports it.
pub(super) async fn backfill_fields(conn: &mut SqliteConnection) -> Result<(), StoreError> {
    let rows = sqlx::query("SELECT body, hash FROM change_log ORDER BY seq")
        .fetch_all(&mut *conn)
        .await
        .map_err(backend)?;
    for row in rows {
        let body: String = row.try_get("body").map_err(backend)?;
        let hash: String = row.try_get("hash").map_err(backend)?;
        if let Ok(record) = ChangeRecord::parse(&body, &hash) {
            insert_fields(conn, &record).await?;
        }
    }
    Ok(())
}

/// Append the record of one commit, inside its transaction.
pub(super) async fn append(
    conn: &mut SqliteConnection,
    revision: u64,
    meta: ChangeMeta,
    before: &BTreeMap<RowKey, Option<RowValue>>,
    after: &BTreeMap<RowKey, Option<RowValue>>,
) -> Result<Committed, StoreError> {
    let keys: BTreeSet<RowKey> = before.keys().cloned().collect();
    let rows = changed_rows(
        &keys,
        |key| before.get(key).cloned().flatten(),
        |key| after.get(key).cloned().flatten(),
    );
    let owning: BTreeSet<String> = rows
        .iter()
        .flat_map(|row| [&row.before, &row.after])
        .flatten()
        .filter(|value| value.run_week().is_none())
        .filter_map(|value| value.owning_run().map(str::to_owned))
        .collect();
    let mut run_weeks = BTreeMap::new();
    for run_id in owning {
        let week: Option<String> = sqlx::query_scalar("SELECT week_start FROM runs WHERE id = ?1")
            .bind(&run_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(backend)?;
        if let Some(week) = week {
            run_weeks.insert(run_id, from_iso(&week).map_err(backend)?);
        }
    }
    let weeks = touched_weeks(&rows, |run_id| run_weeks.get(run_id).copied());
    let prev = head(conn).await?;
    let request_digest = meta.request_digest.clone();
    let record = ChangeRecord::seal(
        prev.as_ref().map(|(seq, hash)| (*seq, hash.as_str())),
        uuid::Uuid::new_v4().to_string(),
        revision,
        meta.clone(),
        weeks,
        rows,
    )
    .map_err(|error| StoreError::Constraint(error.to_string()))?;
    meta.expect
        .check_overrides_changed(&changed_fields(&record))
        .map_err(StoreError::Precondition)?;
    insert(conn, &record, request_digest.as_deref()).await?;
    insert_fields(conn, &record).await?;
    super::journal::enqueue(conn, &change_source(record.seq), &meta.outbox, &meta.at).await?;
    Ok(Committed {
        seq: record.seq,
        revision: record.revision,
        replayed: false,
    })
}

/// Write the genesis record into an empty history, at the current revision.
pub(super) async fn ensure_genesis(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    let empty: Option<i64> = sqlx::query_scalar("SELECT seq FROM change_log LIMIT 1")
        .fetch_optional(&mut *tx)
        .await?;
    if empty.is_none() {
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM store_meta WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;
        let genesis = ChangeRecord::genesis(u64::try_from(revision).unwrap_or_default())
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        insert(&mut tx, &genesis, None)
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    }
    tx.commit().await
}

/// [`ChangeFilter::Run`]: the run's records from the blame field index (its
/// primary key leads with the target), never a scan of `change_log`.
const RUN_CLAUSE: &str =
    "seq IN (SELECT seq FROM change_fields WHERE target_kind = 'run' AND target_id = ?)";

pub(super) const COLUMNS: &str =
    "seq, id, revision, at, actor_kind, actor_id, surface, request_id, body, hash";

/// Parse a stored record and check its indexed columns agree with its body;
/// returns the record and its stored bytes.
pub(super) fn stored(
    row: &SqliteRow,
    weeks: Option<&BTreeSet<DateTime<Utc>>>,
) -> Result<(ChangeRecord, String), (u64, String)> {
    let seq = row
        .try_get::<i64, _>("seq")
        .ok()
        .and_then(|seq| u64::try_from(seq).ok())
        .unwrap_or_default();
    let fail = |reason: String| (seq, reason);
    let text = |column: &str| -> Result<String, (u64, String)> {
        row.try_get(column).map_err(|error| fail(error.to_string()))
    };
    let body = text("body")?;
    let record =
        ChangeRecord::parse(&body, &text("hash")?).map_err(|error| fail(error.to_string()))?;
    let revision: i64 = row
        .try_get("revision")
        .map_err(|error| fail(error.to_string()))?;
    let request_id: Option<String> = row
        .try_get("request_id")
        .map_err(|error| fail(error.to_string()))?;
    let at = from_iso(&text("at")?).map_err(|error| fail(error.to_string()))?;
    let consistent = record.seq == seq
        && record.id == text("id")?
        && i64::try_from(record.revision).ok() == Some(revision)
        && record.at == at
        && record.origin.actor.kind() == text("actor_kind")?
        && record.origin.actor.id() == text("actor_id")?
        && record.origin.surface.as_str() == text("surface")?
        && record.origin.request_id == request_id
        && weeks
            .is_none_or(|weeks| record.weeks.iter().copied().collect::<BTreeSet<_>>() == *weeks);
    if consistent {
        Ok((record, body))
    } else {
        Err(fail("indexed columns do not match the record".into()))
    }
}

impl ChangeHistory for SqliteStore {
    async fn load_change(&self, seq: u64) -> Result<Option<ChangeRecord>, StoreError> {
        match self.load_checked(seq).await? {
            CheckedChange::Missing => Ok(None),
            CheckedChange::Intact(record) => Ok(Some(*record)),
            CheckedChange::Tampered(reason) => {
                Err(StoreError::Backend(format!("change {seq}: {reason}")))
            }
        }
    }

    async fn load_checked(&self, seq: u64) -> Result<CheckedChange, StoreError> {
        // No stored seq exceeds i64::MAX: such a seq is simply missing.
        let Ok(seq) = i64::try_from(seq) else {
            return Ok(CheckedChange::Missing);
        };
        let row = sqlx::query(&format!("SELECT {COLUMNS} FROM change_log WHERE seq = ?1"))
            .bind(seq)
            .fetch_optional(&self.readers)
            .await
            .map_err(backend)?;
        let Some(row) = row else {
            return Ok(CheckedChange::Missing);
        };
        Ok(match stored(&row, None) {
            Err((_, reason)) => CheckedChange::Tampered(reason),
            Ok((record, body)) => match record.check_stored(&body) {
                Ok(()) => CheckedChange::Intact(Box::new(record)),
                Err(reason) => CheckedChange::Tampered(reason),
            },
        })
    }

    async fn list_changes(&self, query: &ChangeQuery) -> Result<ChangePage, StoreError> {
        enum Arg {
            Text(String),
            Int(i64),
        }
        let int = |value: u64| i64::try_from(value).map_err(backend);
        let mut clauses = Vec::new();
        let mut args = Vec::new();
        match &query.filter {
            ChangeFilter::All => {}
            ChangeFilter::Week(week) => {
                clauses.push("seq IN (SELECT seq FROM change_log_weeks WHERE week_start = ?)");
                args.push(Arg::Text(rows::instant(week)?));
            }
            ChangeFilter::Actor(actor) => {
                clauses.push("actor_kind = ? AND actor_id = ?");
                args.push(Arg::Text(actor.kind().to_owned()));
                args.push(Arg::Text(actor.id().to_owned()));
            }
            ChangeFilter::ActorInWeek(actor, week) => {
                clauses.push(
                    "actor_kind = ? AND actor_id = ? \
                     AND seq IN (SELECT seq FROM change_log_weeks WHERE week_start = ?)",
                );
                args.push(Arg::Text(actor.kind().to_owned()));
                args.push(Arg::Text(actor.id().to_owned()));
                args.push(Arg::Text(rows::instant(week)?));
            }
            ChangeFilter::Run(run_id) => {
                clauses.push(RUN_CLAUSE);
                args.push(Arg::Text(run_id.clone()));
            }
            ChangeFilter::Revisions { from, to } => {
                clauses.push("revision BETWEEN ? AND ?");
                args.push(Arg::Int(int(*from)?));
                args.push(Arg::Int(int(*to)?));
            }
        }
        if let Some(cursor) = query.cursor {
            clauses.push(if query.newest_first {
                "seq < ?"
            } else {
                "seq > ?"
            });
            // Clamping keeps the selection: no stored seq exceeds i64::MAX.
            args.push(Arg::Int(i64::try_from(cursor).unwrap_or(i64::MAX)));
        }
        let filter = if clauses.is_empty() {
            "1".to_owned()
        } else {
            clauses.join(" AND ")
        };
        let order = if query.newest_first { "DESC" } else { "ASC" };
        let sql =
            format!("SELECT {COLUMNS} FROM change_log WHERE {filter} ORDER BY seq {order} LIMIT ?");
        args.push(Arg::Int(int(
            u64::try_from(query.page_size() + 1).unwrap_or(u64::MAX)
        )?));
        let mut select = sqlx::query(&sql);
        for arg in args {
            select = match arg {
                Arg::Text(text) => select.bind(text),
                Arg::Int(value) => select.bind(value),
            };
        }
        let found = select.fetch_all(&self.readers).await.map_err(backend)?;
        let records = found
            .iter()
            .map(|row| {
                stored(row, None)
                    .map(|(record, _)| record)
                    .map_err(|(_, reason)| StoreError::Backend(reason))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ChangePage::from_matches(records, query))
    }

    async fn count_changes(&self, filter: &ChangeFilter) -> Result<u64, StoreError> {
        // One constant statement per filter; genesis (seq 0) is never counted.
        let int = |value: u64| i64::try_from(value).map_err(backend);
        let count =
            match filter {
                ChangeFilter::All => {
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM change_log WHERE seq > 0")
                        .fetch_one(&self.readers)
                        .await
                }
                ChangeFilter::Week(week) => {
                    sqlx::query_scalar(
                        "SELECT COUNT(*) FROM change_log WHERE seq > 0 \
                 AND seq IN (SELECT seq FROM change_log_weeks WHERE week_start = ?1)",
                    )
                    .bind(rows::instant(week)?)
                    .fetch_one(&self.readers)
                    .await
                }
                ChangeFilter::Actor(actor) => {
                    sqlx::query_scalar(
                        "SELECT COUNT(*) FROM change_log WHERE seq > 0 \
                 AND actor_kind = ?1 AND actor_id = ?2",
                    )
                    .bind(actor.kind())
                    .bind(actor.id())
                    .fetch_one(&self.readers)
                    .await
                }
                ChangeFilter::ActorInWeek(actor, week) => {
                    sqlx::query_scalar(
                        "SELECT COUNT(*) FROM change_log WHERE seq > 0 \
                 AND actor_kind = ?1 AND actor_id = ?2 \
                 AND seq IN (SELECT seq FROM change_log_weeks WHERE week_start = ?3)",
                    )
                    .bind(actor.kind())
                    .bind(actor.id())
                    .bind(rows::instant(week)?)
                    .fetch_one(&self.readers)
                    .await
                }
                ChangeFilter::Run(run_id) => {
                    sqlx::query_scalar(&format!(
                        "SELECT COUNT(*) FROM change_log WHERE seq > 0 AND {RUN_CLAUSE}"
                    ))
                    .bind(run_id)
                    .fetch_one(&self.readers)
                    .await
                }
                ChangeFilter::Revisions { from, to } => sqlx::query_scalar(
                    "SELECT COUNT(*) FROM change_log WHERE seq > 0 AND revision BETWEEN ?1 AND ?2",
                )
                .bind(int(*from)?)
                .bind(int(*to)?)
                .fetch_one(&self.readers)
                .await,
            }
            .map_err(backend)?;
        u64::try_from(count).map_err(backend)
    }

    async fn verify_history(&self) -> Result<HistoryVerification, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        let mut tx = conn.begin().await.map_err(backend)?;
        let mut weeks: BTreeMap<u64, BTreeSet<DateTime<Utc>>> = BTreeMap::new();
        for row in sqlx::query("SELECT seq, week_start FROM change_log_weeks")
            .fetch_all(&mut *tx)
            .await
            .map_err(backend)?
        {
            let seq: i64 = row.try_get("seq").map_err(backend)?;
            let week: String = row.try_get("week_start").map_err(backend)?;
            weeks
                .entry(u64::try_from(seq).map_err(backend)?)
                .or_default()
                .insert(from_iso(&week).map_err(backend)?);
        }
        let mut index: BTreeMap<u64, BTreeSet<(BlameTarget, String)>> = BTreeMap::new();
        for row in sqlx::query("SELECT target_kind, target_id, field, seq FROM change_fields")
            .fetch_all(&mut *tx)
            .await
            .map_err(backend)?
        {
            let kind: String = row.try_get("target_kind").map_err(backend)?;
            let id: String = row.try_get("target_id").map_err(backend)?;
            let target = match kind.as_str() {
                "run" => BlameTarget::Run(id),
                _ => BlameTarget::FixedRun(id),
            };
            index
                .entry(unsigned(row.try_get("seq").map_err(backend)?)?)
                .or_default()
                .insert((target, row.try_get("field").map_err(backend)?));
        }
        let rows = sqlx::query(&format!("SELECT {COLUMNS} FROM change_log ORDER BY seq"))
            .fetch_all(&mut *tx)
            .await
            .map_err(backend)?;
        let checkpoints = checkpoint_rows(&mut tx, None).await?;
        let _ = tx.rollback().await;
        let empty = BTreeSet::new();
        let no_fields = BTreeSet::new();
        let mut hashes = BTreeMap::new();
        let mut result = verify_chain(rows.iter().map(|row| {
            let seq = row
                .try_get::<i64, _>("seq")
                .ok()
                .and_then(|seq| u64::try_from(seq).ok())
                .unwrap_or_default();
            let (record, body) = stored(row, Some(weeks.get(&seq).unwrap_or(&empty)))?;
            if changed_fields(&record) != *index.get(&seq).unwrap_or(&no_fields) {
                return Err((seq, "blame index does not match the record".to_owned()));
            }
            hashes.insert(seq, (record.hash.clone(), record.revision));
            Ok((record, body))
        }));
        if result.is_intact()
            && let Some(bad) = checkpoints.iter().find(|checkpoint| {
                hashes.get(&checkpoint.head.seq)
                    != Some(&(checkpoint.head.hash.clone(), checkpoint.revision))
            })
        {
            result.first_broken = Some(BrokenLink {
                seq: bad.head.seq,
                reason: format!("checkpoint {:?} does not match its record", bad.name),
            });
        }
        Ok(result)
    }

    async fn history_head(&self) -> Result<ChangeRef, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        let (seq, hash) = head(&mut conn)
            .await?
            .ok_or_else(|| StoreError::Backend("the history has no genesis record".into()))?;
        Ok(ChangeRef { seq, hash })
    }
}

/// For each field of `target` any record set, the last record's `seq`.
async fn last_changes_in(
    conn: &mut SqliteConnection,
    target: &BlameTarget,
) -> Result<BTreeMap<String, u64>, StoreError> {
    let rows = sqlx::query(
        "SELECT field, MAX(seq) AS seq FROM change_fields
         WHERE target_kind = ?1 AND target_id = ?2 GROUP BY field",
    )
    .bind(target.kind())
    .bind(target.id())
    .fetch_all(&mut *conn)
    .await
    .map_err(backend)?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get("field").map_err(backend)?,
                unsigned(row.try_get("seq").map_err(backend)?)?,
            ))
        })
        .collect()
}

async fn versioned_in(
    conn: &mut SqliteConnection,
    targets: &[BlameTarget],
) -> Result<Vec<Versioned>, StoreError> {
    let mut read = Vec::with_capacity(targets.len());
    for target in targets {
        let answers = match target {
            BlameTarget::Run(run_id) => {
                let found = sqlx::query(&format!(
                    "SELECT {RSVP_COLUMNS} FROM rsvps WHERE run_id = ?1 ORDER BY user_id"
                ))
                .bind(run_id)
                .fetch_all(&mut *conn)
                .await
                .map_err(backend)?;
                found
                    .iter()
                    .map(|row| rows::rsvp(row).map(RowValue::Rsvp))
                    .collect::<Result<Vec<_>, _>>()?
            }
            BlameTarget::FixedRun(_) => Vec::new(),
        };
        read.push(Versioned {
            target: target.clone(),
            row: row_value(conn, &target_key(target)).await?,
            answers,
            versions: last_changes_in(conn, target).await?,
        });
    }
    Ok(read)
}

impl BlameIndex for SqliteStore {
    async fn last_changes(
        &self,
        target: &BlameTarget,
    ) -> Result<BTreeMap<String, u64>, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        last_changes_in(&mut conn, target).await
    }

    async fn read_versioned(&self, targets: &[BlameTarget]) -> Result<Vec<Versioned>, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        let mut tx = conn.begin().await.map_err(backend)?;
        let read = versioned_in(&mut tx, targets).await;
        let ended = tx.rollback().await.is_ok();
        if !ended {
            conn.close_on_drop();
        }
        read
    }
}

const CHECKPOINT_COLUMNS: &str =
    "name, kind, seq, hash, revision, week_start, created_at, created_by_kind, created_by_id";

fn checkpoint(row: &SqliteRow) -> Result<Checkpoint, StoreError> {
    let text =
        |column: &str| -> Result<String, StoreError> { row.try_get(column).map_err(backend) };
    let instant = |column: &str| from_iso(&text(column)?).map_err(backend);
    Ok(Checkpoint {
        name: text("name")?,
        kind: CheckpointKind::parse(&text("kind")?)
            .ok_or_else(|| backend("unknown checkpoint kind"))?,
        head: ChangeRef {
            seq: unsigned(row.try_get("seq").map_err(backend)?)?,
            hash: text("hash")?,
        },
        revision: unsigned(row.try_get("revision").map_err(backend)?)?,
        week: instant("week_start")?,
        created_at: instant("created_at")?,
        created_by: Actor::from_parts(&text("created_by_kind")?, &text("created_by_id")?)
            .ok_or_else(|| backend("unknown checkpoint creator"))?,
    })
}

async fn checkpoint_rows(
    conn: &mut SqliteConnection,
    week: Option<&str>,
) -> Result<Vec<Checkpoint>, StoreError> {
    let rows = match week {
        Some(week) => {
            sqlx::query(&format!(
                "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints WHERE week_start = ?1 ORDER BY id"
            ))
            .bind(week)
            .fetch_all(conn)
            .await
        }
        None => {
            sqlx::query(&format!(
                "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints ORDER BY id"
            ))
            .fetch_all(conn)
            .await
        }
    }
    .map_err(backend)?;
    rows.iter().map(checkpoint).collect()
}

/// Inside a write transaction: the existing automatic checkpoint of the
/// week, or a new one at the current head.
async fn create_checkpoint_in(
    conn: &mut SqliteConnection,
    new: &NewCheckpoint,
) -> Result<CheckpointCreated, StoreError> {
    let week = rows::instant(&new.week)?;
    if new.kind == CheckpointKind::Auto {
        let existing = sqlx::query(&format!(
            "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints
             WHERE kind = 'auto' AND (week_start = ?1 OR name = ?2) ORDER BY id LIMIT 1"
        ))
        .bind(&week)
        .bind(&new.name)
        .fetch_optional(&mut *conn)
        .await
        .map_err(backend)?;
        if let Some(row) = existing {
            return Ok(CheckpointCreated::Existing(checkpoint(&row)?));
        }
    }
    let taken: Option<i64> = sqlx::query_scalar("SELECT id FROM checkpoints WHERE name = ?1")
        .bind(&new.name)
        .fetch_optional(&mut *conn)
        .await
        .map_err(backend)?;
    if taken.is_some() {
        return Err(StoreError::Constraint(format!(
            "checkpoint name {:?} is taken",
            new.name
        )));
    }
    let head = sqlx::query("SELECT seq, hash, revision FROM change_log ORDER BY seq DESC LIMIT 1")
        .fetch_one(&mut *conn)
        .await
        .map_err(backend)?;
    let created = Checkpoint {
        name: new.name.clone(),
        kind: new.kind,
        head: ChangeRef {
            seq: unsigned(head.try_get("seq").map_err(backend)?)?,
            hash: head.try_get("hash").map_err(backend)?,
        },
        revision: unsigned(head.try_get("revision").map_err(backend)?)?,
        week: new.week,
        created_at: new.created_at,
        created_by: new.created_by.clone(),
    };
    sqlx::query(&format!(
        "INSERT INTO checkpoints ({CHECKPOINT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
    ))
    .bind(&created.name)
    .bind(created.kind.as_str())
    .bind(i64::try_from(created.head.seq).map_err(backend)?)
    .bind(&created.head.hash)
    .bind(i64::try_from(created.revision).map_err(backend)?)
    .bind(&week)
    .bind(rows::instant(&created.created_at)?)
    .bind(created.created_by.kind())
    .bind(created.created_by.id())
    .execute(&mut *conn)
    .await
    .map_err(backend)?;
    Ok(CheckpointCreated::Created(created))
}

impl Checkpoints for SqliteStore {
    async fn create_checkpoint(&self, new: NewCheckpoint) -> Result<CheckpointCreated, StoreError> {
        check_checkpoint(&new)?;
        let mut lease = self.writer.lease().await.map_err(backend)?;
        let (result, healthy) = match lease.conn().begin_with("BEGIN IMMEDIATE").await {
            Ok(mut tx) => match create_checkpoint_in(&mut tx, &new).await {
                Ok(created) => match tx.commit().await {
                    Ok(()) => (Ok(created), true),
                    Err(error) => (Err(backend(error)), false),
                },
                Err(error) => {
                    let healthy = tx.rollback().await.is_ok();
                    (Err(error), healthy)
                }
            },
            Err(error) => (Err(backend(error)), false),
        };
        lease.finish(healthy);
        self.written().after_if(
            crate::infrastructure::store::Written::Schedule,
            result,
            |created| matches!(created, CheckpointCreated::Created(_)),
        )
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<Checkpoint>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints WHERE name = ?1"
        ))
        .bind(name)
        .fetch_optional(&self.readers)
        .await
        .map_err(backend)?;
        row.as_ref().map(checkpoint).transpose()
    }

    async fn list_checkpoints(
        &self,
        week: Option<DateTime<Utc>>,
    ) -> Result<Vec<Checkpoint>, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(backend)?;
        let week = week.as_ref().map(rows::instant).transpose()?;
        checkpoint_rows(&mut conn, week.as_deref()).await
    }
}
