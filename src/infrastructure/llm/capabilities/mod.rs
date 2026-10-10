mod downgrade;
mod listing;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use downgrade::Capability;
pub(crate) use downgrade::{DowngradeCache, field_capability};
pub use listing::{ListedModel, parse_models_list};

/// Kanata's limit on published output tokens (`max_tokens`).
pub(crate) const MAX_OUTPUT_TOKENS: u32 = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// No reasoning; Kanata's wire name is `none`.
    #[serde(alias = "none")]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// The `reasoning_effort` value sent on the wire.
    pub(crate) fn wire_str(self) -> &'static str {
        match self {
            Self::Off => "none",
            other => other.as_str(),
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "off" | "none" => Some(Self::Off),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// Kanata's per-route admission settings, published on its private listener only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionLimits {
    pub max_in_flight: u32,
    pub max_queue: Option<u32>,
    pub queue_ms: Option<u64>,
    /// Shared by every route on the adapter; `None` caps nothing.
    pub adapter_max_in_flight: Option<u32>,
}

impl AdmissionLimits {
    /// Most concurrent requests the gateway admits for this alias without queueing.
    pub fn concurrency(&self) -> u32 {
        self.adapter_max_in_flight
            .map_or(self.max_in_flight, |adapter| {
                adapter.min(self.max_in_flight)
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustZone {
    Local,
    PrivateNetwork,
    External,
}

/// `off` needs no list (Kanata restricts nothing) or one naming `none`; without
/// reasoning control nothing is sent, so `off` is trivially fine (user decision
/// 2026-09-26: a list without `none` means the model requires reasoning).
pub fn off_allowed(reasoning_control: bool, efforts: Option<&[Effort]>) -> bool {
    !reasoning_control || efforts.is_none_or(|efforts| efforts.contains(&Effort::Off))
}

/// The lowest published level, used instead of `off` where `off` is not allowed.
pub fn reasoning_floor(reasoning_control: bool, efforts: Option<&[Effort]>) -> Option<Effort> {
    if off_allowed(reasoning_control, efforts) {
        return None;
    }
    efforts?.iter().copied().min()
}

/// Which optional request fields one model alias accepts.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Published operation names; informational, not enforced.
    pub operations: Vec<String>,
    pub structured_output: bool,
    pub sampling_controls: bool,
    pub reasoning_control: bool,
    pub function_tools: bool,
    pub streaming: bool,
    pub trust_zone: Option<TrustZone>,
    /// `None` lets the model decide which efforts it honours.
    pub reasoning_efforts: Option<Vec<Effort>>,
    /// Published context window; informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
    /// Published maximum completion size (`max_tokens`); absent when the
    /// gateway leaves the route unconstrained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Gateway admission limits for capacity checks; never sent or enforced here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<AdmissionLimits>,
}

impl ModelCapabilities {
    /// Absent or unreadable metadata: no response_format, sampling, reasoning or tools.
    pub fn minimal() -> Self {
        Self {
            operations: Vec::new(),
            structured_output: false,
            sampling_controls: false,
            reasoning_control: false,
            function_tools: false,
            streaming: false,
            trust_zone: None,
            reasoning_efforts: None,
            context_tokens: None,
            max_output_tokens: None,
            admission: None,
        }
    }

    /// See [`off_allowed`].
    pub fn off_allowed(&self) -> bool {
        off_allowed(self.reasoning_control, self.reasoning_efforts.as_deref())
    }

    /// See [`reasoning_floor`].
    pub fn reasoning_floor(&self) -> Option<Effort> {
        reasoning_floor(self.reasoning_control, self.reasoning_efforts.as_deref())
    }

    /// Ollama's cloud proxy reports `local`; the `-cloud` alias suffix is what marks it.
    pub fn is_cloud(&self, alias: &str) -> bool {
        self.trust_zone == Some(TrustZone::External) || alias.ends_with("-cloud")
    }

    /// These capabilities with `capability` turned off.
    pub fn without(mut self, capability: Capability) -> Self {
        match capability {
            Capability::StructuredOutput => self.structured_output = false,
            Capability::SamplingControls => self.sampling_controls = false,
            Capability::ReasoningControl => self.reasoning_control = false,
        }
        self
    }
}

impl fmt::Debug for ModelCapabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelCapabilities")
            .field("operation_count", &self.operations.len())
            .field("structured_output", &self.structured_output)
            .field("sampling_controls", &self.sampling_controls)
            .field("reasoning_control", &self.reasoning_control)
            .field("function_tools", &self.function_tools)
            .field("streaming", &self.streaming)
            .field("trust_zone", &self.trust_zone)
            .field("reasoning_efforts", &self.reasoning_efforts)
            .field("context_tokens", &self.context_tokens)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("admission", &self.admission)
            .finish()
    }
}

/// Precedence: gateway metadata, then operator-declared capabilities, then minimal.
pub fn resolve(
    published: Option<&ModelCapabilities>,
    declared: Option<&ModelCapabilities>,
) -> ModelCapabilities {
    published
        .or(declared)
        .cloned()
        .unwrap_or_else(ModelCapabilities::minimal)
}
