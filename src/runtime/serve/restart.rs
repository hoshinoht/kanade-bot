//! Setup's "Restart now": a requested restart is the same graceful shutdown a
//! SIGTERM starts; the supervisor (Compose's `restart: unless-stopped`) starts
//! the process again. Offered only where `KANADE_SUPERVISED` says one exists,
//! and only after the configuration the next start will read passes the same
//! checks startup makes.

use std::{collections::BTreeMap, sync::LazyLock};

use tokio::sync::Notify;

use crate::runtime::{
    config::{self, ServeConfig, flag},
    error::Error,
};

/// Set by the image's Compose service: something restarts the process after
/// it exits.
pub const SUPERVISED_KEY: &str = "KANADE_SUPERVISED";

static REQUESTED: LazyLock<Notify> = LazyLock::new(Notify::new);

pub fn supervised(values: &BTreeMap<String, String>) -> Result<bool, Error> {
    Ok(flag(values, SUPERVISED_KEY)?.unwrap_or(false))
}

/// Checks the configuration the next start reads (this process's environment
/// and `kanade.toml` as it is now, with every secret it needs).
pub fn preflight() -> Result<(), Error> {
    let resolved = config::resolve(std::env::vars().collect()).map_err(config::annotate)?;
    ServeConfig::from_mapping(&resolved.values)
        .and_then(|config| config.preflight())
        .map_err(config::annotate)
}

/// Starts the graceful shutdown; a request made before [`requested`] is
/// awaited is kept.
pub fn request() {
    REQUESTED.notify_one();
}

pub async fn requested() {
    REQUESTED.notified().await;
}
