mod capabilities;
mod error;
mod execution;
#[cfg(any(test, feature = "test-support"))]
mod fake;
pub mod governor;
mod http;
pub mod identity;
mod provider;
mod request;
mod response;
mod schema;
pub mod setup;
mod shaping;
mod wire;

pub use capabilities::{
    AdmissionLimits, Capability, Effort, ListedModel, ModelCapabilities, TrustZone, off_allowed,
    parse_models_list, reasoning_floor, resolve as resolve_capabilities,
};
pub use error::{ErrorCode, LlmError};
pub use execution::{
    CALL_TOKEN_BUDGET, CompletionRunner, ExecutionLimits, PROMPT_FLOOR_TOKENS, RESERVE_LIMIT,
    RetryPolicy,
};
#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeAction, FakeProvider};
pub use http::{
    BearerKey, HttpConfigError, HttpLimits, HttpProviderConfig, OpenAiCompatibleProvider,
    TrustRoots,
};
#[cfg(feature = "test-support")]
pub use http::{RequestHook, SentRequest, set_request_hook};
pub use provider::{
    CapabilityFuture, CompletionFuture, LlmProvider, ProviderFailure, ProviderFailureKind,
};
pub use request::{
    ChatRequest, Message, OutputSchema, OutputValidation, Sampling, ToolCallRequest,
    ToolCallValidation, ToolDefinition,
};
pub use response::{CompletionResponse, FinishReason, ToolCall, Usage};
pub use shaping::prepare as shape_request;
pub use shaping::schema_instruction;
#[cfg(any(test, feature = "test-support"))]
pub use shaping::wire_body;
