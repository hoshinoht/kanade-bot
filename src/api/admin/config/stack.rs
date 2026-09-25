//! [`ModelCatalog`] over the running model stack: every read lists the
//! gateway once (bounded), which also refreshes the stack's trust zones.

use std::time::Duration;

use super::desk::{CatalogRead, ConfigFuture, ModelCatalog};
use crate::infrastructure::llm::setup::ModelStack;

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
}
