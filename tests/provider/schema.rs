use kanade::infrastructure::llm::{
    CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, OutputValidation,
};
use serde_json::json;

use super::support::{
    build_runner, output_schema, request, response, run_once, text_reply, tiny_request,
    tiny_response,
};

fn max_output(max_output_bytes: usize) -> ExecutionLimits {
    ExecutionLimits {
        max_output_bytes,
        ..ExecutionLimits::default()
    }
}

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
        input.output_schema = Some(output_schema(name, schema));
        let outcome = run_once(&input, tiny_response("m"), ExecutionLimits::default()).await;
        assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0), "{name}");
    }
}

#[tokio::test]
async fn local_schema_references_are_supported_without_retrieval() {
    let mut input = tiny_request();
    input.output_schema = Some(output_schema(
        "answer",
        json!({
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
    ));
    let output = text_reply(Some(r#"{"answer":"ok"}"#.into()));
    assert_eq!(run_once(&input, output, max_output(32)).await, (None, 1));
}

#[tokio::test]
async fn malformed_and_mismatched_structured_output_is_quarantined() {
    for content in ["{not-json", r#"{"answer":7}"#] {
        let reply = CompletionResponse {
            content: Some(content.into()),
            ..response()
        };
        let outcome = run_once(&request(), reply, ExecutionLimits::default()).await;
        assert_eq!(outcome.0, Some(ErrorCode::InvalidOutput), "{content}");
    }
}

#[tokio::test]
async fn caller_validated_output_comes_back_raw_but_bounded() {
    let mut input = request();
    input.output_schema.as_mut().expect("schema").validation = OutputValidation::CallerValidates;
    for content in [Some("{not-json"), Some(r#"{"answer":7}"#), None] {
        let reply = CompletionResponse {
            content: content.map(str::to_owned),
            ..response()
        };
        let (provider, runner) = build_runner([FakeAction::Response(reply)]);
        let response = runner.complete(&input).await.expect("caller validates");
        assert_eq!(response.content.as_deref(), content);
        assert_eq!(provider.requests().len(), 1, "no retry for content");
    }

    let long = CompletionResponse {
        content: Some("x".repeat(64)),
        ..response()
    };
    let outcome = run_once(&input, long, max_output(32)).await;
    assert_eq!(outcome.0, Some(ErrorCode::InvalidOutput));
}

#[tokio::test]
async fn schema_bytes_are_counted_in_the_request_budget() {
    let mut input = tiny_request();
    input.output_schema = Some(output_schema(
        "answer",
        json!({"type":"object","properties":{"answer":{"type":"string"}}}),
    ));
    let request_bound = ExecutionLimits {
        max_schema_bytes: 128,
        max_request_bytes: 20,
        ..ExecutionLimits::default()
    };
    let schema_bound = ExecutionLimits {
        max_schema_bytes: 8,
        ..ExecutionLimits::default()
    };
    for limits in [request_bound, schema_bound] {
        let outcome = run_once(&input, tiny_response("m"), limits).await;
        assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0));
    }
}
