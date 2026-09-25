use std::{future::Future, pin::Pin};

use tokio::time::Instant;

use super::{Capability, ChatRequest, CompletionResponse, ModelCapabilities};

pub type CompletionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompletionResponse, ProviderFailure>> + Send + 'a>>;
pub type CapabilityFuture<'a> =
    Pin<Box<dyn Future<Output = Option<ModelCapabilities>> + Send + 'a>>;

pub trait LlmProvider: Send + Sync {
    fn complete(&self, request: &ChatRequest) -> CompletionFuture<'_>;

    /// Effective capabilities for `model`, resolved once per runner call and
    /// bounded by `deadline`; `None` sends requests unshaped.
    fn capabilities<'a>(&'a self, _model: &'a str, _deadline: Instant) -> CapabilityFuture<'a> {
        Box::pin(async { None })
    }

    /// Send `request`, already shaped for `capabilities`, with a body built from
    /// exactly those capabilities.
    fn complete_with(
        &self,
        request: &ChatRequest,
        _capabilities: &ModelCapabilities,
    ) -> CompletionFuture<'_> {
        self.complete(request)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderFailureKind {
    Transient,
    Permanent,
    Authentication,
    InvalidOutput,
    /// The gateway refused a field of this capability; the provider remembers the
    /// downgrade and the runner reshapes and retries once.
    CapabilityRejected(Capability),
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub kind: ProviderFailureKind,
    pub reason_code: &'static str,
}

impl std::fmt::Debug for ProviderFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderFailure")
            .field("kind", &self.kind)
            .field("has_reason_code", &true)
            .finish()
    }
}
