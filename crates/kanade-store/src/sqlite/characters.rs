//! `CharacterStore` over `roster_characters` (0037), `roster_run_picks`
//! (0039), `roster_fixed_picks` (0040) and their per-boss tables (0041, the
//! picks all in `picks.rs`): constant SQL, reads on a reader connection, one
//! `BEGIN IMMEDIATE` per write with its History row (`settings_changes`,
//! section `characters`). Deleting a character cascades to its picks.

use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteRow};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use super::{picks, settings};
use crate::Written;
use kanade_domain::characters::{
    Character, CharacterChange, CharacterRecord, CharacterRole, CharacterStore, CharacterWrite,
    Pick, PickChange, PickScope, PickWrite, plan,
};
use kanade_domain::history::Actor;
use kanade_domain::scheduler::StoreError;

const COLUMNS: &str = "id, member_id, ign, world, class, level, role, retired_at, created_at, \
     updated_at, updated_by_kind, updated_by_id, version";

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!(
        "roster_characters.{column} is unreadable: {detail}"
    ))
}

fn decode(row: &SqliteRow) -> Result<Character, StoreError> {
    let text = |column: &str| -> Result<String, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let optional = |column: &str| -> Result<Option<String>, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let at = |text: &str, column: &str| {
        kanade_domain::time::from_iso(text).map_err(|error| corrupt(column, error))
    };
    let level: Option<i64> = row
        .try_get("level")
        .map_err(|error| corrupt("level", error))?;
    let version: i64 = row
        .try_get("version")
        .map_err(|error| corrupt("version", error))?;
    let role = text("role")?;
    let kind = text("updated_by_kind")?;
    Ok(Character {
        id: text("id")?,
        member_id: text("member_id")?,
        ign: text("ign")?,
        world: text("world")?,
        class: optional("class")?,
        level: level
            .map(u16::try_from)
            .transpose()
            .map_err(|error| corrupt("level", error))?,
        role: CharacterRole::parse(&role).ok_or_else(|| corrupt("role", role))?,
        retired_at: optional("retired_at")?
            .map(|text| at(&text, "retired_at"))
            .transpose()?,
        created_at: at(&text("created_at")?, "created_at")?,
        updated_at: at(&text("updated_at")?, "updated_at")?,
        updated_by: Actor::from_parts(&kind, &text("updated_by_id")?)
            .ok_or_else(|| corrupt("updated_by_kind", kind))?,
        version: u32::try_from(version).map_err(|error| corrupt("version", error))?,
    })
}

pub(super) async fn load(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<Character>, StoreError> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM roster_characters WHERE id = ?1"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .as_ref()
    .map(decode)
    .transpose()
}

async fn of_member(
    conn: &mut SqliteConnection,
    member_id: &str,
) -> Result<Vec<Character>, StoreError> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM roster_characters WHERE member_id = ?1 ORDER BY created_at, id"
    ))
    .bind(member_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?
    .iter()
    .map(decode)
    .collect()
}

/// Another row on `row`'s world holds its IGN.
async fn ign_taken(conn: &mut SqliteConnection, row: &Character) -> Result<bool, StoreError> {
    Ok(sqlx::query(
        "SELECT 1 FROM roster_characters WHERE world = ?1 AND ign_key = ?2 AND id <> ?3",
    )
    .bind(&row.world)
    .bind(row.ign_key())
    .bind(&row.id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    .is_some())
}

async fn insert(conn: &mut SqliteConnection, row: &Character) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO roster_characters (id, member_id, ign, ign_key, world, class, level, role, \
         retired_at, created_at, updated_at, updated_by_kind, updated_by_id, version) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )
    .bind(&row.id)
    .bind(&row.member_id)
    .bind(&row.ign)
    .bind(row.ign_key())
    .bind(&row.world)
    .bind(&row.class)
    .bind(row.level.map(i64::from))
    .bind(row.role.as_str())
    .bind(row.retired_at.as_ref().map(instant).transpose()?)
    .bind(instant(&row.created_at)?)
    .bind(instant(&row.updated_at)?)
    .bind(row.updated_by.kind())
    .bind(row.updated_by.id())
    .bind(i64::from(row.version))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

async fn replace(
    conn: &mut SqliteConnection,
    before: &Character,
    after: &Character,
) -> Result<(), StoreError> {
    let written = sqlx::query(
        "UPDATE roster_characters SET ign = ?2, ign_key = ?3, world = ?4, class = ?5, \
         level = ?6, role = ?7, retired_at = ?8, updated_at = ?9, updated_by_kind = ?10, \
         updated_by_id = ?11, version = ?12 WHERE id = ?1 AND version = ?13",
    )
    .bind(&after.id)
    .bind(&after.ign)
    .bind(after.ign_key())
    .bind(&after.world)
    .bind(&after.class)
    .bind(after.level.map(i64::from))
    .bind(after.role.as_str())
    .bind(after.retired_at.as_ref().map(instant).transpose()?)
    .bind(instant(&after.updated_at)?)
    .bind(after.updated_by.kind())
    .bind(after.updated_by.id())
    .bind(i64::from(after.version))
    .bind(i64::from(before.version))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?
    .rows_affected();
    if written != 1 {
        return Err(StoreError::Backend(
            "roster_characters row moved inside its write".into(),
        ));
    }
    Ok(())
}

async fn write(
    conn: &mut SqliteConnection,
    before: Option<&Character>,
    after: Option<&Character>,
) -> Result<(), StoreError> {
    match (before, after) {
        (None, Some(row)) => insert(conn, row).await,
        (Some(before), Some(after)) => replace(conn, before, after).await,
        (Some(row), None) => {
            sqlx::query("DELETE FROM roster_characters WHERE id = ?1")
                .bind(&row.id)
                .execute(&mut *conn)
                .await
                .map_err(store_error)?;
            Ok(())
        }
        (None, None) => Ok(()),
    }
}

/// One character write and its History row, inside the caller's
/// transaction; also whether a History row was appended.
pub(super) async fn apply(
    conn: &mut SqliteConnection,
    change: CharacterChange,
    record: &CharacterRecord,
) -> Result<(CharacterWrite, bool), StoreError> {
    let current = load(conn, change.id()).await?;
    let member = match (change.row(), &current) {
        (Some(row), _) => Some(row.member_id.clone()),
        (None, Some(row)) => Some(row.member_id.clone()),
        (None, None) => None,
    };
    let mut others = match &member {
        Some(member) => of_member(conn, member).await?,
        None => Vec::new(),
    };
    others.retain(|other| other.id != change.id());
    let taken = match change.row() {
        Some(row) => ign_taken(conn, row).await?,
        None => false,
    };
    let planned = plan(change, current, &others, taken, record)?;
    // The old main steps down first: one active main per member at a time.
    if let Some((before, after)) = &planned.demoted {
        replace(conn, before, after).await?;
    }
    write(conn, planned.before.as_ref(), planned.after.as_ref()).await?;
    let change = record.change(&planned.rows());
    if let Some(change) = &change {
        settings::append(conn, change).await?;
    }
    Ok((planned.written, change.is_some()))
}

impl CharacterStore for SqliteStore {
    async fn member_characters(&self, member_id: &str) -> Result<Vec<Character>, StoreError> {
        read_txn!(self, tx, of_member(&mut tx, member_id))
    }

    async fn character(&self, id: &str) -> Result<Option<Character>, StoreError> {
        read_txn!(self, tx, load(&mut tx, id))
    }

    async fn apply_character(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
    ) -> Result<CharacterWrite, StoreError> {
        let (written, recorded) = write_txn!(self, tx, apply(&mut tx, change, &record))?;
        if recorded {
            self.written().notify(Written::Settings);
        }
        Ok(written)
    }

    async fn picks(&self, scope: PickScope, targets: &[String]) -> Result<Vec<Pick>, StoreError> {
        read_txn!(self, tx, picks::of_targets(&mut tx, scope, targets))
    }

    async fn apply_pick(
        &self,
        change: PickChange,
        record: CharacterRecord,
    ) -> Result<PickWrite, StoreError> {
        let (written, recorded) = write_txn!(self, tx, picks::apply(&mut tx, change, &record))?;
        if recorded {
            self.written().notify(Written::Settings);
        }
        Ok(written)
    }
}
