use std::{sync::Arc, time::Duration};

use kanade_llm::{
    ChatRequest, CompletionResponse, CompletionRunner, ErrorCode, ExecutionLimits, FakeAction,
    FakeProvider, FinishReason, Message, OutputSchema, OutputValidation, RetryPolicy, ToolCall,
    ToolCallRequest, ToolDefinition, Usage,
};
use serde_json::{Map, Value, json};

pub fn request() -> ChatRequest {
    ChatRequest {
        model: "synthetic-model-v1".into(),
        messages: vec![
            Message::System {
                content: "synthetic system prompt".into(),
            },
            Message::User {
                content: "what is next?".into(),
            },
        ],
        tools: vec![ToolDefinition {
            name: "read_schedule".into(),
            description: Some("synthetic test tool".into()),
            input_schema: json!({"type":"object","properties":{"week":{"type":"string","enum":["next"]}},"required":["week"],"additionalProperties":false}),
        }],
        output_schema: Some(answer_schema()),
        max_output_tokens: 128,
        reasoning: None,
        sampling: None,
        prompt_cache_key: None,
    }
}

pub fn response() -> CompletionResponse {
    serde_json::from_str(include_str!(
        "../fixtures/provider/structured-response.json"
    ))
    .unwrap()
}

pub fn default_retry() -> RetryPolicy {
    RetryPolicy {
        total_deadline: Duration::from_secs(2),
        max_attempts: 3,
        backoff: Duration::from_millis(10),
    }
}

pub fn build_runner(
    actions: impl IntoIterator<Item = FakeAction>,
) -> (Arc<FakeProvider>, CompletionRunner<FakeProvider>) {
    build_runner_with(actions, ExecutionLimits::default(), default_retry())
}

pub fn build_runner_with(
    actions: impl IntoIterator<Item = FakeAction>,
    limits: ExecutionLimits,
    retry: RetryPolicy,
) -> (Arc<FakeProvider>, CompletionRunner<FakeProvider>) {
    let provider = Arc::new(FakeProvider::new(actions));
    let runner = CompletionRunner::ungoverned(provider.clone(), limits, retry).unwrap();
    (provider, runner)
}

/// Completes `input` once against a fake answering `output` under `limits`:
/// the error code (`None` on success) and how many requests reached the provider.
pub async fn run_once(
    input: &ChatRequest,
    output: CompletionResponse,
    limits: ExecutionLimits,
) -> (Option<ErrorCode>, usize) {
    let (provider, runner) =
        build_runner_with([FakeAction::Response(output)], limits, default_retry());
    let code = runner.complete(input).await.err().map(|error| error.code);
    (code, provider.requests().len())
}

/// A strict, runner-validated output schema.
pub fn output_schema(name: &str, schema: Value) -> OutputSchema {
    OutputSchema {
        name: name.into(),
        schema,
        strict: true,
        validation: OutputValidation::Runner,
    }
}

/// The closed `{"answer": string}` output schema.
pub fn answer_schema() -> OutputSchema {
    output_schema(
        "answer",
        json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}),
    )
}

/// A reply from model `m` without usage or tool calls, finishing `Stop`.
pub fn text_reply(content: Option<String>) -> CompletionResponse {
    CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

/// A reply from model `m` without usage or content, finishing `ToolCalls`.
pub fn calls_reply(tool_calls: Vec<ToolCall>) -> CompletionResponse {
    CompletionResponse {
        tool_calls,
        finish_reason: FinishReason::ToolCalls,
        ..text_reply(None)
    }
}

pub fn response_call(id: &str, name: &str, arguments: impl Into<String>) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
    }
}

pub fn tiny_request() -> ChatRequest {
    ChatRequest {
        model: "m".into(),
        messages: vec![Message::User {
            content: "u".into(),
        }],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 1,
        reasoning: None,
        sampling: None,
        prompt_cache_key: None,
    }
}

pub fn tiny_response(model: &str) -> CompletionResponse {
    CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: model.into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: None,
        }),
    }
}

pub fn tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"value": {"type": "string"}},
        "required": ["value"],
        "additionalProperties": false
    })
}

pub fn tool_request() -> ChatRequest {
    ChatRequest {
        model: "m".into(),
        messages: vec![Message::User {
            content: "go".into(),
        }],
        tools: vec![ToolDefinition {
            name: "tool".into(),
            description: None,
            input_schema: tool_schema(),
        }],
        output_schema: None,
        max_output_tokens: 1,
        reasoning: None,
        sampling: None,
        prompt_cache_key: None,
    }
}

pub fn tool_call(id: &str) -> ToolCallRequest {
    ToolCallRequest {
        id: id.into(),
        name: "tool".into(),
        arguments: r#"{"value":"ok"}"#.into(),
    }
}

pub fn response_tool_call(id: &str) -> ToolCall {
    response_call(id, "tool", r#"{"value":"ok"}"#)
}

pub fn deep_schema(depth: usize) -> Value {
    let mut schema = json!({"type": "string"});
    for _ in 0..depth {
        let mut properties = Map::new();
        properties.insert("next".into(), schema);
        let mut object = Map::new();
        object.insert("type".into(), Value::String("object".into()));
        object.insert("properties".into(), Value::Object(properties));
        object.insert("required".into(), json!(["next"]));
        object.insert("additionalProperties".into(), Value::Bool(false));
        schema = Value::Object(object);
    }
    schema
}

pub fn wide_value(width: usize) -> Value {
    Value::Array((0..width).map(|_| Value::Null).collect())
}

pub fn dispose_json_value_iteratively(value: Value) {
    let mut work = vec![value];
    while let Some(value) = work.pop() {
        match value {
            Value::Array(values) => work.extend(values),
            Value::Object(map) => work.extend(map.into_values()),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
}
