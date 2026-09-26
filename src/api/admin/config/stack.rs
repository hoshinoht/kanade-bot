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
        ModelStack::running(self)
    }
}
