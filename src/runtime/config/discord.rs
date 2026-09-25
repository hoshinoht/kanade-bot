//! Bot token and the one-gateway-session guard.

use std::{collections::BTreeMap, path::PathBuf};

use super::{Error, non_empty};
use crate::runtime::secrets::Redacted;

const TOKEN_FILE: &str = "KANADE_DISCORD_TOKEN_FILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordSettings {
    pub token_file: PathBuf,
    /// `KANADE_EXPECT_V4_STOPPED=1`; checked only before the gateway connects.
    pub v4_stopped: bool,
}

impl DiscordSettings {
    pub(super) fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        refuse_plain_token(values)?;
        let v4_stopped = match non_empty(values, "KANADE_EXPECT_V4_STOPPED") {
            None | Some("0") => false,
            Some("1") => true,
            Some(_) => {
                return Err(Error::Configuration(
                    "KANADE_EXPECT_V4_STOPPED must be 0 or 1".into(),
                ));
            }
        };
        let token_file = non_empty(values, TOKEN_FILE)
            .ok_or_else(|| Error::Configuration(format!("{TOKEN_FILE} is required")))?;
        Ok(Self {
            token_file: PathBuf::from(token_file),
            v4_stopped,
        })
    }

    /// Before connecting the gateway: v5 shares v4's bot token, and Discord
    /// allows one gateway session per token.
    pub fn require_v4_stopped(&self) -> Result<(), Error> {
        if self.v4_stopped {
            Ok(())
        } else {
            Err(Error::Configuration(
                "KANADE_EXPECT_V4_STOPPED must be 1: stop the v4 container first".into(),
            ))
        }
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
