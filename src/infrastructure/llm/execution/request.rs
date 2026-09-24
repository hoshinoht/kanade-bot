use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    ChatRequest, ErrorCode, LlmError, Message,
    schema::{self},
};
use super::{
    accounting::{encoded_size, request_text, value_bounds},
    policy::ExecutionLimits,
};

pub(super) fn validate_request(
    request: &ChatRequest,
    limits: &ExecutionLimits,
) -> Result<usize, LlmError> {
    if request.model.is_empty()
        || request.messages.is_empty()
        || request.messages.len() > limits.max_messages
        || request.tools.len() > limits.max_tools
        || request.max_output_tokens == 0
    {
        return Err(LlmError::new(ErrorCode::RequestInvalid, "request-shape"));
    }
    request_text(&request.model, limits)?;

    let mut tools = BTreeMap::new();
    for tool in &request.tools {
        request_text(&tool.name, limits)?;
        if tool.name.is_empty()
            || tools
                .insert(tool.name.as_str(), &tool.input_schema)
                .is_some()
        {
            return Err(LlmError::new(ErrorCode::RequestInvalid, "tool-name"));
        }
        if let Some(description) = &tool.description {
            request_text(description, limits)?;
        }
        schema::preflight_schema(
            &tool.input_schema,
            &value_bounds(limits, limits.max_schema_bytes),
        )?;
        encoded_size(
            &tool.input_schema,
            limits.max_schema_bytes,
            ErrorCode::RequestInvalid,
            "schema",
        )?;
    }

    if let Some(output) = &request.output_schema {
        request_text(&output.name, limits)?;
        if output.name.is_empty() {
            return Err(LlmError::new(ErrorCode::RequestInvalid, "output-name"));
        }
        schema::preflight_schema(
            &output.schema,
            &value_bounds(limits, limits.max_schema_bytes),
        )?;
        encoded_size(
            &output.schema,
            limits.max_schema_bytes,
            ErrorCode::RequestInvalid,
            "schema",
        )?;
    }

    let mut pending = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut call_count = 0usize;
    for message in &request.messages {
        match message {
            Message::System { content } | Message::User { content } => {
                if !pending.is_empty() {
                    return Err(LlmError::new(ErrorCode::RequestInvalid, "unanswered-tool"));
                }
                request_text(content, limits)?;
            }
            Message::Assistant {
                content,
                tool_calls,
            } => {
                if !pending.is_empty() || (content.is_none() && tool_calls.is_empty()) {
                    return Err(LlmError::new(ErrorCode::RequestInvalid, "assistant-shape"));
                }
                if let Some(content) = content {
                    request_text(content, limits)?;
                }
                for call in tool_calls {
                    call_count = call_count
                        .checked_add(1)
                        .ok_or_else(|| LlmError::new(ErrorCode::RequestInvalid, "tool-count"))?;
                    if call_count > limits.max_tools {
                        return Err(LlmError::new(ErrorCode::RequestInvalid, "tool-count"));
                    }
                    call_request(
                        call.id.as_str(),
                        call.name.as_str(),
                        call.arguments.as_str(),
                        &tools,
                        limits,
                        &mut TranscriptState {
                            pending: &mut pending,
                            seen: &mut seen,
                        },
                    )?;
                }
            }
            Message::Tool {
                tool_call_id,
                content,
            } => {
                request_text(tool_call_id, limits)?;
                request_text(content, limits)?;
                if !pending.remove(tool_call_id.as_str()) {
                    return Err(LlmError::new(ErrorCode::RequestInvalid, "tool-result"));
                }
            }
        }
    }
    if !pending.is_empty() {
        return Err(LlmError::new(ErrorCode::RequestInvalid, "unanswered-tool"));
    }
    let request_bytes = encoded_size(
        request,
        limits.max_request_bytes,
        ErrorCode::RequestInvalid,
        "aggregate",
    )?;
    for tool in &request.tools {
        schema::validate_schema(
            &tool.input_schema,
            &value_bounds(limits, limits.max_schema_bytes),
        )?;
    }
    if let Some(output) = &request.output_schema {
        schema::validate_schema(
            &output.schema,
            &value_bounds(limits, limits.max_schema_bytes),
        )?;
    }
    validate_tool_arguments(request, &tools, limits)?;
    Ok(request_bytes)
}

struct TranscriptState<'state, 'id> {
    pending: &'state mut BTreeSet<&'id str>,
    seen: &'state mut BTreeSet<&'id str>,
}

fn call_request<'id>(
    id: &'id str,
    name: &str,
    args: &str,
    tools: &BTreeMap<&'id str, &'id serde_json::Value>,
    limits: &ExecutionLimits,
    state: &mut TranscriptState<'_, 'id>,
) -> Result<(), LlmError> {
    request_text(id, limits)?;
    request_text(name, limits)?;
    request_text(args, limits)?;
    tools
        .get(name)
        .ok_or_else(|| LlmError::new(ErrorCode::RequestInvalid, "tool-name"))?;
    if id.is_empty() || !state.pending.insert(id) || !state.seen.insert(id) {
        return Err(LlmError::new(ErrorCode::RequestInvalid, "tool-id"));
    }
    schema::inspect_json(
        args,
        &value_bounds(limits, limits.max_output_bytes),
        ErrorCode::RequestInvalid,
        "tool-arguments",
    )
}

fn validate_tool_arguments(
    request: &ChatRequest,
    tools: &BTreeMap<&str, &serde_json::Value>,
    limits: &ExecutionLimits,
) -> Result<(), LlmError> {
    for message in &request.messages {
        let Message::Assistant { tool_calls, .. } = message else {
            continue;
        };
        for call in tool_calls {
            let schema = tools
                .get(call.name.as_str())
                .ok_or_else(|| LlmError::new(ErrorCode::RequestInvalid, "tool-name"))?;
            schema::parse_and_validate(
                schema,
                &call.arguments,
                &value_bounds(limits, limits.max_output_bytes),
                &value_bounds(limits, limits.max_schema_bytes),
            )
            .map_err(|_| LlmError::new(ErrorCode::RequestInvalid, "tool-arguments"))?;
        }
    }
    Ok(())
}
