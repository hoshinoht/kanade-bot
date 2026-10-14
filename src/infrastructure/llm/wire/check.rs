//! Local checks mirroring Kanata's request validation, so a malformed value
//! fails here as a request error instead of a gateway 400 that would read as
//! "field unsupported" and downgrade the alias.

use serde_json::Value;

use super::super::{
    ChatRequest, ErrorCode, Message, ModelCapabilities, capabilities::MAX_OUTPUT_TOKENS,
    schema::encoded_size,
};

const MAX_NAME_BYTES: usize = 64;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 32;

/// Kanata refuses an empty tool result; this stands in for one.
pub(crate) const EMPTY_TOOL_RESULT: &str = "(no output)";

/// The reason the request cannot be sent as shaped for `capabilities`.
pub(crate) fn check(
    request: &ChatRequest,
    capabilities: &ModelCapabilities,
) -> Result<(), &'static str> {
    for message in &request.messages {
        match message {
            Message::System { content } | Message::User { content } if content.is_empty() => {
                return Err("empty-content");
            }
            Message::Assistant {
                content,
                tool_calls,
            } => {
                if tool_calls.is_empty() && content.as_deref().is_none_or(str::is_empty) {
                    return Err("empty-assistant");
                }
                if tool_calls
                    .iter()
                    .any(|call| call.id.is_empty() || !valid_name(&call.name))
                {
                    return Err("tool-call");
                }
            }
            Message::Tool { tool_call_id, .. } if tool_call_id.is_empty() => {
                return Err("tool-call-id");
            }
            _ => {}
        }
    }
    if capabilities.function_tools && request.tools.iter().any(|tool| !valid_name(&tool.name)) {
        return Err("tool-name");
    }
    if capabilities.structured_output
        && let Some(output) = &request.output_schema
        && (!valid_name(&output.name)
            || !output.schema.is_object()
            || depth(&output.schema) > MAX_SCHEMA_DEPTH
            || encoded_size(
                &output.schema,
                MAX_SCHEMA_BYTES,
                ErrorCode::RequestInvalid,
                "schema",
            )
            .is_err())
    {
        return Err("response-format");
    }
    if capabilities.sampling_controls {
        if !(1..=MAX_OUTPUT_TOKENS).contains(&request.max_output_tokens) {
            return Err("max-tokens");
        }
        if let Some(sampling) = &request.sampling {
            if sampling
                .temperature
                .is_some_and(|value| !(value.is_finite() && (0.0..=2.0).contains(&value)))
            {
                return Err("temperature");
            }
            if sampling
                .top_p
                .is_some_and(|value| !(value.is_finite() && value > 0.0 && value <= 1.0))
            {
                return Err("top-p");
            }
            if sampling
                .seed
                .is_some_and(|seed| i64::try_from(seed).is_err())
            {
                return Err("seed");
            }
        }
    }
    Ok(())
}

pub(crate) fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Kanata's container depth: scalars 0, each array or object adds one.
fn depth(value: &Value) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(value, 0usize)];
    while let Some((value, above)) = stack.pop() {
        let children: Box<dyn Iterator<Item = &Value>> = match value {
            Value::Array(items) => Box::new(items.iter()),
            Value::Object(map) => Box::new(map.values()),
            _ => continue,
        };
        let level = above + 1;
        deepest = deepest.max(level);
        if level > MAX_SCHEMA_DEPTH {
            break;
        }
        stack.extend(children.map(|child| (child, level)));
    }
    deepest
}
