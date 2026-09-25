//! Live `/healthz`: the store answers a read. Discord and the delivery tick
//! are not wired yet, so both report `disabled`.

use std::sync::Arc;

use crate::{
    infrastructure::store::SqliteStore,
    runtime::application::{Health, HealthFuture, HealthProbe},
};

pub struct LiveHealth {
    store: Arc<SqliteStore>,
}

impl LiveHealth {
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self { store }
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
            Health {
                status: if storage_ok { "ok" } else { "degraded" },
                mode: "live",
                scheduler: "disabled",
                storage: if storage_ok { "ok" } else { "error" },
                discord: "disabled",
            }
        })
    }
}
