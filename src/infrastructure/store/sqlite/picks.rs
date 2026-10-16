//! Character picks over `roster_run_picks` (0039), `roster_fixed_picks`
//! (0040) and their per-boss tables `roster_run_boss_picks` /
//! `roster_fixed_boss_picks` (0041): constant SQL per table, reads on a
//! reader connection, writes inside the caller's `BEGIN IMMEDIATE` with
//! their History row (`settings_changes`, section `characters`).

use std::collections::BTreeMap;

use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use super::rows::instant;
use super::schedule::store_error;
use super::{characters, settings};
use crate::domain::characters::{
    CharacterRecord, Pick, PickChange, PickScope, PickWrite, plan_pick, plan_same,
};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

/// One table: its name and every statement over it, all constant. A
/// per-boss table's `load`, `put` and `delete` take the boss third.
struct Table {
    name: &'static str,
    load: &'static str,
    of_targets: &'static str,
    put: &'static str,
    delete: &'static str,
}

const RUNS: Table = Table {
    name: "roster_run_picks",
    load: "SELECT run_id AS target, member_id, NULL AS boss, character_id, picked_at, \
           picked_by_kind, picked_by_id, version FROM roster_run_picks \
           WHERE run_id = ?1 AND member_id = ?2",
    of_targets: "SELECT run_id AS target, member_id, NULL AS boss, character_id, picked_at, \
                 picked_by_kind, picked_by_id, version FROM roster_run_picks \
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
    load: "SELECT fixed_run_id AS target, member_id, NULL AS boss, character_id, picked_at, \
           picked_by_kind, picked_by_id, version FROM roster_fixed_picks \
           WHERE fixed_run_id = ?1 AND member_id = ?2",
    of_targets: "SELECT fixed_run_id AS target, member_id, NULL AS boss, character_id, \
                 picked_at, picked_by_kind, picked_by_id, version FROM roster_fixed_picks \
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

const RUN_BOSSES: Table = Table {
    name: "roster_run_boss_picks",
    load: "SELECT run_id AS target, member_id, boss, character_id, picked_at, \
           picked_by_kind, picked_by_id, version FROM roster_run_boss_picks \
           WHERE run_id = ?1 AND member_id = ?2 AND boss = ?3",
    of_targets: "SELECT run_id AS target, member_id, boss, character_id, picked_at, \
                 picked_by_kind, picked_by_id, version FROM roster_run_boss_picks \
                 WHERE run_id IN (SELECT value FROM json_each(?1)) \
                 ORDER BY run_id, member_id, boss",
    put: "INSERT INTO roster_run_boss_picks (run_id, member_id, boss, character_id, picked_at, \
          picked_by_kind, picked_by_id, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
          ON CONFLICT (run_id, member_id, boss) DO UPDATE SET \
          character_id = excluded.character_id, picked_at = excluded.picked_at, \
          picked_by_kind = excluded.picked_by_kind, picked_by_id = excluded.picked_by_id, \
          version = excluded.version",
    delete: "DELETE FROM roster_run_boss_picks WHERE run_id = ?1 AND member_id = ?2 \
             AND boss = ?3",
};

const TIMING_BOSSES: Table = Table {
    name: "roster_fixed_boss_picks",
    load: "SELECT fixed_run_id AS target, member_id, boss, character_id, picked_at, \
           picked_by_kind, picked_by_id, version FROM roster_fixed_boss_picks \
           WHERE fixed_run_id = ?1 AND member_id = ?2 AND boss = ?3",
    of_targets: "SELECT fixed_run_id AS target, member_id, boss, character_id, picked_at, \
                 picked_by_kind, picked_by_id, version FROM roster_fixed_boss_picks \
                 WHERE fixed_run_id IN (SELECT value FROM json_each(?1)) \
                 ORDER BY fixed_run_id, member_id, boss",
    put: "INSERT INTO roster_fixed_boss_picks (fixed_run_id, member_id, boss, character_id, \
          picked_at, picked_by_kind, picked_by_id, version) \
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
          ON CONFLICT (fixed_run_id, member_id, boss) DO UPDATE SET \
          character_id = excluded.character_id, picked_at = excluded.picked_at, \
          picked_by_kind = excluded.picked_by_kind, picked_by_id = excluded.picked_by_id, \
          version = excluded.version",
    delete: "DELETE FROM roster_fixed_boss_picks WHERE fixed_run_id = ?1 AND member_id = ?2 \
             AND boss = ?3",
};

/// The table of `scope`'s slot picks, or with `per_boss` its per-boss ones.
fn table(scope: PickScope, per_boss: bool) -> &'static Table {
    match (scope, per_boss) {
        (PickScope::Run, false) => &RUNS,
        (PickScope::Timing, false) => &TIMINGS,
        (PickScope::Run, true) => &RUN_BOSSES,
        (PickScope::Timing, true) => &TIMING_BOSSES,
    }
}

fn decode(scope: PickScope, per_boss: bool, row: &SqliteRow) -> Result<Pick, StoreError> {
    let corrupt = |column: &str, detail: &dyn std::fmt::Display| {
        StoreError::Backend(format!(
            "{}.{column} is unreadable: {detail}",
            table(scope, per_boss).name
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
        boss: row
            .try_get("boss")
            .map_err(|error| corrupt("boss", &error))?,
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
    boss: Option<&str>,
) -> Result<Option<Pick>, StoreError> {
    let per_boss = boss.is_some();
    let mut query = sqlx::query(table(scope, per_boss).load)
        .bind(target)
        .bind(member_id);
    if let Some(boss) = boss {
        query = query.bind(boss);
    }
    query
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?
        .as_ref()
        .map(|row| decode(scope, per_boss, row))
        .transpose()
}

/// The picks of `scope` on `targets`, the slots' own then the per-boss
/// ones (a JSON array bound as one parameter, so each statement stays
/// constant).
pub(super) async fn of_targets(
    conn: &mut SqliteConnection,
    scope: PickScope,
    targets: &[String],
) -> Result<Vec<Pick>, StoreError> {
    let ids = serde_json::to_string(targets)
        .map_err(|error| StoreError::Backend(format!("pick targets: {error}")))?;
    let mut picks = Vec::new();
    for per_boss in [false, true] {
        let rows = sqlx::query(table(scope, per_boss).of_targets)
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(store_error)?;
        for row in &rows {
            picks.push(decode(scope, per_boss, row)?);
        }
    }
    Ok(picks)
}

async fn put(conn: &mut SqliteConnection, pick: &Pick) -> Result<(), StoreError> {
    let mut query = sqlx::query(table(pick.scope, pick.boss.is_some()).put)
        .bind(&pick.target)
        .bind(&pick.member_id);
    if let Some(boss) = &pick.boss {
        query = query.bind(boss);
    }
    query
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

async fn delete(conn: &mut SqliteConnection, pick: &Pick) -> Result<(), StoreError> {
    let mut query = sqlx::query(table(pick.scope, pick.boss.is_some()).delete)
        .bind(&pick.target)
        .bind(&pick.member_id);
    if let Some(boss) = &pick.boss {
        query = query.bind(boss);
    }
    query.execute(&mut *conn).await.map_err(store_error)?;
    Ok(())
}

/// One pick write and its History row, inside the caller's transaction;
/// also whether a History row was appended.
pub(super) async fn apply(
    conn: &mut SqliteConnection,
    change: PickChange,
    record: &CharacterRecord,
) -> Result<(PickWrite, bool), StoreError> {
    if let PickChange::Same {
        scope,
        target,
        member_id,
        versions,
    } = &change
    {
        return same(conn, *scope, target, member_id, versions, record).await;
    }
    let (scope, target, member_id) = change.slot();
    let current = load(conn, scope, target, member_id, change.boss()).await?;
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
        (Some(before), None) => delete(conn, before).await?,
        (None, None) => {}
    }
    if let Some(change) = &planned.change {
        settings::append(conn, change).await?;
    }
    Ok((planned.written, planned.change.is_some()))
}

/// "Same for every boss": the member's per-boss picks named in `versions`
/// go in one write with one History row, or none at all.
async fn same(
    conn: &mut SqliteConnection,
    scope: PickScope,
    target: &str,
    member_id: &str,
    versions: &BTreeMap<String, u32>,
    record: &CharacterRecord,
) -> Result<(PickWrite, bool), StoreError> {
    let mut current = Vec::new();
    for boss in versions.keys() {
        if let Some(pick) = load(conn, scope, target, member_id, Some(boss)).await? {
            let character = characters::load(conn, &pick.character_id).await?;
            current.push((pick, character));
        }
    }
    let planned = plan_same(versions, current, record);
    for pick in &planned.cleared {
        delete(conn, pick).await?;
    }
    if let Some(change) = &planned.change {
        settings::append(conn, change).await?;
    }
    Ok((planned.written, planned.change.is_some()))
}
