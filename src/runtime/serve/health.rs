//! Live `/healthz`: the store answers a read, the gateway is ready and the
//! delivery tick keeps completing; `extraction` is informational and never
//! degrades the status. With `KANADE_DISCORD_GATEWAY=0` Discord
//! and the scheduler report `disabled` and only storage decides.

use std::sync::Arc;

use super::{extract::ExtractionStatus, tick::TickStatus};
use crate::{
    bot::{
        events::DroppedEvents,
        gateway::{Connection, ConnectionStatus},
    },
    chat::driver::ChatHandle,
    infrastructure::store::SqliteStore,
    runtime::application::{DroppedHealth, Health, HealthFuture, HealthProbe},
};

/// What the gateway shares with health.
#[derive(Clone, Debug)]
pub struct GatewayProbe {
    pub connection: ConnectionStatus,
    pub dropped: DroppedEvents,
}

pub struct LiveHealth {
    store: Arc<SqliteStore>,
    gateway: Option<GatewayProbe>,
    tick: Option<Arc<TickStatus>>,
    chat: Arc<ChatHandle>,
    extraction: Option<Arc<ExtractionStatus>>,
}

impl LiveHealth {
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self {
            store,
            gateway: None,
            tick: None,
            chat: Arc::default(),
            extraction: None,
        }
    }

    /// Filled once serve starts chat; shared with the API and `/limits`.
    pub fn chat(&self) -> Arc<ChatHandle> {
        Arc::clone(&self.chat)
    }

    pub fn with_discord(mut self, gateway: GatewayProbe, tick: Arc<TickStatus>) -> Self {
        self.gateway = Some(gateway);
        self.tick = Some(tick);
        self
    }

    pub fn with_extraction(mut self, extraction: Arc<ExtractionStatus>) -> Self {
        self.extraction = Some(extraction);
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
            let (discord, discord_ok, dropped_events) = match &self.gateway {
                None => ("disabled", true, None),
                Some(gateway) => {
                    let connection = gateway.connection.get();
                    let counts = gateway.dropped.counts();
                    (
                        connection.as_str(),
                        connection == Connection::Ready,
                        Some(DroppedHealth {
                            other_guild: counts.other_guild,
                            no_guild: counts.no_guild,
                        }),
                    )
                }
            };
            let (scheduler, scheduler_ok, last_tick_age_seconds) = match &self.tick {
                None => ("disabled", true, None),
                Some(tick) => {
                    let state = tick.state();
                    (state, state == "running", tick.last_age_seconds())
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
