mod error;
mod execution;
#[cfg(any(test, feature = "test-support"))]
mod fake;
mod provider;
mod request;
mod response;
mod schema;

pub use error::{ErrorCode, LlmError};
pub use execution::{CompletionRunner, ExecutionLimits, RetryPolicy};
#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeAction, FakeProvider};
pub use provider::{CompletionFuture, LlmProvider, ProviderFailure, ProviderFailureKind};
pub use request::{ChatRequest, Message, OutputSchema, ToolCallRequest, ToolDefinition};
pub use response::{CompletionResponse, FinishReason, ToolCall, Usage};
