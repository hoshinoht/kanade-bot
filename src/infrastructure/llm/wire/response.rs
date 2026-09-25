use std::collections::BTreeSet;

use serde_json::Value;

use super::super::{
    ChatRequest, CompletionResponse, FinishReason, Message, ProviderFailure, ProviderFailureKind,
    ToolCall, Usage, shaping,
};

/// Normalize `choices[0]` and `usage` of one completion body.
///
/// A missing `model` echoes the request; a missing tool-call id gets a deterministic
/// unused `call_<index>` id (reused ids are left for the runner to reject); `stop`
/// with tool calls becomes `tool_calls`
/// (some OpenAI-compatible servers report both). Everything else is left for the
/// runner to validate.
pub(crate) fn parse_completion(
    body: &[u8],
    request: &ChatRequest,
    unfence: bool,
) -> Result<CompletionResponse, ProviderFailure> {
    let malformed = ProviderFailure {
        kind: ProviderFailureKind::InvalidOutput,
        reason_code: "malformed-completion",
    };
    let value: Value = serde_json::from_slice(body).map_err(|_| malformed.clone())?;
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .ok_or_else(|| malformed.clone())?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or(malformed)?;

    let mut content = message
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if unfence
        && let Some(text) = &content
        && let Some(inner) = shaping::unfence(text)
    {
        content = Some(inner.to_owned());
    }

    let mut reserved = historical_ids(request);
    let mut tool_calls = Vec::new();
    for (index, raw) in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let function = raw.get("function");
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let arguments = match function.and_then(|function| function.get("arguments")) {
            Some(Value::String(text)) => text.clone(),
            None | Some(Value::Null) => "{}".to_owned(),
            Some(other) => other.to_string(),
        };
        let id = match raw.get("id").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => id.to_owned(),
            _ => {
                let mut id = format!("call_{index}");
                while reserved.contains(&id) {
                    id.push('_');
                }
                id
            }
        };
        reserved.insert(id.clone());
        tool_calls.push(ToolCall {
            id,
            name,
            arguments,
        });
    }

    let finish_reason = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("stop") if !tool_calls.is_empty() => FinishReason::ToolCalls,
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(other) => FinishReason::Other(other.to_owned()),
        None => FinishReason::Other(String::new()),
    };
    let count = |key: &str| {
        value
            .get("usage")
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
    };
    let usage = match (count("prompt_tokens"), count("completion_tokens")) {
        (Some(prompt_tokens), Some(completion_tokens)) => Some(Usage {
            prompt_tokens,
            completion_tokens,
        }),
        _ => None,
    };
    Ok(CompletionResponse {
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&request.model)
            .to_owned(),
        content,
        tool_calls,
        finish_reason,
        usage,
    })
}

fn historical_ids(request: &ChatRequest) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for message in &request.messages {
        match message {
            Message::Assistant { tool_calls, .. } => {
                ids.extend(tool_calls.iter().map(|call| call.id.clone()));
            }
            Message::Tool { tool_call_id, .. } => {
                ids.insert(tool_call_id.clone());
            }
            Message::System { .. } | Message::User { .. } => {}
        }
    }
    ids
}
