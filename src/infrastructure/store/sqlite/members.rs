//! `MemberStore` over `members` (0001 + 0010) and `member_aliases`: constant
//! SQL, reads on a reader connection, one `BEGIN IMMEDIATE` per write.

use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::list;
use super::schedule::store_error;
use crate::domain::members::{Member, MemberProfile, MemberStore, PingLevel, is_valid_alias};
use crate::domain::scheduler::StoreError;

const COLUMNS: &str = "user_id, display_name, nickname, has_role, is_bot, ping_level, aliases, \
    reply_style, roles, is_guild_admin";

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("members.{column} is unreadable: {detail}"))
}

fn get<T>(row: &SqliteRow, column: &str) -> Result<T, StoreError>
where
    T: for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(column).map_err(|error| corrupt(column, error))
}

fn json_list(row: &SqliteRow, column: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(&get::<String>(row, column)?).map_err(|error| corrupt(column, error))
}

fn profile_of(row: &SqliteRow) -> Result<MemberProfile, StoreError> {
    let level: String = get(row, "ping_level")?;
    Ok(MemberProfile {
        member: Member {
            user_id: get(row, "user_id")?,
            display_name: get(row, "display_name")?,
            nickname: get(row, "nickname")?,
            has_role: get(row, "has_role")?,
            is_bot: get(row, "is_bot")?,
            ping_level: PingLevel::parse_stored(&level)
                .map_err(|error| corrupt("ping_level", error))?,
        },
        aliases: json_list(row, "aliases")?,
        reply_style: get(row, "reply_style")?,
        roles: json_list(row, "roles")?,
        is_guild_admin: get(row, "is_guild_admin")?,
    })
}

async fn all(conn: &mut SqliteConnection) -> Result<Vec<MemberProfile>, StoreError> {
    sqlx::query(&format!("SELECT {COLUMNS} FROM members ORDER BY user_id"))
        .fetch_all(&mut *conn)
        .await
        .map_err(store_error)?
        .iter()
        .map(profile_of)
        .collect()
}

async fn one(
    conn: &mut SqliteConnection,
    user_id: &str,
) -> Result<Option<MemberProfile>, StoreError> {
    sqlx::query(&format!("SELECT {COLUMNS} FROM members WHERE user_id = ?1"))
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(store_error)?
        .as_ref()
        .map(profile_of)
        .transpose()
}

async fn put(conn: &mut SqliteConnection, profile: &MemberProfile) -> Result<(), StoreError> {
    if let Some(bad) = profile.aliases.iter().find(|alias| !is_valid_alias(alias)) {
        return Err(StoreError::Constraint(format!(
            "alias {bad:?} is not one lowercase word"
        )));
    }
    let member = &profile.member;
    // An upsert, not a replace: a REPLACE would cascade-delete the aliases first.
    sqlx::query(
        "INSERT INTO members (user_id, display_name, nickname, has_role, is_bot, ping_level, \
         aliases, reply_style, roles, is_guild_admin) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT (user_id) DO UPDATE SET display_name = excluded.display_name, \
         nickname = excluded.nickname, has_role = excluded.has_role, is_bot = excluded.is_bot, \
         ping_level = excluded.ping_level, aliases = excluded.aliases, \
         reply_style = excluded.reply_style, roles = excluded.roles, \
         is_guild_admin = excluded.is_guild_admin",
    )
    .bind(&member.user_id)
    .bind(&member.display_name)
    .bind(&member.nickname)
    .bind(member.has_role)
    .bind(member.is_bot)
    .bind(member.ping_level.as_str())
    .bind(list(&profile.aliases))
    .bind(&profile.reply_style)
    .bind(list(&profile.roles))
    .bind(profile.is_guild_admin)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    sqlx::query("DELETE FROM member_aliases WHERE user_id = ?1")
        .bind(&member.user_id)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    for alias in &profile.aliases {
        sqlx::query("INSERT INTO member_aliases (alias, user_id) VALUES (?1, ?2)")
            .bind(alias)
            .bind(&member.user_id)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

impl MemberStore for SqliteStore {
    async fn list_members(&self) -> Result<Vec<MemberProfile>, StoreError> {
        read_txn!(self, tx, all(&mut tx))
    }

    async fn load_member(&self, user_id: &str) -> Result<Option<MemberProfile>, StoreError> {
        read_txn!(self, tx, one(&mut tx, user_id))
    }

    async fn put_member(&self, profile: MemberProfile) -> Result<(), StoreError> {
        write_txn!(self, tx, put(&mut tx, &profile))
    }
}
