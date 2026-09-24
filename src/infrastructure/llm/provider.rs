use std::{future::Future, pin::Pin};

use super::{ChatRequest, CompletionResponse};

pub type CompletionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompletionResponse, ProviderFailure>> + Send + 'a>>;

pub trait LlmProvider: Send + Sync {
    fn complete(&self, request: &ChatRequest) -> CompletionFuture<'_>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderFailureKind {
    Transient,
    Permanent,
    Authentication,
    InvalidOutput,
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
