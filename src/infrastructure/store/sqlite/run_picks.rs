//! Run picks over `roster_run_picks` (0039): constant SQL, reads on a
//! reader connection, writes inside the caller's `BEGIN IMMEDIATE` with
//! their History row (`settings_changes`, section `characters`).

use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use super::rows::instant;
use super::schedule::store_error;
use super::{characters, settings};
use crate::domain::characters::{CharacterRecord, PickChange, PickWrite, RunPick, plan_pick};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

const COLUMNS: &str =
    "run_id, member_id, character_id, picked_at, picked_by_kind, picked_by_id, version";

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("roster_run_picks.{column} is unreadable: {detail}"))
}

fn decode(row: &SqliteRow) -> Result<RunPick, StoreError> {
    let text = |column: &str| -> Result<String, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let version: i64 = row
        .try_get("version")
        .map_err(|error| corrupt("version", error))?;
    let kind = text("picked_by_kind")?;
    Ok(RunPick {
        run_id: text("run_id")?,
        member_id: text("member_id")?,
        character_id: text("character_id")?,
        picked_at: crate::domain::time::from_iso(&text("picked_at")?)
            .map_err(|error| corrupt("picked_at", error))?,
        picked_by: Actor::from_parts(&kind, &text("picked_by_id")?)
            .ok_or_else(|| corrupt("picked_by_kind", kind))?,
        version: u32::try_from(version).map_err(|error| corrupt("version", error))?,
    })
}

async fn load(
    conn: &mut SqliteConnection,
    run_id: &str,
    member_id: &str,
) -> Result<Option<RunPick>, StoreError> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM roster_run_picks WHERE run_id = ?1 AND member_id = ?2"
    ))
    .bind(run_id)
    .bind(member_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(decode)
    .transpose()
}

/// The picks on `run_ids` (a JSON array bound as one parameter, so the
/// statement stays constant).
pub(super) async fn of_runs(
    conn: &mut SqliteConnection,
    run_ids: &[String],
) -> Result<Vec<RunPick>, StoreError> {
    let ids = serde_json::to_string(run_ids)
        .map_err(|error| StoreError::Backend(format!("run ids: {error}")))?;
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM roster_run_picks \
         WHERE run_id IN (SELECT value FROM json_each(?1)) ORDER BY run_id, member_id"
    ))
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?
    .iter()
    .map(decode)
    .collect()
}

async fn put(conn: &mut SqliteConnection, pick: &RunPick) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO roster_run_picks (run_id, member_id, character_id, picked_at, \
         picked_by_kind, picked_by_id, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT (run_id, member_id) DO UPDATE SET character_id = excluded.character_id, \
         picked_at = excluded.picked_at, picked_by_kind = excluded.picked_by_kind, \
         picked_by_id = excluded.picked_by_id, version = excluded.version",
    )
    .bind(&pick.run_id)
    .bind(&pick.member_id)
    .bind(&pick.character_id)
    .bind(instant(&pick.picked_at)?)
    .bind(pick.picked_by.kind())
    .bind(pick.picked_by.id())
    .bind(i64::from(pick.version))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

/// One pick write and its History row, inside the caller's transaction;
/// also whether a History row was appended.
pub(super) async fn apply(
    conn: &mut SqliteConnection,
    change: PickChange,
    record: &CharacterRecord,
) -> Result<(PickWrite, bool), StoreError> {
    let (run_id, member_id) = change.slot();
    let current = load(conn, run_id, member_id).await?;
    let current_character = match &current {
        Some(pick) => characters::load(conn, &pick.character_id).await?,
        None => None,
    };
    let character = match change.character_id() {
        Some(id) => characters::load(conn, id).await?,
        None => None,
    };
    let planned = plan_pick(
        change,
        current,
        current_character.as_ref(),
        character.as_ref(),
        record,
    )?;
    match (&planned.before, &planned.after) {
        (_, Some(after)) => put(conn, after).await?,
        (Some(before), None) => {
            sqlx::query("DELETE FROM roster_run_picks WHERE run_id = ?1 AND member_id = ?2")
                .bind(&before.run_id)
                .bind(&before.member_id)
                .execute(&mut *conn)
                .await
                .map_err(store_error)?;
        }
        (None, None) => {}
    }
    if let Some(change) = &planned.change {
        settings::append(conn, change).await?;
    }
    Ok((planned.written, planned.change.is_some()))
}
