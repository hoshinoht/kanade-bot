use kanade::infrastructure::llm::{
    CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, FinishReason, Message,
    OutputSchema, OutputValidation,
};
use serde_json::{Map, Value, json};

use super::support::{
    build_runner_with, default_retry, request, response, tiny_request, tiny_response,
};

#[tokio::test]
async fn canonical_request_size_matches_serde_json_at_both_boundaries() {
    let mut input = request();
    input.messages[0] = Message::System {
        content: "quote \" slash \\ line\ncontrol\tunicode ☃".repeat(8),
    };
    input.messages[1] = Message::User {
        content: "object { array [ ] }".repeat(8),
    };
    input.tools[0].description = Some("description \" \\ \n ☃".repeat(8));
    let encoded_len = serde_json::to_vec(&input).unwrap().len();

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(response())],
        ExecutionLimits {
            max_request_bytes: encoded_len,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(response())],
        ExecutionLimits {
            max_request_bytes: encoded_len - 1,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn canonical_schema_size_matches_serde_json_at_both_boundaries() {
    let key = "quote\"\\\n☃";
    let mut properties = Map::new();
    properties.insert(
        key.into(),
        json!({"type":"array","items":{"type":"string"}}),
    );
    let schema = json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": [key],
        "additionalProperties": false
    });
    let schema_len = serde_json::to_vec(&schema).unwrap().len();
    let mut input = tiny_request();
    input.output_schema = Some(OutputSchema {
        name: "answer".into(),
        schema,
        strict: true,
        validation: OutputValidation::Runner,
    });
    let mut output_object = Map::new();
    output_object.insert(key.into(), json!(["ok"]));
    let output = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: Some(serde_json::to_string(&Value::Object(output_object)).unwrap()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    };

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output.clone())],
        ExecutionLimits {
            max_schema_bytes: schema_len,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_schema_bytes: schema_len - 1,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn canonical_response_size_matches_serde_json_at_both_boundaries() {
    let output = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: Some("quote \" slash \\ line\ncontrol\tunicode ☃".repeat(16)),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    };
    let response_len = serde_json::to_vec(&output).unwrap().len();

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output.clone())],
        ExecutionLimits {
            max_response_bytes: response_len,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&tiny_request()).await.is_ok());
    assert_eq!(provider.requests().len(), 1);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_response_bytes: response_len - 1,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&tiny_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn canonical_structured_output_size_matches_serde_json_at_both_boundaries() {
    let mut input = tiny_request();
    input.output_schema = Some(OutputSchema {
        name: "answer".into(),
        schema: json!({
            "type":"object",
            "properties":{"answer":{"type":"string"}},
            "required":["answer"],
            "additionalProperties":false
        }),
        strict: true,
        validation: OutputValidation::Runner,
    });
    let mut output_object = Map::new();
    output_object.insert(
        "answer".into(),
        Value::String("quote \" slash \\ line\ncontrol\tunicode ☃".into()),
    );
    let value = Value::Object(output_object);
    let output_len = serde_json::to_vec(&value).unwrap().len();
    let output = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: Some(serde_json::to_string(&value).unwrap()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    };

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output.clone())],
        ExecutionLimits {
            max_output_bytes: output_len,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_output_bytes: output_len - 1,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn encoded_large_all_of_schema_fails_before_provider_or_compilation() {
    let schema = json!({"allOf": vec![Value::Bool(false); 11_000]});
    assert!(
        serde_json::to_vec(&schema).unwrap().len() > ExecutionLimits::default().max_schema_bytes
    );
    let mut input = tiny_request();
    input.output_schema = Some(OutputSchema {
        name: "large".into(),
        schema,
        strict: true,
        validation: OutputValidation::Runner,
    });
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits::default(),
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn encoded_escaping_changes_the_reservation() {
    let mut plain = tiny_request();
    plain.messages[0] = Message::User {
        content: "a".repeat(64),
    };
    let mut escaped = tiny_request();
    escaped.messages[0] = Message::User {
        content: "\"\\\n\t".repeat(16),
    };
    let plain_len = serde_json::to_vec(&plain).unwrap().len();
    let escaped_len = serde_json::to_vec(&escaped).unwrap().len();
    assert!(escaped_len > plain_len);
    let budget = u32::try_from(plain_len.div_ceil(4) + plain.max_output_tokens as usize).unwrap();

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits {
            token_budget: budget,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&plain).await.is_ok());
    assert_eq!(provider.requests().len(), 1);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits {
            token_budget: budget,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&escaped).await.unwrap_err().code,
        ErrorCode::BudgetExceeded
    );
    assert!(provider.requests().is_empty());
}
