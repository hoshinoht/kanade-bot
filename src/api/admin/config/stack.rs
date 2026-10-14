//! [`ModelCatalog`] over the running model stack: every read lists the
//! gateway once (bounded), which also refreshes the stack's trust zones;
//! saved roles switch the running stack.

use std::time::Duration;

use std::collections::BTreeMap;

use super::desk::{CatalogRead, ConfigFuture, ModelCatalog};
use crate::{
    domain::settings::Models,
    infrastructure::llm::{
        governor::Role,
        setup::{ModelRoles, ModelStack, RoleSwap, RunningRole},
    },
};

const LISTING_TIMEOUT: Duration = Duration::from_secs(5);

impl ModelCatalog for ModelStack {
    fn read(&self) -> ConfigFuture<'_, CatalogRead> {
        Box::pin(async move {
            let listed = tokio::time::timeout(LISTING_TIMEOUT, self.provider.list_models()).await;
            CatalogRead {
                reachable: matches!(listed, Ok(Ok(_))),
                snapshot: self.catalog(),
            }
        })
    }

    fn apply(&self, models: &Models) -> Result<Vec<RoleSwap>, String> {
        self.apply_roles(ModelRoles::from(models))
            .map_err(|error| error.to_string())
    }

    fn running(&self) -> BTreeMap<Role, RunningRole> {
        let waiting = self.awaiting_restart();
        let mut running = ModelStack::running(self);
        running.retain(|role, _| !waiting.contains(role));
        running
    }

    /// Serve composes extraction and the heading rewriter at startup, only
    /// for roles that had a model then (chat reads its route per question).
    fn awaiting_restart(&self) -> Vec<Role> {
        [Role::Extraction, Role::Rewrite]
            .into_iter()
            .filter(|&role| self.has_role(role) && !self.routed_at_start(role))
            .collect()
    }
}
