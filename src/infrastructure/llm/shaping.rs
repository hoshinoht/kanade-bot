use super::{ChatRequest, Effort, ErrorCode, LlmError, Message, ModelCapabilities};

const SCHEMA_INSTRUCTION: &str = "OUTPUT FORMAT\nAnswer with exactly one JSON value and nothing else: no prose, no markdown. It must validate against this JSON Schema:\n";

/// Fit `request` to `capabilities` before validation and accounting.
///
/// Returns `None` when nothing changes. `off` for a model that requires reasoning
/// becomes its lowest published level. A model without structured output gets the
/// output schema as one system instruction, merged into a leading system message
/// (chat templates often honour only one) or prepended; `output_schema` stays set so
/// the runner still validates the reply against it.
pub fn prepare(
    request: &ChatRequest,
    capabilities: &ModelCapabilities,
) -> Result<Option<ChatRequest>, LlmError> {
    if let Some(effort) = request.reasoning
        && effort != Effort::Off
        && capabilities.reasoning_control
        && capabilities
            .reasoning_efforts
            .as_ref()
            .is_some_and(|published| !published.contains(&effort))
    {
        return Err(LlmError::new(
            ErrorCode::UnsupportedCapability,
            "reasoning-effort",
        ));
    }
    if !capabilities.function_tools && uses_tools(request) {
        return Err(LlmError::new(
            ErrorCode::UnsupportedCapability,
            "function-tools",
        ));
    }
    super::wire::check(request, capabilities)
        .map_err(|reason| LlmError::new(ErrorCode::RequestInvalid, reason))?;
    let floor = (request.reasoning == Some(Effort::Off))
        .then(|| capabilities.reasoning_floor())
        .flatten();
    // Substituted here too so the placeholder counts toward every bound.
    let mut shaped = request
        .messages
        .iter()
        .any(|message| matches!(message, Message::Tool { content, .. } if content.is_empty()))
        .then(|| {
            let mut copy = request.clone();
            for message in &mut copy.messages {
                if let Message::Tool { content, .. } = message
                    && content.is_empty()
                {
                    super::wire::EMPTY_TOOL_RESULT.clone_into(content);
                }
            }
            copy
        });
    if let Some(floor) = floor {
        shaped.get_or_insert_with(|| request.clone()).reasoning = Some(floor);
    }
    let Some(output) = &request.output_schema else {
        return Ok(shaped);
    };
    if capabilities.structured_output {
        return Ok(shaped);
    }
    let instruction = schema_instruction(&output.schema)
        .map_err(|_| LlmError::new(ErrorCode::RequestInvalid, "schema"))?;
    let mut shaped = shaped.take().unwrap_or_else(|| request.clone());
    match shaped.messages.first_mut() {
        Some(Message::System { content }) => {
            content.push_str("\n\n");
            content.push_str(&instruction);
        }
        _ => shaped.messages.insert(
            0,
            Message::System {
                content: instruction,
            },
        ),
    }
    Ok(Some(shaped))
}

/// The system instruction [`prepare`] adds for a model without structured
/// output, so callers can budget for it before sending.
pub fn schema_instruction(schema: &serde_json::Value) -> Result<String, serde_json::Error> {
    Ok(format!(
        "{SCHEMA_INSTRUCTION}{}",
        serde_json::to_string(schema)?
    ))
}

/// Test support: the wire body the HTTP adapter would send for `request`.
#[cfg(any(test, feature = "test-support"))]
pub fn wire_body(
    request: &ChatRequest,
    capabilities: &ModelCapabilities,
) -> Result<serde_json::Value, LlmError> {
    let shaped = prepare(request, capabilities)?;
    Ok(super::wire::chat_body(
        shaped.as_ref().unwrap_or(request),
        capabilities,
    ))
}

fn uses_tools(request: &ChatRequest) -> bool {
    !request.tools.is_empty()
        || request.messages.iter().any(|message| match message {
            Message::Assistant { tool_calls, .. } => !tool_calls.is_empty(),
            Message::Tool { .. } => true,
            Message::System { .. } | Message::User { .. } => false,
        })
}

/// One fenced block around the whole reply, as models answering a prompt-borne
/// schema often emit; anything else is returned unchanged.
pub(crate) fn unfence(content: &str) -> Option<&str> {
    let body = content.trim().strip_prefix("```")?.strip_suffix("```")?;
    let body = body.strip_prefix("json").unwrap_or(body);
    let (header, body) = body.split_once('\n')?;
    if !header.trim_matches([' ', '\t']).is_empty() || body.contains("```") {
        return None;
    }
    Some(body.strip_suffix('\n').unwrap_or(body))
}

#[cfg(test)]
mod tests {
    use super::unfence;

    #[test]
    fn unfence_accepts_only_one_whole_fence() {
        assert_eq!(unfence("```json\n{\"a\":1}\n```"), Some("{\"a\":1}"));
        assert_eq!(unfence(" ```\n[1]```\n"), Some("[1]"));
        assert_eq!(unfence("{\"a\":1}"), None);
        assert_eq!(unfence("```json {}\n```"), None);
        assert_eq!(unfence("```\n{}\n```\n```\n{}\n```"), None);
        assert_eq!(unfence("text ```\n{}\n```"), None);
    }
}
