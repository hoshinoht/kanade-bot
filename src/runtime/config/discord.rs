//! Bot token and the one-gateway-session guard.

use std::{collections::BTreeMap, path::PathBuf};

use super::{Error, non_empty};
use crate::runtime::secrets::Redacted;

const TOKEN_FILE: &str = "KANADE_DISCORD_TOKEN_FILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordSettings {
    pub token_file: PathBuf,
}

impl DiscordSettings {
    /// Live serve only: v5 shares v4's bot token, and Discord allows one
    /// gateway session per token.
    pub(super) fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        refuse_plain_token(values)?;
        if non_empty(values, "KANADE_EXPECT_V4_STOPPED") != Some("1") {
            return Err(Error::Configuration(
                "KANADE_EXPECT_V4_STOPPED must be 1: stop the v4 container first".into(),
            ));
        }
        let token_file = non_empty(values, TOKEN_FILE)
            .ok_or_else(|| Error::Configuration(format!("{TOKEN_FILE} is required")))?;
        Ok(Self {
            token_file: PathBuf::from(token_file),
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
