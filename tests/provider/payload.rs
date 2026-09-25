use kanade::infrastructure::llm::{
    CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, FinishReason, OutputSchema,
    OutputValidation, ToolCall,
};
use serde_json::{Value, json};

use super::support::{build_runner_with, default_retry, tiny_request, tool_request};

#[tokio::test]
async fn hundred_kib_plain_content_uses_payload_limit_not_metadata_limit() {
    let mut input = tiny_request();
    input.max_output_tokens = 100_000;
    let output = CompletionResponse {
        model: "m".into(),
        content: Some("p".repeat(100 * 1024)),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_output_bytes: 200 * 1024,
            token_budget: 200_000,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn hundred_kib_structured_content_uses_payload_limit_not_metadata_limit() {
    let mut input = tiny_request();
    input.max_output_tokens = 100_000;
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
    let content = serde_json::to_string(&json!({"answer": "s".repeat(100 * 1024)})).unwrap();
    let output = CompletionResponse {
        model: "m".into(),
        content: Some(content),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_output_bytes: 200 * 1024,
            token_budget: 200_000,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn hundred_kib_returned_tool_arguments_use_payload_limit_not_metadata_limit() {
    let mut input = tool_request();
    input.max_output_tokens = 100_000;
    let arguments = serde_json::to_string(&json!({"value": "a".repeat(100 * 1024)})).unwrap();
    let output = CompletionResponse {
        model: "m".into(),
        content: None,
        tool_calls: vec![ToolCall {
            id: "large".into(),
            name: "tool".into(),
            arguments,
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_output_bytes: 200 * 1024,
            token_budget: 200_000,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn raw_payload_limits_are_exact_for_plain_content_and_arguments() {
    let limit = 128;
    let mut input = tiny_request();
    input.max_output_tokens = 10_000;
    for (length, expected) in [(limit, None), (limit + 1, Some(ErrorCode::InvalidOutput))] {
        let output = CompletionResponse {
            model: "m".into(),
            content: Some("x".repeat(length)),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: None,
        };
        let (provider, runner) = build_runner_with(
            [FakeAction::Response(output)],
            ExecutionLimits {
                max_output_bytes: limit,
                token_budget: 20_000,
                ..ExecutionLimits::default()
            },
            default_retry(),
        );
        let result = runner.complete(&input).await;
        match expected {
            None => assert!(result.is_ok()),
            Some(code) => assert_eq!(result.unwrap_err().code, code),
        }
        assert_eq!(provider.requests().len(), 1);
    }

    let empty_arguments = r#"{"value":""}"#;
    let value_length = limit - empty_arguments.len();
    for (extra, expected) in [(0, None), (1, Some(ErrorCode::InvalidOutput))] {
        let arguments = format!(r#"{{"value":"{}"}}"#, "x".repeat(value_length + extra));
        assert_eq!(arguments.len(), limit + extra);
        let output = CompletionResponse {
            model: "m".into(),
            content: None,
            tool_calls: vec![ToolCall {
                id: "call".into(),
                name: "tool".into(),
                arguments,
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        };
        let (provider, runner) = build_runner_with(
            [FakeAction::Response(output)],
            ExecutionLimits {
                max_output_bytes: limit,
                token_budget: 20_000,
                ..ExecutionLimits::default()
            },
            default_retry(),
        );
        let result = runner.complete(&tool_request()).await;
        match expected {
            None => assert!(result.is_ok()),
            Some(code) => assert_eq!(result.unwrap_err().code, code),
        }
        assert_eq!(provider.requests().len(), 1);
    }
}

#[tokio::test]
async fn response_metadata_over_64_kib_still_fails() {
    let over = "x".repeat(64 * 1024 + 1);
    let metadata_cases = [
        CompletionResponse {
            model: "m".into(),
            content: None,
            tool_calls: vec![ToolCall {
                id: over.clone(),
                name: "tool".into(),
                arguments: r#"{"value":"ok"}"#.into(),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        CompletionResponse {
            model: "m".into(),
            content: None,
            tool_calls: vec![ToolCall {
                id: "call".into(),
                name: over.clone(),
                arguments: r#"{"value":"ok"}"#.into(),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        CompletionResponse {
            model: "m".into(),
            content: None,
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Other(over),
            usage: None,
        },
    ];
    for (index, output) in metadata_cases.into_iter().enumerate() {
        let input = if output.tool_calls.is_empty() {
            tiny_request()
        } else {
            tool_request()
        };
        let (provider, runner) = build_runner_with(
            [FakeAction::Response(output)],
            ExecutionLimits {
                max_output_bytes: 200 * 1024,
                token_budget: 20_000,
                ..ExecutionLimits::default()
            },
            default_retry(),
        );
        assert_eq!(
            runner.complete(&input).await.unwrap_err().code,
            ErrorCode::InvalidOutput,
            "metadata case {index}"
        );
        assert_eq!(provider.requests().len(), 1, "metadata case {index}");
    }
}

#[tokio::test]
async fn multiple_payloads_can_exceed_the_response_aggregate() {
    let mut input = tool_request();
    input.max_output_tokens = 400_000;
    let content = "c".repeat(260 * 1024);
    let arguments = serde_json::to_string(&json!({"value": "a".repeat(260 * 1024)})).unwrap();
    let output = CompletionResponse {
        model: "m".into(),
        content: Some(content),
        tool_calls: vec![ToolCall {
            id: "aggregate".into(),
            name: "tool".into(),
            arguments,
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_output_bytes: 300 * 1024,
            token_budget: 500_000,
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
async fn escaped_payloads_respect_raw_and_canonical_boundaries() {
    let mut input = tiny_request();
    input.max_output_tokens = 10_000;
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
    let value = json!({"answer":"quote \" slash \\ unicode ☃"});
    let canonical = serde_json::to_string(&value).unwrap();
    let escaped = r#"{"answer":"quote \u0022 slash \u005c unicode \u2603"}"#;
    let canonical_len = serde_json::to_vec(&value).unwrap().len();
    assert_eq!(serde_json::from_str::<Value>(escaped).unwrap(), value);
    assert!(escaped.len() > canonical_len);

    let cases = [(canonical, canonical_len), (escaped.into(), escaped.len())];
    for (payload, limit) in cases {
        let output = CompletionResponse {
            model: "m".into(),
            content: Some(payload.clone()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: None,
        };
        let (provider, runner) = build_runner_with(
            [FakeAction::Response(output.clone())],
            ExecutionLimits {
                max_output_bytes: limit,
                token_budget: 20_000,
                ..ExecutionLimits::default()
            },
            default_retry(),
        );
        assert!(runner.complete(&input).await.is_ok());
        assert_eq!(provider.requests().len(), 1);

        let (provider, runner) = build_runner_with(
            [FakeAction::Response(output)],
            ExecutionLimits {
                max_output_bytes: limit - 1,
                token_budget: 20_000,
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
}
