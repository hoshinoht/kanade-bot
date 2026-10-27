use serde_json::{Map, Value, json};

use super::super::{ChatRequest, Effort, Message, ModelCapabilities, OutputLimit, ToolCallRequest};

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
    if let Some(max_tokens) = sent_max_tokens(request, capabilities) {
        body.insert(size_field(capabilities).into(), json!(max_tokens));
    }
    if capabilities.sampling_controls
        && let Some(sampling) = &request.sampling
    {
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
    if let Some(effort) = sent_effort(request, capabilities) {
        body.insert("reasoning_effort".into(), json!(effort.wire_str()));
    }
    if capabilities.prompt_cache_key
        && let Some(key) = &request.prompt_cache_key
    {
        body.insert("prompt_cache_key".into(), json!(key));
    }
    Value::Object(body)
}

/// The size (under [`size_field`]) the body carries, if any: with a declared
/// `output_limit` always (none once omitted), else only with sampling
/// controls. The runner records exactly this for the logs.
pub(crate) fn sent_max_tokens(
    request: &ChatRequest,
    capabilities: &ModelCapabilities,
) -> Option<u32> {
    match capabilities.output_limit {
        None => capabilities
            .sampling_controls
            .then_some(request.max_output_tokens),
        Some(OutputLimit::Omitted) => None,
        Some(_) => Some(request.max_output_tokens),
    }
}

/// `max_tokens` unless the operator declared `max_completion_tokens`.
fn size_field(capabilities: &ModelCapabilities) -> &'static str {
    capabilities
        .output_limit
        .and_then(OutputLimit::field)
        .unwrap_or("max_tokens")
}

/// The `reasoning_effort` the body carries, if any: only with reasoning
/// control, and `off` only where the published list allows `none`. The
/// runner records exactly this for the logs.
pub(crate) fn sent_effort(
    request: &ChatRequest,
    capabilities: &ModelCapabilities,
) -> Option<Effort> {
    let effort = request.reasoning?;
    (capabilities.reasoning_control
        && (effort != Effort::Off || publishes(capabilities, Effort::Off)))
    .then_some(effort)
}

/// Kanata publishes no list when a provider passes every level through, so `None`
/// allows `none`; a published list must name it.
fn publishes(capabilities: &ModelCapabilities, effort: Effort) -> bool {
    capabilities
        .reasoning_efforts
        .as_ref()
        .is_none_or(|efforts| efforts.contains(&effort))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(reasoning: Option<Effort>) -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            messages: vec![Message::User {
                content: "hi".into(),
            }],
            tools: Vec::new(),
            output_schema: None,
            max_output_tokens: 16,
            reasoning,
            sampling: None,
            prompt_cache_key: None,
        }
    }

    fn caps(control: bool, efforts: Option<Vec<Effort>>) -> ModelCapabilities {
        let mut caps = ModelCapabilities::minimal();
        caps.reasoning_control = control;
        caps.reasoning_efforts = efforts;
        caps
    }

    /// What the logs record is exactly what the body carries.
    #[test]
    fn the_sent_effort_matches_the_body() {
        let cases = [
            (Some(Effort::Off), caps(true, Some(Vec::new())), None),
            (Some(Effort::Off), caps(true, Some(vec![Effort::Low])), None),
            (Some(Effort::Off), caps(true, None), Some(Effort::Off)),
            (Some(Effort::Low), caps(false, None), None),
            (
                Some(Effort::Low),
                caps(true, Some(vec![Effort::Low])),
                Some(Effort::Low),
            ),
            (None, caps(true, None), None),
        ];
        for (effort, capabilities, sent) in cases {
            let request = request(effort);
            assert_eq!(sent_effort(&request, &capabilities), sent, "{effort:?}");
            let body = chat_body(&request, &capabilities);
            assert_eq!(
                body.get("reasoning_effort").cloned(),
                sent.map(|effort| json!(effort.wire_str())),
                "{effort:?}"
            );
        }
    }

    /// The size the logs record is exactly the one the body carries, under
    /// the declared name; no declaration keeps `max_tokens` with sampling.
    #[test]
    fn the_sent_size_follows_the_declared_output_limit() {
        let cases = [
            (false, None, None),
            (true, None, Some("max_tokens")),
            (false, Some(OutputLimit::MaxTokens), Some("max_tokens")),
            (
                false,
                Some(OutputLimit::MaxCompletionTokens),
                Some("max_completion_tokens"),
            ),
            (
                true,
                Some(OutputLimit::MaxCompletionTokens),
                Some("max_completion_tokens"),
            ),
            (true, Some(OutputLimit::Omitted), None),
        ];
        for (sampling, limit, field) in cases {
            let mut capabilities = ModelCapabilities::minimal();
            capabilities.sampling_controls = sampling;
            capabilities.output_limit = limit;
            let request = request(None);
            let sent = sent_max_tokens(&request, &capabilities);
            assert_eq!(sent, field.map(|_| 16), "{sampling} {limit:?}");
            let body = chat_body(&request, &capabilities);
            for name in ["max_tokens", "max_completion_tokens"] {
                assert_eq!(
                    body.get(name).cloned(),
                    (field == Some(name)).then(|| json!(16)),
                    "{sampling} {limit:?} {name}"
                );
            }
        }
    }

    /// The key rides every body whose capabilities accept it (Kanata
    /// metadata), verbatim; a declared or minimal profile never gets it.
    #[test]
    fn the_prompt_cache_key_goes_only_where_it_is_accepted() {
        let listed = crate::parse_models_list(
            br#"{"data": [{"id": "m", "kanata": {"trust_zone": "external"}}]}"#,
        )
        .expect("listing")
        .remove(0)
        .capabilities
        .expect("kanata metadata");
        assert!(listed.prompt_cache_key);
        for role in ["chat", "extraction", "rewrite"] {
            let key = format!("kanade-{role}-37f96542b663");
            let request = ChatRequest {
                prompt_cache_key: Some(key.clone()),
                ..request(None)
            };
            assert_eq!(chat_body(&request, &listed)["prompt_cache_key"], json!(key));
            let plain = chat_body(&request, &ModelCapabilities::minimal());
            assert_eq!(plain.get("prompt_cache_key"), None, "{role}");
            assert_eq!(plain.get("store"), None);
        }
        assert_eq!(
            chat_body(&request(None), &listed).get("prompt_cache_key"),
            None
        );
    }
}
