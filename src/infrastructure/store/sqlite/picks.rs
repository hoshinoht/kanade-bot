//! Character picks over `roster_run_picks` (0039) and `roster_fixed_picks`
//! (0040): constant SQL per table, reads on a reader connection, writes
//! inside the caller's `BEGIN IMMEDIATE` with their History row
//! (`settings_changes`, section `characters`).

use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use super::rows::instant;
use super::schedule::store_error;
use super::{characters, settings};
use crate::domain::characters::{
    CharacterRecord, Pick, PickChange, PickScope, PickWrite, plan_pick,
};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

/// One scope's table: its name and every statement over it, all constant.
struct Table {
    name: &'static str,
    load: &'static str,
    of_targets: &'static str,
    put: &'static str,
    delete: &'static str,
}

const RUNS: Table = Table {
    name: "roster_run_picks",
    load: "SELECT run_id AS target, member_id, character_id, picked_at, picked_by_kind, \
           picked_by_id, version FROM roster_run_picks WHERE run_id = ?1 AND member_id = ?2",
    of_targets: "SELECT run_id AS target, member_id, character_id, picked_at, picked_by_kind, \
                 picked_by_id, version FROM roster_run_picks \
                 WHERE run_id IN (SELECT value FROM json_each(?1)) ORDER BY run_id, member_id",
    put: "INSERT INTO roster_run_picks (run_id, member_id, character_id, picked_at, \
          picked_by_kind, picked_by_id, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
          ON CONFLICT (run_id, member_id) DO UPDATE SET character_id = excluded.character_id, \
          picked_at = excluded.picked_at, picked_by_kind = excluded.picked_by_kind, \
          picked_by_id = excluded.picked_by_id, version = excluded.version",
    delete: "DELETE FROM roster_run_picks WHERE run_id = ?1 AND member_id = ?2",
};

const TIMINGS: Table = Table {
    name: "roster_fixed_picks",
    load: "SELECT fixed_run_id AS target, member_id, character_id, picked_at, picked_by_kind, \
           picked_by_id, version FROM roster_fixed_picks \
           WHERE fixed_run_id = ?1 AND member_id = ?2",
    of_targets: "SELECT fixed_run_id AS target, member_id, character_id, picked_at, \
                 picked_by_kind, picked_by_id, version FROM roster_fixed_picks \
                 WHERE fixed_run_id IN (SELECT value FROM json_each(?1)) \
                 ORDER BY fixed_run_id, member_id",
    put: "INSERT INTO roster_fixed_picks (fixed_run_id, member_id, character_id, picked_at, \
          picked_by_kind, picked_by_id, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
          ON CONFLICT (fixed_run_id, member_id) DO UPDATE SET \
          character_id = excluded.character_id, picked_at = excluded.picked_at, \
          picked_by_kind = excluded.picked_by_kind, picked_by_id = excluded.picked_by_id, \
          version = excluded.version",
    delete: "DELETE FROM roster_fixed_picks WHERE fixed_run_id = ?1 AND member_id = ?2",
};

fn table(scope: PickScope) -> &'static Table {
    match scope {
        PickScope::Run => &RUNS,
        PickScope::Timing => &TIMINGS,
    }
}

fn decode(scope: PickScope, row: &SqliteRow) -> Result<Pick, StoreError> {
    let corrupt = |column: &str, detail: &dyn std::fmt::Display| {
        StoreError::Backend(format!(
            "{}.{column} is unreadable: {detail}",
            table(scope).name
        ))
    };
    let text = |column: &str| -> Result<String, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, &error))
    };
    let version: i64 = row
        .try_get("version")
        .map_err(|error| corrupt("version", &error))?;
    let kind = text("picked_by_kind")?;
    Ok(Pick {
        scope,
        target: text("target")?,
        member_id: text("member_id")?,
        character_id: text("character_id")?,
        picked_at: crate::domain::time::from_iso(&text("picked_at")?)
            .map_err(|error| corrupt("picked_at", &error))?,
        picked_by: Actor::from_parts(&kind, &text("picked_by_id")?)
            .ok_or_else(|| corrupt("picked_by_kind", &kind))?,
        version: u32::try_from(version).map_err(|error| corrupt("version", &error))?,
    })
}

async fn load(
    conn: &mut SqliteConnection,
    scope: PickScope,
    target: &str,
    member_id: &str,
) -> Result<Option<Pick>, StoreError> {
    sqlx::query(table(scope).load)
        .bind(target)
        .bind(member_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?
        .as_ref()
        .map(|row| decode(scope, row))
        .transpose()
}

/// The picks of `scope` on `targets` (a JSON array bound as one parameter,
/// so the statement stays constant).
pub(super) async fn of_targets(
    conn: &mut SqliteConnection,
    scope: PickScope,
    targets: &[String],
) -> Result<Vec<Pick>, StoreError> {
    let ids = serde_json::to_string(targets)
        .map_err(|error| StoreError::Backend(format!("pick targets: {error}")))?;
    sqlx::query(table(scope).of_targets)
        .bind(ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?
        .iter()
        .map(|row| decode(scope, row))
        .collect()
}

async fn put(conn: &mut SqliteConnection, pick: &Pick) -> Result<(), StoreError> {
    sqlx::query(table(pick.scope).put)
        .bind(&pick.target)
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
    let (scope, target, member_id) = change.slot();
    let current = load(conn, scope, target, member_id).await?;
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
            sqlx::query(table(before.scope).delete)
                .bind(&before.target)
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
