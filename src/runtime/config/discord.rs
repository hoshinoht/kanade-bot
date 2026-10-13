//! Bot token and the gateway switch.

use std::{collections::BTreeMap, path::PathBuf};

use super::{Error, non_empty};
use crate::runtime::secrets::Redacted;

const TOKEN_FILE: &str = "KANADE_DISCORD_TOKEN_FILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordSettings {
    pub token_file: PathBuf,
    /// `KANADE_DISCORD_GATEWAY` (default `1`); `0` serves the admin API only,
    /// with no gateway, roster sync or delivery tick.
    pub gateway: bool,
}

fn flag(values: &BTreeMap<String, String>, key: &str, default: bool) -> Result<bool, Error> {
    match non_empty(values, key) {
        None => Ok(default),
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(Error::Configuration(format!("{key} must be 0 or 1"))),
    }
}

impl DiscordSettings {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        refuse_plain_token(values)?;
        let gateway = flag(values, "KANADE_DISCORD_GATEWAY", true)?;
        let token_file = non_empty(values, TOKEN_FILE)
            .ok_or_else(|| Error::Configuration(format!("{TOKEN_FILE} is required")))?;
        Ok(Self {
            token_file: PathBuf::from(token_file),
            gateway,
        })
    }

    pub fn read_token(&self) -> Result<Redacted, Error> {
        Redacted::read(&self.token_file, TOKEN_FILE)
    }
}

/// Refused in every command so a token never sits in the process environment.
pub(super) fn refuse_plain_token(values: &BTreeMap<String, String>) -> Result<(), Error> {
    for plain in ["KANADE_DISCORD_TOKEN", "DISCORD_TOKEN"] {
        if non_empty(values, plain).is_some() {
            return Err(Error::Configuration(format!(
                "{plain} is not read; use {TOKEN_FILE}"
            )));
        }
    }
    Ok(())
}
