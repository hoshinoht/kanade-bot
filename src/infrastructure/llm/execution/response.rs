use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    ChatRequest, CompletionResponse, ErrorCode, FinishReason, LlmError, Message,
    schema::{self},
};
use super::{
    accounting::{encoded_size, response_metadata_text, response_payload_text, value_bounds},
    policy::ExecutionLimits,
};

pub(super) fn validate_response(
    request: &ChatRequest,
    response: CompletionResponse,
    limits: &ExecutionLimits,
) -> Result<CompletionResponse, LlmError> {
    if response.tool_calls.len() > limits.max_tools {
        return Err(LlmError::new(ErrorCode::InvalidOutput, "tool-count"));
    }
    if response.model != request.model {
        return Err(LlmError::new(ErrorCode::ModelMismatch, "model"));
    }
    if matches!(
        response.finish_reason,
        FinishReason::Stop if !response.tool_calls.is_empty()
    ) {
        return Err(LlmError::new(ErrorCode::InvalidOutput, "stop-tool-calls"));
    }
    if matches!(
        response.finish_reason,
        FinishReason::ToolCalls if response.tool_calls.is_empty()
    ) {
        return Err(LlmError::new(
            ErrorCode::InvalidOutput,
            "missing-tool-calls",
        ));
    }

    response_metadata_text(&response.model, limits)?;
    if let Some(content) = &response.content {
        response_payload_text(content, limits)?;
    }
    let tools: BTreeMap<_, _> = request
        .tools
        .iter()
        .map(|tool| (tool.name.as_str(), &tool.input_schema))
        .collect();
    let mut ids = historical_tool_ids(request);
    for call in &response.tool_calls {
        response_metadata_text(&call.id, limits)?;
        response_metadata_text(&call.name, limits)?;
        response_payload_text(&call.arguments, limits)?;
        if call.id.is_empty() || call.name.is_empty() || !ids.insert(call.id.as_str()) {
            return Err(LlmError::new(ErrorCode::InvalidOutput, "tool-id"));
        }
        if !tools.contains_key(call.name.as_str()) {
            return Err(LlmError::new(ErrorCode::InvalidOutput, "tool-name"));
        }
    }

    match &response.finish_reason {
        FinishReason::Stop | FinishReason::ToolCalls => {}
        FinishReason::Other(reason) => {
            response_metadata_text(reason, limits)?;
        }
        FinishReason::Length | FinishReason::ContentFilter => {}
    }
    encoded_size(
        &response,
        limits.max_response_bytes,
        ErrorCode::InvalidOutput,
        "aggregate",
    )?;
    match &response.finish_reason {
        FinishReason::Other(_) | FinishReason::Length | FinishReason::ContentFilter => {
            return Err(LlmError::new(ErrorCode::Incomplete, "finish"));
        }
        FinishReason::Stop | FinishReason::ToolCalls => {}
    }

    for call in &response.tool_calls {
        let schema = tools
            .get(call.name.as_str())
            .ok_or_else(|| LlmError::new(ErrorCode::InvalidOutput, "tool-name"))?;
        schema::parse_and_validate(
            schema,
            &call.arguments,
            &value_bounds(limits, limits.max_output_bytes),
            &value_bounds(limits, limits.max_schema_bytes),
        )?;
    }
    if let Some(output) = &request.output_schema {
        let content = response
            .content
            .as_deref()
            .ok_or_else(|| LlmError::new(ErrorCode::InvalidOutput, "missing-json"))?;
        schema::parse_and_validate(
            &output.schema,
            content,
            &value_bounds(limits, limits.max_output_bytes),
            &value_bounds(limits, limits.max_schema_bytes),
        )?;
    }
    Ok(response)
}

fn historical_tool_ids(request: &ChatRequest) -> BTreeSet<&str> {
    request
        .messages
        .iter()
        .flat_map(|message| match message {
            Message::Assistant { tool_calls, .. } => tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            Message::Tool { tool_call_id, .. } => vec![tool_call_id.as_str()],
            Message::System { .. } | Message::User { .. } => Vec::new(),
        })
        .collect()
}
