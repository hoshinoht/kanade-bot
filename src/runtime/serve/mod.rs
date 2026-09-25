//! Live `serve`: owns the SQLite store, serves the admin and public routers
//! against it and, unless `KANADE_DISCORD_GATEWAY=0`, runs the Discord
//! gateway, roster sync and delivery tick for the configured guild.

pub mod api;
mod commands;
pub mod discord;
mod health;
pub mod models;
pub mod settings;
mod store;
mod tick;

use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    api::{auth, server, state::StaticChannels},
    bot::gateway::{EventSource, shard},
    infrastructure::store::SqliteStore,
    runtime::{config::ServeConfig, error::Error, logging},
};

use discord::{GatewayTransport, LateTransport, Wiring};
use health::LiveHealth;

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
    let token = config.discord.read_token()?;
    if !config.discord.gateway {
        drop(token);
        let store = store::open(&config.store).await?;
        let served = serve_store(&config, store.clone(), shutdown).await;
        store::close(store, CLOSE_WAIT).await;
        return served;
    }
    // One gateway session per token: v4 must be stopped before this connects.
    config.discord.require_v4_stopped()?;
    let wiring = Wiring {
        source: shard(token.expose().to_owned()),
        transport: Arc::new(LateTransport::new(token)),
        clock: Arc::new(auth::system_now),
        tick: config.tick,
    };
    serve_with(&config, shutdown, wiring).await
}

/// Live serve over any gateway source and transport; closes the store it
/// opens, after everything that used it has stopped.
pub async fn serve_with<S, T>(
    config: &ServeConfig,
    shutdown: impl Future<Output = ()>,
    wiring: Wiring<S, T>,
) -> Result<(), Error>
where
    S: EventSource + 'static,
    T: GatewayTransport,
{
    let store = store::open(&config.store).await?;
    let served = serve_live(config, store.clone(), shutdown, wiring).await;
    store::close(store, CLOSE_WAIT).await;
    served
}

async fn serve_store(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    // No guild cache without the gateway: channel pickers are empty.
    let health = LiveHealth::new(store.clone());
    let composition =
        api::compose(config, store, Arc::new(StaticChannels(Vec::new())), health).await?;
    logging::discord_disabled();
    server::serve(&config.runtime, Some(composition.admin), shutdown).await
}

async fn serve_live<S, T>(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    shutdown: impl Future<Output = ()>,
    wiring: Wiring<S, T>,
) -> Result<(), Error>
where
    S: EventSource + 'static,
    T: GatewayTransport,
{
    let prepared = discord::prepare(config, wiring.tick);
    let health = LiveHealth::new(store.clone())
        .with_discord(prepared.probe.clone(), prepared.tick_status.clone());
    let composition = api::compose(config, store.clone(), prepared.cache.clone(), health).await?;
    let mut discord = discord::start(config, store, &composition, prepared, wiring).await?;
    // HTTP keeps serving until the Discord side has stopped, then drains.
    let served = server::serve(
        &config.runtime,
        Some(composition.admin),
        discord.until(shutdown),
    )
    .await;
    discord.stop().await;
    discord.result().and(served)
}

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
