use std::time::Duration;

use super::super::{ErrorCode, LlmError};

const MAX_MESSAGES: usize = 256;
const MAX_BYTES: usize = 1_048_576;
const MAX_TOOLS: usize = 64;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 100_000;
const MAX_TOKENS: u32 = 1_000_000;
const MAX_ATTEMPTS: u8 = 5;
const MAX_DEADLINE: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct ExecutionLimits {
    pub max_messages: usize,
    pub max_content_bytes: usize,
    pub max_tools: usize,
    pub max_schema_bytes: usize,
    pub max_depth: usize,
    pub max_output_bytes: usize,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_value_nodes: usize,
    pub token_budget: u32,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_messages: 64,
            max_content_bytes: 64 * 1024,
            max_tools: 32,
            max_schema_bytes: 64 * 1024,
            max_depth: 32,
            max_output_bytes: 256 * 1024,
            max_request_bytes: 512 * 1024,
            max_response_bytes: 512 * 1024,
            max_value_nodes: 20_000,
            token_budget: 16_384,
        }
    }
}

impl ExecutionLimits {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.max_messages == 0
            || self.max_messages > MAX_MESSAGES
            || self.max_content_bytes == 0
            || self.max_content_bytes > MAX_BYTES
            || self.max_tools == 0
            || self.max_tools > MAX_TOOLS
            || self.max_schema_bytes == 0
            || self.max_schema_bytes > MAX_BYTES
            || self.max_depth == 0
            || self.max_depth > MAX_DEPTH
            || self.max_output_bytes == 0
            || self.max_output_bytes > MAX_BYTES
            || self.max_request_bytes == 0
            || self.max_request_bytes > MAX_BYTES
            || self.max_response_bytes == 0
            || self.max_response_bytes > MAX_BYTES
            || self.max_value_nodes == 0
            || self.max_value_nodes > MAX_NODES
            || self.token_budget == 0
            || self.token_budget > MAX_TOKENS
        {
            Err(LlmError::new(ErrorCode::RequestInvalid, "invalid-limits"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    pub total_deadline: Duration,
    pub max_attempts: u8,
    pub backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            total_deadline: Duration::from_secs(30),
            max_attempts: 3,
            backoff: Duration::from_millis(100),
        }
    }
}

impl RetryPolicy {
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.total_deadline.is_zero()
            || self.total_deadline > MAX_DEADLINE
            || self.max_attempts == 0
            || self.max_attempts > MAX_ATTEMPTS
            || self.backoff.is_zero()
            || self.backoff > MAX_BACKOFF
        {
            Err(LlmError::new(
                ErrorCode::RequestInvalid,
                "invalid-retry-policy",
            ))
        } else {
            Ok(())
        }
    }
}
