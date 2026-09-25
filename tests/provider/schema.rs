use kanade::infrastructure::llm::{
    CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, FinishReason, OutputSchema,
    OutputValidation,
};
use serde_json::json;

use super::support::{
    build_runner, build_runner_with, default_retry, request, tiny_request, tiny_response,
};

#[tokio::test]
async fn external_schema_references_are_rejected_before_the_provider_call() {
    let schemas = [
        (
            "$id",
            json!({"$id":"https://invalid.example/schema","type":"object"}),
        ),
        (
            "$dynamicRef",
            json!({"$dynamicRef":"#/thing","type":"object"}),
        ),
        (
            "$recursiveRef",
            json!({"$recursiveRef":"#/thing","type":"object"}),
        ),
        ("$ref", json!({"$ref":"https://invalid.example/schema"})),
    ];
    for (name, schema) in schemas {
        let mut input = tiny_request();
        input.output_schema = Some(OutputSchema {
            name: name.into(),
            schema,
            strict: true,
            validation: OutputValidation::Runner,
        });
        let (provider, runner) = build_runner([FakeAction::Response(tiny_response("m"))]);
        assert_eq!(
            runner.complete(&input).await.unwrap_err().code,
            ErrorCode::RequestInvalid,
            "{name}"
        );
        assert!(provider.requests().is_empty(), "{name}");
    }
}

#[tokio::test]
async fn local_schema_references_are_supported_without_retrieval() {
    let mut input = tiny_request();
    input.output_schema = Some(OutputSchema {
        name: "answer".into(),
        schema: json!({
            "$defs": {
                "answer": {
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"],
                    "additionalProperties": false
                }
            },
            "$ref": "#/$defs/answer"
        }),
        strict: true,
        validation: OutputValidation::Runner,
    });
    let output = CompletionResponse {
        model: "m".into(),
        content: Some(r#"{"answer":"ok"}"#.into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_output_bytes: 32,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn malformed_and_mismatched_structured_output_is_quarantined() {
    let malformed = CompletionResponse {
        content: Some("{not-json".into()),
        ..super::support::response()
    };
    let (_, runner) = build_runner([FakeAction::Response(malformed)]);
    assert_eq!(
        runner.complete(&request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );

    let mismatch = CompletionResponse {
        content: Some(r#"{"answer":7}"#.into()),
        ..super::support::response()
    };
    let (_, runner) = build_runner([FakeAction::Response(mismatch)]);
    assert_eq!(
        runner.complete(&request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
}

#[tokio::test]
async fn caller_validated_output_comes_back_raw_but_bounded() {
    let mut input = request();
    input.output_schema.as_mut().expect("schema").validation = OutputValidation::CallerValidates;
    for content in [Some("{not-json"), Some(r#"{"answer":7}"#), None] {
        let reply = CompletionResponse {
            content: content.map(str::to_owned),
            ..super::support::response()
        };
        let (provider, runner) = build_runner([FakeAction::Response(reply)]);
        let response = runner.complete(&input).await.expect("caller validates");
        assert_eq!(response.content.as_deref(), content);
        assert_eq!(provider.requests().len(), 1, "no retry for content");
    }

    let long = CompletionResponse {
        content: Some("x".repeat(64)),
        ..super::support::response()
    };
    let (_, runner) = build_runner_with(
        [FakeAction::Response(long)],
        ExecutionLimits {
            max_output_bytes: 32,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
}

#[tokio::test]
async fn schema_bytes_are_counted_in_the_request_budget() {
    let mut input = tiny_request();
    input.output_schema = Some(OutputSchema {
        name: "answer".into(),
        schema: json!({"type":"object","properties":{"answer":{"type":"string"}}}),
        strict: true,
        validation: OutputValidation::Runner,
    });
    let limits = ExecutionLimits {
        max_schema_bytes: 128,
        max_request_bytes: 20,
        ..ExecutionLimits::default()
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        limits,
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits {
            max_schema_bytes: 8,
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
