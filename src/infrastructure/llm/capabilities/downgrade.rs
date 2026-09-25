use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Mutex,
};

use super::ModelCapabilities;

/// An optional request capability a gateway can reject at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    StructuredOutput,
    SamplingControls,
    ReasoningControl,
}

/// Optional wire fields a 400 `error.param` may name, and the capability each needs.
const FIELDS: [(&str, Capability); 6] = [
    ("response_format", Capability::StructuredOutput),
    ("temperature", Capability::SamplingControls),
    ("seed", Capability::SamplingControls),
    ("top_p", Capability::SamplingControls),
    ("max_tokens", Capability::SamplingControls),
    ("reasoning_effort", Capability::ReasoningControl),
];

pub(crate) fn field_capability(field: &str) -> Option<(&'static str, Capability)> {
    FIELDS.iter().find(|(name, _)| *name == field).copied()
}

/// Capabilities a gateway disproved, per model alias, for the owner's lifetime.
#[derive(Default)]
pub(crate) struct DowngradeCache {
    revoked: Mutex<BTreeMap<String, BTreeSet<Capability>>>,
}

impl DowngradeCache {
    pub(crate) fn revoke(&self, alias: &str, capability: Capability) {
        self.lock()
            .entry(alias.to_owned())
            .or_default()
            .insert(capability);
    }

    pub(crate) fn apply(&self, alias: &str, capabilities: ModelCapabilities) -> ModelCapabilities {
        let revoked = self.lock().get(alias).cloned().unwrap_or_default();
        revoked
            .into_iter()
            .fold(capabilities, ModelCapabilities::without)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, BTreeSet<Capability>>> {
        // The map stays consistent under poisoning: every mutation is a single insert.
        self.revoked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl fmt::Debug for DowngradeCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DowngradeCache")
            .field("alias_count", &self.lock().len())
            .finish()
    }
}
