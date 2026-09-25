mod capabilities;
mod error;
mod execution;
#[cfg(any(test, feature = "test-support"))]
mod fake;
mod http;
mod provider;
mod request;
mod response;
mod schema;
mod shaping;
mod wire;

pub use capabilities::{
    Capability, Effort, ListedModel, ModelCapabilities, TrustZone, parse_models_list,
    resolve as resolve_capabilities,
};
pub use error::{ErrorCode, LlmError};
pub use execution::{CompletionRunner, ExecutionLimits, RetryPolicy};
#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeAction, FakeProvider};
pub use http::{
    BearerKey, HttpConfigError, HttpLimits, HttpProviderConfig, OpenAiCompatibleProvider,
    TrustRoots,
};
pub use provider::{
    CapabilityFuture, CompletionFuture, LlmProvider, ProviderFailure, ProviderFailureKind,
};
pub use request::{ChatRequest, Message, OutputSchema, Sampling, ToolCallRequest, ToolDefinition};
pub use response::{CompletionResponse, FinishReason, ToolCall, Usage};
pub use shaping::prepare as shape_request;
