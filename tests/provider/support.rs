use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, CompletionRunner, ExecutionLimits, FakeAction, FakeProvider,
    FinishReason, Message, OutputSchema, RetryPolicy, ToolCall, ToolCallRequest, ToolDefinition,
    Usage,
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
        output_schema: Some(OutputSchema {
            name: "answer".into(),
            schema: json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}),
            strict: true,
        }),
        max_output_tokens: 128,
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
    let runner = CompletionRunner::new(provider.clone(), limits, retry).unwrap();
    (provider, runner)
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
    }
}

pub fn tiny_response(model: &str) -> CompletionResponse {
    CompletionResponse {
        model: model.into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
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
    ToolCall {
        id: id.into(),
        name: "tool".into(),
        arguments: r#"{"value":"ok"}"#.into(),
    }
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

pub fn take_request_schemas(request: &mut ChatRequest) {
    if let Some(output) = request.output_schema.take() {
        dispose_json_value_iteratively(output.schema);
    }
    for tool in &mut request.tools {
        let schema = std::mem::replace(&mut tool.input_schema, Value::Null);
        dispose_json_value_iteratively(schema);
    }
}
