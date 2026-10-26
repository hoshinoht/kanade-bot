use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use serde::Serialize;

use super::super::{
    AdmissionLimits, Effort, ListedModel, ModelCapabilities, TrustZone, governor::Governor,
    off_allowed, reasoning_floor,
};

/// Read-only view of the gateway's `GET /v1/models` plus the operator's
/// declared models it does not list, for the config API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CatalogSnapshot {
    /// False until a listing has succeeded.
    pub listed: bool,
    pub models: Vec<CatalogModel>,
}

/// Capabilities shown are the published metadata, else the operator's
/// declaration, else the minimal profile (the provider's precedence).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CatalogModel {
    pub alias: String,
    /// The entry carries a `kanata` metadata object.
    pub published: bool,
    /// The gateway's listing names the alias; false for a declared model the
    /// listing omits, or when the gateway serves no listing.
    pub listed: bool,
    pub trust_zone: Option<TrustZone>,
    pub leaves_homelab: bool,
    pub reasoning_control: bool,
    /// `None`: every level is accepted.
    pub reasoning_efforts: Option<Vec<Effort>>,
    pub structured_output: bool,
    pub sampling_controls: bool,
    pub function_tools: bool,
    pub context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
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

/// A `<base>:<level>` alias: Kanata's listing of `base` with reasoning baked in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub base: String,
    pub effort: Effort,
}

/// Only a known effort suffix (`none` … `max`) on a base that is itself listed
/// makes a variant; Ollama tags such as `gpt-oss:120b-cloud` never do.
pub fn variant_of(alias: &str, listed: impl Fn(&str) -> bool) -> Option<Variant> {
    let (base, level) = alias.rsplit_once(':')?;
    let effort = match level {
        "none" => Effort::Off,
        "off" => return None,
        other => Effort::parse(other)?,
    };
    (!base.is_empty() && listed(base)).then(|| Variant {
        base: base.to_owned(),
        effort,
    })
}

impl CatalogSnapshot {
    /// Variants are a listing feature: only a listed base makes one.
    pub fn variant(&self, alias: &str) -> Option<Variant> {
        variant_of(alias, |base| {
            self.models.iter().any(|m| m.listed && m.alias == base)
        })
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

/// Routes with no listing stay as they are (external until one confirms).
pub(super) fn rederive(listed: &[ListedModel], governor: &Governor) {
    governor.rederive_external(|alias| leaves_homelab(alias, published(listed, alias)));
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
    /// Operator declarations; never consulted for trust zones or admission.
    declared: BTreeMap<String, ModelCapabilities>,
}

impl CatalogState {
    pub fn new(declared: BTreeMap<String, ModelCapabilities>) -> Self {
        Self {
            listing: RwLock::default(),
            declared,
        }
    }

    /// Published metadata in `listed`, else the operator's declaration.
    pub fn capabilities<'a>(
        &'a self,
        listed: Option<&'a [ListedModel]>,
        alias: &str,
    ) -> Option<&'a ModelCapabilities> {
        listed
            .and_then(|listed| published(listed, alias))
            .or_else(|| self.declared.get(alias))
    }

    /// Listing observer: keeps the snapshot and re-derives each route's
    /// `external` flag from its current alias. A failed refresh keeps the
    /// last derivation.
    pub fn apply(&self, listed: &[ListedModel], governor: &Governor) {
        *self.listing.write().unwrap_or_else(|p| p.into_inner()) = Some(listed.into());
        rederive(listed, governor);
    }

    pub fn listing(&self) -> Option<Arc<[ListedModel]>> {
        self.listing
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The listing's models, then every declared model it does not name (all
    /// of them before a listing or without one) as unpublished and external,
    /// so a declared window and output maximum apply either way.
    pub fn snapshot(&self) -> CatalogSnapshot {
        let listing = self.listing();
        let listed = listing.as_deref().unwrap_or_default();
        let entry = |alias: &str, caps: Option<&ModelCapabilities>, listed: bool| {
            let minimal = ModelCapabilities::minimal();
            let shown = caps
                .or_else(|| self.declared.get(alias))
                .unwrap_or(&minimal);
            CatalogModel {
                alias: alias.to_owned(),
                published: caps.is_some(),
                listed,
                trust_zone: shown.trust_zone,
                leaves_homelab: leaves_homelab(alias, caps),
                reasoning_control: shown.reasoning_control,
                reasoning_efforts: shown.reasoning_efforts.clone(),
                structured_output: shown.structured_output,
                sampling_controls: shown.sampling_controls,
                function_tools: shown.function_tools,
                context_tokens: shown.context_tokens,
                max_output_tokens: shown.max_output_tokens,
                admission: shown.admission,
            }
        };
        let mut models: Vec<CatalogModel> = listed
            .iter()
            .map(|model| entry(&model.id, model.capabilities.as_ref(), true))
            .collect();
        models.extend(
            self.declared
                .keys()
                .filter(|alias| !listed.iter().any(|model| &model.id == *alias))
                .map(|alias| entry(alias, None, false)),
        );
        CatalogSnapshot {
            listed: listing.is_some(),
            models,
        }
    }
}
