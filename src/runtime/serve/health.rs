//! Live `/healthz`: the store answers a read, the gateway is ready and the
//! delivery tick keeps completing; `extraction` is informational and never
//! degrades the status. Without the `bot` component Discord and the
//! scheduler report `disabled` and only storage decides. The bot half
//! supplies its probes ([`DiscordHealth`], [`ExtractionHealth`]).

use std::{future::Future, pin::Pin, sync::Arc};

use crate::{
    chat::driver::ChatHandle,
    runtime::application::{DroppedHealth, Health, HealthFuture, HealthProbe},
};
use kanade_store::SqliteStore;

/// The gateway and the delivery tick, as health reads them.
pub trait DiscordHealth: Send + Sync {
    /// The connection's state, whether it counts as ready, and the events
    /// refused for another guild or no guild.
    fn gateway(&self) -> (&'static str, bool, DroppedHealth);
    /// The tick's state and the seconds since its last completed tick.
    fn scheduler(&self) -> (&'static str, Option<u64>);
}

/// Extraction's state, as health reads it.
pub trait ExtractionHealth: Send + Sync {
    fn state<'a>(
        &'a self,
        store: &'a SqliteStore,
    ) -> Pin<Box<dyn Future<Output = &'static str> + Send + 'a>>;
}

pub struct LiveHealth {
    store: Arc<SqliteStore>,
    discord: Option<Arc<dyn DiscordHealth>>,
    chat: Arc<ChatHandle>,
    extraction: Option<Arc<dyn ExtractionHealth>>,
}

impl LiveHealth {
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self {
            store,
            discord: None,
            chat: Arc::default(),
            extraction: None,
        }
    }

    /// Filled once serve starts chat; shared with the API and `/limits`.
    pub fn chat(&self) -> Arc<ChatHandle> {
        Arc::clone(&self.chat)
    }

    #[cfg(feature = "bot")]
    pub(in crate::runtime::serve) fn probing_discord(
        mut self,
        probe: Arc<dyn DiscordHealth>,
    ) -> Self {
        self.discord = Some(probe);
        self
    }

    #[cfg(feature = "bot")]
    pub(in crate::runtime::serve) fn probing_extraction(
        mut self,
        probe: Arc<dyn ExtractionHealth>,
    ) -> Self {
        self.extraction = Some(probe);
        self
    }
}

impl std::fmt::Debug for LiveHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveHealth").finish_non_exhaustive()
    }
}

impl HealthProbe for LiveHealth {
    fn health(&self) -> HealthFuture<'_> {
        Box::pin(async move {
            let storage_ok = self.store.schema_version().await.is_ok();
            let (discord, discord_ok, dropped_events) = match &self.discord {
                None => ("disabled", true, None),
                Some(probe) => {
                    let (state, ready, dropped) = probe.gateway();
                    (state, ready, Some(dropped))
                }
            };
            let (scheduler, scheduler_ok, last_tick_age_seconds) = match &self.discord {
                None => ("disabled", true, None),
                Some(probe) => {
                    let (state, age) = probe.scheduler();
                    (state, state == "running", age)
                }
            };
            let extraction = match &self.extraction {
                Some(extraction) => Some(extraction.state(&self.store).await),
                None => None,
            };
            Health {
                status: if storage_ok && discord_ok && scheduler_ok {
                    "ok"
                } else {
                    "degraded"
                },
                mode: "live",
                scheduler,
                storage: if storage_ok { "ok" } else { "error" },
                discord,
                dropped_events,
                last_tick_age_seconds,
                chat: Some(self.chat.status()),
                extraction,
            }
        })
    }
}
