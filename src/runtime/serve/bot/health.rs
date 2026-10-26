//! What the Discord side shares with `/healthz`: the gateway's connection
//! and dropped events, the tick's state and extraction's.

use std::{future::Future, pin::Pin, sync::Arc};

use crate::{
    bot::{
        events::DroppedEvents,
        gateway::{Connection, ConnectionStatus},
    },
    runtime::{
        application::DroppedHealth,
        serve::{
            bot::{extract::ExtractionStatus, tick::TickStatus},
            health::{DiscordHealth, ExtractionHealth, LiveHealth},
        },
    },
};
use kanade_store::SqliteStore;

/// What the gateway shares with health.
#[derive(Clone, Debug)]
pub struct GatewayProbe {
    pub connection: ConnectionStatus,
    pub dropped: DroppedEvents,
}

struct DiscordProbe {
    gateway: GatewayProbe,
    tick: Arc<TickStatus>,
}

impl DiscordHealth for DiscordProbe {
    fn gateway(&self) -> (&'static str, bool, DroppedHealth) {
        let connection = self.gateway.connection.get();
        let counts = self.gateway.dropped.counts();
        (
            connection.as_str(),
            connection == Connection::Ready,
            DroppedHealth {
                other_guild: counts.other_guild,
                no_guild: counts.no_guild,
            },
        )
    }

    fn scheduler(&self) -> (&'static str, Option<u64>) {
        (self.tick.state(), self.tick.last_age_seconds())
    }
}

impl ExtractionHealth for ExtractionStatus {
    fn state<'a>(
        &'a self,
        store: &'a SqliteStore,
    ) -> Pin<Box<dyn Future<Output = &'static str> + Send + 'a>> {
        Box::pin(ExtractionStatus::state(self, store))
    }
}

impl LiveHealth {
    pub fn with_discord(self, gateway: GatewayProbe, tick: Arc<TickStatus>) -> Self {
        self.probing_discord(Arc::new(DiscordProbe { gateway, tick }))
    }

    pub fn with_extraction(self, extraction: Arc<ExtractionStatus>) -> Self {
        self.probing_extraction(extraction)
    }
}
