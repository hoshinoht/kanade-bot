//! [`ScheduleStore`] over SQLite.
//!
//! A commit collapses the change set to one final value per key, deletes every
//! touched key, then inserts the surviving rows. SQLite checks UNIQUE per
//! statement, so this order lets one commit swap `(fixed_run_id, week_start)`
//! or rebuild a `(run_id, kind)`; run foreign keys are deferred to COMMIT.

use std::collections::BTreeMap;

use sqlx::error::ErrorKind;
use sqlx::sqlite::{Sqlite, SqliteRow};
use sqlx::{Connection, Row, SqliteConnection};

use super::journal::UNPROVEN_REMINDER;
use super::rows::{self, FIXED_COLUMNS, REMINDER_COLUMNS, RSVP_COLUMNS, RUN_COLUMNS};
use super::{SqliteStore, history};
use crate::domain::attendance::PastRun;
use crate::domain::history::{Actor, ChangeMeta};
use crate::domain::schedule::{Change, ChangeSet, FixedRun, Reminder, Rsvp, Run, ScheduleSnapshot};
use crate::domain::scheduler::{
    AttendanceHistory, Committed, RecordedRequest, ScheduleStore, Scope, StoreError,
};
use crate::infrastructure::store::history::touched_keys;
use crate::infrastructure::store::order::sort_snapshot;

pub(super) fn store_error(error: sqlx::Error) -> StoreError {
    match &error {
        sqlx::Error::Database(db) => match db.kind() {
            ErrorKind::UniqueViolation
            | ErrorKind::ForeignKeyViolation
            | ErrorKind::NotNullViolation
            | ErrorKind::CheckViolation => StoreError::Constraint(db.message().to_owned()),
            _ => StoreError::Backend(error.to_string()),
        },
        _ => StoreError::Backend(error.to_string()),
    }
}

fn revision_of(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Backend(format!("negative revision {value}")))
}

pub(super) async fn read_revision(conn: &mut SqliteConnection) -> Result<u64, StoreError> {
    let value: i64 = sqlx::query_scalar("SELECT revision FROM store_meta WHERE id = 1")
        .fetch_one(conn)
        .await
        .map_err(store_error)?;
    revision_of(value)
}

async fn fetch<T>(
    conn: &mut SqliteConnection,
    sql: &str,
    ids: &str,
    decode: fn(&SqliteRow) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    sqlx::query(sql)
        .bind(ids)
        .fetch_all(conn)
        .await
        .map_err(store_error)?
        .iter()
        .map(decode)
        .collect()
}

/// The ids of the runs a scope selects, as a JSON array for `json_each`.
async fn scoped_run_ids(conn: &mut SqliteConnection, scope: &Scope) -> Result<String, StoreError> {
    let query = match scope {
        Scope::All => sqlx::query("SELECT id FROM runs"),
        Scope::Weeks(weeks) => {
            // An unencodable instant cannot match a stored week.
            let weeks: Vec<String> = weeks
                .iter()
                .filter_map(|week| rows::instant(week).ok())
                .collect();
            sqlx::query("SELECT id FROM runs WHERE week_start IN (SELECT value FROM json_each(?1))")
                .bind(rows::list(&weeks))
        }
        Scope::Run(id) => sqlx::query("SELECT id FROM runs WHERE id = ?1").bind(id.clone()),
        Scope::Reminder(id) => {
            sqlx::query("SELECT run_id AS id FROM reminders WHERE id = ?1").bind(id.clone())
        }
    };
    let ids: Vec<String> = query
        .fetch_all(conn)
        .await
        .map_err(store_error)?
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<_, _>>()
        .map_err(store_error)?;
    Ok(rows::list(&ids))
}

pub(super) async fn load(
    conn: &mut SqliteConnection,
    scope: &Scope,
) -> Result<ScheduleSnapshot, StoreError> {
    let revision = read_revision(conn).await?;
    let mut fixed_runs: Vec<FixedRun> =
        sqlx::query(&format!("SELECT {FIXED_COLUMNS} FROM fixed_runs"))
            .fetch_all(&mut *conn)
            .await
            .map_err(store_error)?
            .iter()
            .map(rows::fixed_run)
            .collect::<Result<_, _>>()?;
    rows::attach_standing(conn, &mut fixed_runs, None).await?;
    let ids = scoped_run_ids(conn, scope).await?;
    let within = "IN (SELECT value FROM json_each(?1))";
    let mut runs = fetch(
        conn,
        &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id {within}"),
        &ids,
        rows::run,
    )
    .await?;
    rows::attach_attendance(conn, &mut runs, &ids).await?;
    rows::attach_status_pins(conn, &mut runs, &ids).await?;
    let reminders = fetch(
        conn,
        &format!("SELECT {REMINDER_COLUMNS} FROM reminders WHERE run_id {within}"),
        &ids,
        rows::reminder,
    )
    .await?;
    let rsvps = fetch(
        conn,
        &format!("SELECT {RSVP_COLUMNS} FROM rsvps WHERE run_id {within}"),
        &ids,
        rows::rsvp,
    )
    .await?;
    let unproven_retired: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT id FROM reminders WHERE run_id {within} AND {UNPROVEN_REMINDER}"
    ))
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    let mut snapshot = ScheduleSnapshot {
        unproven_retired: unproven_retired.into_iter().collect(),
        revision,
        fixed_runs,
        runs,
        reminders,
        rsvps,
    };
    sort_snapshot(&mut snapshot);
    Ok(snapshot)
}

/// The final value of every key a change set touches; `None` deletes.
#[derive(Default)]
pub(super) struct Collapsed {
    fixed: BTreeMap<String, Option<FixedRun>>,
    runs: BTreeMap<String, Run>,
    reminders: BTreeMap<String, Option<Reminder>>,
    rsvps: BTreeMap<(String, String), Option<Rsvp>>,
}

impl Collapsed {
    pub(super) fn new(changes: ChangeSet) -> Self {
        let mut out = Self::default();
        for change in changes.changes {
            match change {
                Change::PutFixedRun(row) => {
                    out.fixed.insert(row.id.clone(), Some(row));
                }
                Change::DeleteFixedRun(id) => {
                    out.fixed.insert(id, None);
                }
                Change::PutRun(row) => {
                    out.runs.insert(row.id.clone(), row);
                }
                Change::PutReminder(row) => {
                    out.reminders.insert(row.id.clone(), Some(row));
                }
                Change::DeleteReminder(id) => {
                    out.reminders.insert(id, None);
                }
                Change::PutRsvp(row) => {
                    out.rsvps
                        .insert((row.run_id.clone(), row.user_id.clone()), Some(row));
                }
                Change::DeleteRsvp { run_id, user_id } => {
                    out.rsvps.insert((run_id, user_id), None);
                }
            }
        }
        out
    }
}

async fn execute<'q>(
    tx: &mut SqliteConnection,
    query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
) -> Result<(), StoreError> {
    query.execute(tx).await.map(|_| ()).map_err(store_error)
}

pub(super) async fn write(tx: &mut SqliteConnection, rows: Collapsed) -> Result<(), StoreError> {
    let insert_fixed = format!(
        "INSERT INTO fixed_runs ({FIXED_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
    );
    let insert_run =
        format!("INSERT INTO runs ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)");
    let insert_reminder =
        format!("INSERT INTO reminders ({REMINDER_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)");
    let insert_rsvp = format!("INSERT INTO rsvps ({RSVP_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5)");
    for id in rows.fixed.keys() {
        execute(
            tx,
            sqlx::query("DELETE FROM standing_answers WHERE fixed_run_id = ?1").bind(id),
        )
        .await?;
        execute(
            tx,
            sqlx::query("DELETE FROM fixed_runs WHERE id = ?1").bind(id),
        )
        .await?;
    }
    for id in rows.runs.keys() {
        execute(
            tx,
            sqlx::query("DELETE FROM run_attendance WHERE run_id = ?1").bind(id),
        )
        .await?;
        execute(
            tx,
            sqlx::query("DELETE FROM run_status_pins WHERE run_id = ?1").bind(id),
        )
        .await?;
        execute(tx, sqlx::query("DELETE FROM runs WHERE id = ?1").bind(id)).await?;
    }
    for id in rows.reminders.keys() {
        execute(
            tx,
            sqlx::query("DELETE FROM reminders WHERE id = ?1").bind(id),
        )
        .await?;
    }
    for (run_id, user_id) in rows.rsvps.keys() {
        let delete = sqlx::query("DELETE FROM rsvps WHERE run_id = ?1 AND user_id = ?2")
            .bind(run_id)
            .bind(user_id);
        execute(tx, delete).await?;
    }
    for row in rows.fixed.values().flatten() {
        let insert = sqlx::query(&insert_fixed)
            .bind(&row.id)
            .bind(&row.owner_id)
            .bind(&row.channel_id)
            .bind(rows::list(&row.bosses))
            .bind(rows::weekday(row.weekday))
            .bind(rows::wall_time(row.time)?)
            .bind(rows::list(&row.participants))
            .bind(&row.note)
            .bind(row.attendance_default.as_str());
        execute(tx, insert).await?;
        for answer in &row.standing {
            let insert = sqlx::query(
                "INSERT INTO standing_answers (fixed_run_id, user_id, set_by, at) \
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(&row.id)
            .bind(&answer.user_id)
            .bind(&answer.set_by)
            .bind(rows::instant(&answer.at)?);
            execute(tx, insert).await?;
        }
    }
    for row in rows.runs.values() {
        let insert = sqlx::query(&insert_run)
            .bind(&row.id)
            .bind(&row.fixed_run_id)
            .bind(&row.channel_id)
            .bind(rows::instant(&row.week_start)?)
            .bind(rows::instant(&row.datetime)?)
            .bind(rows::list(&row.bosses))
            .bind(rows::list(&row.participants))
            .bind(row.status.as_str())
            .bind(row.source.as_str());
        execute(tx, insert).await?;
        for record in &row.attendance {
            let insert = sqlx::query(
                "INSERT INTO run_attendance (run_id, user_id, attended, recorded_by, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(&row.id)
            .bind(&record.user_id)
            .bind(i64::from(record.attended))
            .bind(&record.recorded_by)
            .bind(rows::instant(&record.at)?);
            execute(tx, insert).await?;
        }
        if let Some(pin) = &row.status_pin {
            let insert =
                sqlx::query("INSERT INTO run_status_pins (run_id, status, at) VALUES (?1, ?2, ?3)")
                    .bind(&row.id)
                    .bind(pin.status.as_str())
                    .bind(rows::instant(&pin.at)?);
            execute(tx, insert).await?;
        }
    }
    for row in rows.reminders.values().flatten() {
        let insert = sqlx::query(&insert_reminder)
            .bind(&row.id)
            .bind(&row.run_id)
            .bind(&row.kind)
            .bind(rows::instant(&row.fire_at)?)
            .bind(rows::optional_instant(row.sent_at.as_ref())?)
            .bind(&row.message_id);
        execute(tx, insert).await?;
    }
    for row in rows.rsvps.values().flatten() {
        let insert = sqlx::query(&insert_rsvp)
            .bind(&row.run_id)
            .bind(&row.user_id)
            .bind(row.state.as_str())
            .bind(row.source.as_str())
            .bind(rows::instant(&row.at)?);
        execute(tx, insert).await?;
    }
    execute(
        tx,
        sqlx::query("UPDATE store_meta SET revision = revision + 1 WHERE id = 1"),
    )
    .await
}

enum Outcome {
    Written(Committed),
    Replayed(Committed),
    Unchanged,
}

/// One commit on the writer. The flag is false when the connection's
/// transaction state is unknown (a failed COMMIT or ROLLBACK, e.g. after
/// SQLite rolled back on its own), so the caller must replace it.
async fn commit_on(
    conn: &mut SqliteConnection,
    expected_revision: u64,
    changes: ChangeSet,
    meta: ChangeMeta,
) -> (Result<Option<Committed>, StoreError>, bool) {
    // A failed BEGIN (busy, or a worker that ran it but never answered)
    // leaves the connection's transaction depth unknown.
    let mut tx = match conn.begin_with("BEGIN IMMEDIATE").await {
        Ok(tx) => tx,
        Err(error) => return (Err(store_error(error)), false),
    };
    let planned = async {
        if let Some(earlier) = history::replayed(&mut tx, &meta).await? {
            return Ok(Outcome::Replayed(earlier));
        }
        history::check_expect(&mut tx, &meta).await?;
        let found = read_revision(&mut tx).await?;
        if found != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                found,
            });
        }
        if changes.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        let keys = touched_keys(&changes);
        let before = history::row_values(&mut tx, &keys).await?;
        write(&mut tx, Collapsed::new(changes)).await?;
        let after = history::row_values(&mut tx, &keys).await?;
        let committed = history::append(&mut tx, found + 1, meta, &before, &after).await?;
        Ok(Outcome::Written(committed))
    }
    .await;
    match planned {
        Ok(Outcome::Written(committed)) => match tx.commit().await {
            Ok(()) => (Ok(Some(committed)), true),
            Err(error) => (Err(store_error(error)), false),
        },
        Ok(Outcome::Replayed(committed)) => {
            let healthy = tx.rollback().await.is_ok();
            (Ok(Some(committed)), healthy)
        }
        Ok(Outcome::Unchanged) => {
            let healthy = tx.rollback().await.is_ok();
            (Ok(None), healthy)
        }
        Err(error) => {
            let healthy = tx.rollback().await.is_ok();
            (Err(error), healthy)
        }
    }
}

impl AttendanceHistory for SqliteStore {
    async fn member_history(
        &self,
        member: &str,
        per_timing: usize,
    ) -> Result<Vec<PastRun>, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(store_error)?;
        rows::member_history(&mut conn, member, per_timing).await
    }
}

impl ScheduleStore for SqliteStore {
    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> Result<Option<RecordedRequest>, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(store_error)?;
        history::recorded_request(&mut conn, actor, request_id).await
    }

    async fn load(&self, scope: &Scope) -> Result<ScheduleSnapshot, StoreError> {
        let mut conn = self.readers.acquire().await.map_err(store_error)?;
        let mut tx = conn.begin().await.map_err(store_error)?;
        let snapshot = load(&mut tx, scope).await;
        let ended = tx.rollback().await.is_ok();
        if !ended {
            conn.close_on_drop();
        }
        snapshot
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> Result<Option<Committed>, StoreError> {
        let mut lease = self
            .writer
            .lease()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let (result, healthy) = commit_on(lease.conn(), expected_revision, changes, meta).await;
        lease.finish(healthy);
        result
    }
}
