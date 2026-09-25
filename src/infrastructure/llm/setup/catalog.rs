use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use serde::Serialize;

use super::super::{
    AdmissionLimits, Effort, ListedModel, ModelCapabilities, TrustZone,
    governor::{Governor, Role},
    off_allowed, reasoning_floor,
};

/// Read-only view of the gateway's `GET /v1/models` for the config API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CatalogSnapshot {
    /// False until a listing has succeeded.
    pub listed: bool,
    pub models: Vec<CatalogModel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CatalogModel {
    pub alias: String,
    /// The entry carries a `kanata` metadata object.
    pub published: bool,
    pub trust_zone: Option<TrustZone>,
    pub leaves_homelab: bool,
    pub reasoning_control: bool,
    /// `None`: every level is accepted.
    pub reasoning_efforts: Option<Vec<Effort>>,
    pub structured_output: bool,
    pub sampling_controls: bool,
    pub function_tools: bool,
    pub context_tokens: Option<u32>,
    pub admission: Option<AdmissionLimits>,
}

impl CatalogModel {
    /// Whether a role on this alias may send no reasoning (`off`).
    pub fn off_allowed(&self) -> bool {
        off_allowed(self.reasoning_control, self.reasoning_efforts.as_deref())
    }

    /// The level used instead of `off` where `off` is not allowed.
    pub fn reasoning_floor(&self) -> Option<Effort> {
        reasoning_floor(self.reasoning_control, self.reasoning_efforts.as_deref())
    }
}

/// Fails closed (`ModelInfo.leaves_homelab`): only a published `local` or
/// `private_network` zone stays home, and a `-cloud` alias never does because
/// Ollama's cloud proxy reports `local`.
pub fn leaves_homelab(alias: &str, capabilities: Option<&ModelCapabilities>) -> bool {
    let home = matches!(
        capabilities.and_then(|caps| caps.trust_zone),
        Some(TrustZone::Local | TrustZone::PrivateNetwork)
    );
    !home || alias.ends_with("-cloud")
}

pub(super) fn published<'a>(
    listed: &'a [ListedModel],
    alias: &str,
) -> Option<&'a ModelCapabilities> {
    listed
        .iter()
        .find(|model| model.id == alias)
        .and_then(|model| model.capabilities.as_ref())
}

#[derive(Debug, Default)]
pub(super) struct CatalogState {
    listing: RwLock<Option<Arc<[ListedModel]>>>,
}

impl CatalogState {
    /// Listing observer: keeps the snapshot and re-derives each route's
    /// `external` flag. A failed refresh keeps the last derivation.
    pub fn apply(
        &self,
        listed: &[ListedModel],
        governor: &Governor,
        aliases: &BTreeMap<Role, String>,
    ) {
        *self.listing.write().unwrap_or_else(|p| p.into_inner()) = Some(listed.into());
        for (&role, alias) in aliases {
            governor.set_external(role, leaves_homelab(alias, published(listed, alias)));
        }
    }

    pub fn listing(&self) -> Option<Arc<[ListedModel]>> {
        self.listing
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn snapshot(&self) -> CatalogSnapshot {
        let Some(listing) = self.listing() else {
            return CatalogSnapshot::default();
        };
        let models = listing
            .iter()
            .map(|model| {
                let caps = model.capabilities.as_ref();
                let minimal = ModelCapabilities::minimal();
                let shown = caps.unwrap_or(&minimal);
                CatalogModel {
                    alias: model.id.clone(),
                    published: caps.is_some(),
                    trust_zone: shown.trust_zone,
                    leaves_homelab: leaves_homelab(&model.id, caps),
                    reasoning_control: shown.reasoning_control,
                    reasoning_efforts: shown.reasoning_efforts.clone(),
                    structured_output: shown.structured_output,
                    sampling_controls: shown.sampling_controls,
                    function_tools: shown.function_tools,
                    context_tokens: shown.context_tokens,
                    admission: shown.admission,
                }
            })
            .collect();
        CatalogSnapshot {
            listed: true,
            models,
        }
    }
}
