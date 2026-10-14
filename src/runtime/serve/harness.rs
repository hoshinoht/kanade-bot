//! Test support: live serve's Discord side (gateway handler, chat, extraction,
//! cards, roster and tick) over a caller's gateway source and transport, with
//! one model stack the caller builds once and shares across many short
//! serves (the V02 quality harness, `examples/v02_quality`). No HTTP listener.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use super::{
    CLOSE_WAIT, api,
    budget::ShutdownClock,
    discord::{self, Discord, GatewayTransport, Wiring},
    health::LiveHealth,
    models, settings, store,
};
use crate::{
    bot::gateway::EventSource,
    infrastructure::{llm::setup::ModelStack, store::SqliteStore},
    runtime::{config::ServeConfig, error::Error},
};

/// The model stack serve would compose over an empty store: env seeds fill
/// every role (`seed_roles`), then the same provider, governor and groups.
/// `None` without `KANADE_MODEL_BASE_URL`. Its listing and catalog refresh
/// are the caller's (`check_startup`, `spawn_catalog_refresh`).
pub fn shared_models(config: &ServeConfig) -> Result<Option<Arc<ModelStack>>, Error> {
    let mut seeded = settings::seed(&config.seeds);
    models::seed_roles(&mut seeded, &config.models, &BTreeMap::new());
    api::model_stack(&config.models, &seeded)
}

/// Run `work` over the configured store before serve opens it.
pub async fn with_store<R>(
    config: &ServeConfig,
    work: impl AsyncFnOnce(&SqliteStore) -> R,
) -> Result<R, Error> {
    let store = store::open(&config.store).await?;
    let result = work(&store).await;
    store::close(store, Duration::ZERO).await;
    Ok(result)
}

/// A started Discord side and the store it owns.
pub struct LiveServe {
    pub discord: Discord,
    pub store: Arc<SqliteStore>,
    composition: api::Composition,
    /// Serve's one shutdown deadline: `Discord::until` starts it on the stop
    /// signal, and the store close gets what is left of it.
    shutdown: ShutdownClock,
}

/// Open the store, compose as serve does (with `models` shared) and start the
/// Discord side; nothing connects until the source yields `READY`.
pub async fn start<S, T>(
    config: &ServeConfig,
    wiring: Wiring<S, T>,
    models: Option<Arc<ModelStack>>,
) -> Result<LiveServe, Error>
where
    S: EventSource + 'static,
    T: GatewayTransport,
{
    let store = store::open(&config.store).await?;
    let shutdown = ShutdownClock::default();
    let mut prepared = discord::prepare(config, wiring.tick);
    prepared.shutdown = shutdown.clone();
    let health = LiveHealth::new(store.clone())
        .with_discord(prepared.probe.clone(), prepared.tick_status.clone())
        .with_extraction(prepared.extraction.clone());
    let composed = api::compose_with(
        config,
        store.clone(),
        prepared.cache.clone(),
        health,
        models,
    )
    .await;
    let mut composition = match composed {
        Ok(composition) => composition,
        Err(error) => {
            store::close(store, CLOSE_WAIT).await;
            return Err(error);
        }
    };
    match discord::start(config, store.clone(), &mut composition, prepared, wiring).await {
        Ok(discord) => Ok(LiveServe {
            discord,
            store,
            composition,
            shutdown,
        }),
        Err(error) => {
            drop(composition);
            store::close(store, CLOSE_WAIT).await;
            Err(error)
        }
    }
}

impl LiveServe {
    /// Stop the Discord side in order under serve's deadline (idempotent
    /// after `Discord::until`), release every handle and close the store
    /// within what the deadline leaves, as `serve_with` does.
    pub async fn close(mut self) {
        self.shutdown.start();
        self.discord.stop().await;
        let Self {
            discord,
            store,
            composition,
            shutdown,
        } = self;
        drop((discord, composition));
        store::close(store, shutdown.store(CLOSE_WAIT)).await;
    }
}
