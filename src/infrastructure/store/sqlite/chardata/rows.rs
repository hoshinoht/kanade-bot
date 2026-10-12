//! Row decoding for the character-data file. Errors name the column only,
//! never a stored value (which may be Nexon data).

use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::domain::chardata::{Link, LinkStatus, Post, PostState, Snapshot, parse_date};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

pub(super) const LINK_COLUMNS: &str = "character_id, member_id, ign_key, region, status, \
     requested_by_kind, requested_by_id, requested_at, consent_version, consented_at, \
     linked_at, version";
pub(super) const SNAPSHOT_COLUMNS: &str = "character_id, ocid, ocid_fetched_at, character_name, \
     world_name, character_class, level, combat_power, image_url, data_date, fetched_at, \
     expires_at";
pub(super) const POST_COLUMNS: &str =
    "ref, channel_id, message_id, character_id, posted_at, strip_due_at, state";

fn corrupt(column: &str) -> StoreError {
    StoreError::Backend(format!("chardata column {column} is unreadable"))
}

pub(super) fn at(text: &str) -> Result<DateTime<Utc>, StoreError> {
    crate::domain::time::from_iso(text).map_err(|_| corrupt("timestamp"))
}

fn get<'r, T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>>(
    row: &'r SqliteRow,
    column: &str,
) -> Result<T, StoreError> {
    row.try_get(column).map_err(|_| corrupt(column))
}

fn instant(row: &SqliteRow, column: &str) -> Result<DateTime<Utc>, StoreError> {
    at(&get::<String>(row, column)?)
}

fn optional_instant(row: &SqliteRow, column: &str) -> Result<Option<DateTime<Utc>>, StoreError> {
    get::<Option<String>>(row, column)?
        .as_deref()
        .map(at)
        .transpose()
}

fn number<T: TryFrom<i64>>(value: Option<i64>, column: &str) -> Result<Option<T>, StoreError> {
    value
        .map(|value| T::try_from(value).map_err(|_| corrupt(column)))
        .transpose()
}

pub(super) fn link(row: &SqliteRow) -> Result<Link, StoreError> {
    let status: String = get(row, "status")?;
    let kind: String = get(row, "requested_by_kind")?;
    let id: String = get(row, "requested_by_id")?;
    let version: i64 = get(row, "version")?;
    Ok(Link {
        character_id: get(row, "character_id")?,
        member_id: get(row, "member_id")?,
        ign_key: get(row, "ign_key")?,
        region: get(row, "region")?,
        status: LinkStatus::parse(&status).ok_or_else(|| corrupt("status"))?,
        requested_by: Actor::from_parts(&kind, &id).ok_or_else(|| corrupt("requested_by"))?,
        requested_at: instant(row, "requested_at")?,
        consent_version: number(get(row, "consent_version")?, "consent_version")?,
        consented_at: optional_instant(row, "consented_at")?,
        linked_at: optional_instant(row, "linked_at")?,
        version: number(Some(version), "version")?.ok_or_else(|| corrupt("version"))?,
    })
}

pub(super) fn snapshot(row: &SqliteRow) -> Result<Snapshot, StoreError> {
    let date: Option<String> = get(row, "data_date")?;
    Ok(Snapshot {
        character_id: get(row, "character_id")?,
        ocid: get(row, "ocid")?,
        ocid_fetched_at: instant(row, "ocid_fetched_at")?,
        character_name: get(row, "character_name")?,
        world_name: get(row, "world_name")?,
        character_class: get(row, "character_class")?,
        level: number(get(row, "level")?, "level")?,
        combat_power: number(get(row, "combat_power")?, "combat_power")?,
        image_url: get(row, "image_url")?,
        data_date: date
            .as_deref()
            .map(|text| parse_date(text).ok_or_else(|| corrupt("data_date")))
            .transpose()?,
        fetched_at: instant(row, "fetched_at")?,
        expires_at: instant(row, "expires_at")?,
    })
}

pub(super) fn post(row: &SqliteRow) -> Result<Post, StoreError> {
    let state: String = get(row, "state")?;
    Ok(Post {
        reference: get(row, "ref")?,
        channel_id: get(row, "channel_id")?,
        message_id: get(row, "message_id")?,
        character_id: get(row, "character_id")?,
        posted_at: instant(row, "posted_at")?,
        strip_due_at: instant(row, "strip_due_at")?,
        state: PostState::parse(&state).ok_or_else(|| corrupt("state"))?,
    })
}
