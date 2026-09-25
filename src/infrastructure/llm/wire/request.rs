use serde_json::{Map, Value, json};

use super::super::{ChatRequest, Effort, Message, ModelCapabilities, ToolCallRequest};

/// The OpenAI chat-completions body `capabilities` allow; unsupported optional
/// fields are omitted, never sent as null.
pub(crate) fn chat_body(request: &ChatRequest, capabilities: &ModelCapabilities) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(request.model));
    body.insert(
        "messages".into(),
        Value::Array(request.messages.iter().map(message).collect()),
    );
    if capabilities.function_tools && !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                let mut function = Map::new();
                function.insert("name".into(), json!(tool.name));
                if let Some(description) = &tool.description {
                    function.insert("description".into(), json!(description));
                }
                function.insert("parameters".into(), tool.input_schema.clone());
                json!({"type": "function", "function": function})
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    if capabilities.structured_output
        && let Some(output) = &request.output_schema
    {
        body.insert(
            "response_format".into(),
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": output.name,
                    "schema": output.schema,
                    "strict": output.strict,
                }
            }),
        );
    }
    if capabilities.sampling_controls {
        body.insert("max_tokens".into(), json!(request.max_output_tokens));
        if let Some(sampling) = &request.sampling {
            if let Some(temperature) = sampling.temperature {
                body.insert("temperature".into(), json!(temperature));
            }
            if let Some(seed) = sampling.seed {
                body.insert("seed".into(), json!(seed));
            }
            if let Some(top_p) = sampling.top_p {
                body.insert("top_p".into(), json!(top_p));
            }
        }
    }
    if capabilities.reasoning_control
        && let Some(effort) = request.reasoning
        && (effort != Effort::Off || publishes(capabilities, Effort::Off))
    {
        body.insert("reasoning_effort".into(), json!(effort.wire_str()));
    }
    Value::Object(body)
}

/// `Off` is sent as `none` only where the alias publishes it; elsewhere omitting
/// the field is the only portable "no preference".
fn publishes(capabilities: &ModelCapabilities, effort: Effort) -> bool {
    capabilities
        .reasoning_efforts
        .as_ref()
        .is_some_and(|efforts| efforts.contains(&effort))
}

fn message(message: &Message) -> Value {
    match message {
        Message::System { content } => json!({"role": "system", "content": content}),
        Message::User { content } => json!({"role": "user", "content": content}),
        Message::Assistant {
            content,
            tool_calls,
        } => {
            let mut out = Map::new();
            out.insert("role".into(), json!("assistant"));
            if let Some(content) = content {
                out.insert("content".into(), json!(content));
            }
            if !tool_calls.is_empty() {
                out.insert(
                    "tool_calls".into(),
                    Value::Array(tool_calls.iter().map(tool_call).collect()),
                );
            }
            Value::Object(out)
        }
        Message::Tool {
            tool_call_id,
            content,
        } => {
            let content = if content.is_empty() {
                super::EMPTY_TOOL_RESULT
            } else {
                content
            };
            json!({"role": "tool", "tool_call_id": tool_call_id, "content": content})
        }
    }
}

fn tool_call(call: &ToolCallRequest) -> Value {
    json!({
        "id": call.id,
        "type": "function",
        "function": {"name": call.name, "arguments": call.arguments},
    })
}
