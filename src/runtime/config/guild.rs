//! Guild, role and user snowflakes.

use std::collections::BTreeMap;

use super::{Error, non_empty};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuildSettings {
    pub guild_id: u64,
    pub bossing_role_id: u64,
    pub admin_role_id: Option<u64>,
    pub chat_pilot_role_id: Option<u64>,
    pub debug_user_ids: Vec<u64>,
    /// Where `/debug ping` and `/debug header` post without `channel:`.
    pub test_channel_id: Option<u64>,
}

impl GuildSettings {
    pub(super) fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        Ok(Self {
            guild_id: required(values, "KANADE_GUILD_ID")?,
            bossing_role_id: required(values, "KANADE_BOSSING_ROLE_ID")?,
            admin_role_id: optional(values, "KANADE_ADMIN_ROLE_ID")?,
            chat_pilot_role_id: optional(values, "KANADE_CHAT_PILOT_ROLE_ID")?,
            debug_user_ids: list(values, "KANADE_DEBUG_USER_IDS")?,
            test_channel_id: optional(values, "KANADE_TEST_CHANNEL_ID")?,
        })
    }
}

/// Canonical decimal: no sign, no leading zero, non-zero, fits `u64`.
fn parse(value: &str) -> Option<u64> {
    let canonical = !value.is_empty()
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit());
    canonical.then(|| value.parse().ok()).flatten()
}

fn invalid(key: &str) -> Error {
    Error::Configuration(format!("{key} must be a Discord snowflake"))
}

pub(super) fn required(values: &BTreeMap<String, String>, key: &str) -> Result<u64, Error> {
    optional(values, key)?.ok_or_else(|| Error::Configuration(format!("{key} is required")))
}

pub(super) fn optional(values: &BTreeMap<String, String>, key: &str) -> Result<Option<u64>, Error> {
    non_empty(values, key)
        .map(|value| parse(value).ok_or_else(|| invalid(key)))
        .transpose()
}

/// Comma-separated, blanks around items ignored, duplicates dropped in order.
pub(super) fn list(values: &BTreeMap<String, String>, key: &str) -> Result<Vec<u64>, Error> {
    let Some(text) = non_empty(values, key) else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for item in text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let id = parse(item).ok_or_else(|| {
            Error::Configuration(format!("{key} must be comma-separated Discord snowflakes"))
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}
