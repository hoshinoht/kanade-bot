mod downgrade;
mod listing;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use downgrade::Capability;
pub(crate) use downgrade::{DowngradeCache, field_capability};
pub use listing::{ListedModel, parse_models_list};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Off,
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustZone {
    Local,
    PrivateNetwork,
    External,
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
        }
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
