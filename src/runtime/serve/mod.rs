//! Live `serve`: owns the SQLite store and serves the admin and public
//! routers against it. The Discord gateway, roster sync and delivery tick
//! are not wired yet, so the process runs with Discord disabled.

pub mod api;
mod health;
pub mod settings;
mod store;

use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    api::{server, state::StaticChannels},
    infrastructure::store::SqliteStore,
    runtime::{config::ServeConfig, error::Error, logging},
};

/// How long closing waits for connections a timed-out drain left behind.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

pub async fn run(config: ServeConfig) -> Result<(), Error> {
    serve_until(config, server::wait_for_shutdown()).await
}

/// [`run`] with the shutdown signal supplied (tests).
pub async fn serve_until(
    config: ServeConfig,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    // Checked now so a missing secret fails the deploy, not the gateway later.
    drop(config.discord.read_token()?);
    let store = store::open(&config.store).await?;
    let served = serve_store(&config, store.clone(), shutdown).await;
    store::close(store, CLOSE_WAIT).await;
    served
}

async fn serve_store(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    // No guild cache until the gateway is wired: channel pickers are empty.
    let composition = api::compose(config, store, Arc::new(StaticChannels(Vec::new()))).await?;
    logging::discord_disabled();
    server::serve(&config.runtime, Some(composition.admin), shutdown).await
}

#[cfg(test)]
mod tests;
