use serde::Serialize;

use super::super::{
    ErrorCode, LlmError,
    schema::{self, ValueBounds},
};
use super::policy::ExecutionLimits;

pub(super) fn value_bounds(limits: &ExecutionLimits, bytes: usize) -> ValueBounds {
    ValueBounds {
        max_bytes: bytes,
        max_depth: limits.max_depth,
        max_nodes: limits.max_value_nodes,
    }
}

pub(super) fn request_text(value: &str, limits: &ExecutionLimits) -> Result<(), LlmError> {
    text(value, limits.max_content_bytes, ErrorCode::RequestInvalid)
}

pub(super) fn response_metadata_text(
    value: &str,
    limits: &ExecutionLimits,
) -> Result<(), LlmError> {
    text(value, limits.max_content_bytes, ErrorCode::InvalidOutput)
}

pub(super) fn response_payload_text(value: &str, limits: &ExecutionLimits) -> Result<(), LlmError> {
    text(value, limits.max_output_bytes, ErrorCode::InvalidOutput)
}

pub(super) fn encoded_size<T: Serialize + ?Sized>(
    value: &T,
    max_bytes: usize,
    code: ErrorCode,
    reason: &str,
) -> Result<usize, LlmError> {
    schema::encoded_size(value, max_bytes, code, reason)
}

pub(super) fn estimate(bytes: usize, max_output_tokens: u32) -> Result<u32, LlmError> {
    let prompt = u32::try_from(bytes.div_ceil(4))
        .map_err(|_| LlmError::new(ErrorCode::BudgetExceeded, "estimate"))?;
    prompt
        .checked_add(max_output_tokens)
        .ok_or_else(|| LlmError::new(ErrorCode::BudgetExceeded, "reservation-overflow"))
}

fn text(value: &str, max_content_bytes: usize, code: ErrorCode) -> Result<(), LlmError> {
    if value.len() > max_content_bytes {
        return Err(LlmError::new(code, "string-size"));
    }
    Ok(())
}
