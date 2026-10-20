//! Bot token and the gateway switch.

use std::collections::BTreeMap;

use super::{Error, flag, refuse_plain};
use crate::runtime::secrets::{self, Redacted, SecretFile};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordSettings {
    /// `discord_token` in the secrets directory; required.
    pub token: SecretFile,
    /// `KANADE_DISCORD_GATEWAY` (default `1`); `0` serves the admin API only,
    /// with no gateway, roster sync or delivery tick.
    pub gateway: bool,
}

impl DiscordSettings {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        refuse_plain_token(values)?;
        Ok(Self {
            token: SecretFile::named(values, secrets::DISCORD_TOKEN),
            gateway: flag(values, "KANADE_DISCORD_GATEWAY")?.unwrap_or(true),
        })
    }

    pub fn read_token(&self) -> Result<Redacted, Error> {
        self.token.read("the bot signs in to Discord with it")
    }
}

/// Refused in every command so a token never sits in the process environment.
pub(super) fn refuse_plain_token(values: &BTreeMap<String, String>) -> Result<(), Error> {
    refuse_plain(values, "KANADE_DISCORD_TOKEN", secrets::DISCORD_TOKEN)?;
    refuse_plain(values, "DISCORD_TOKEN", secrets::DISCORD_TOKEN)
}
