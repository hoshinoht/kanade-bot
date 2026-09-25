use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, FinishReason, Message,
    OutputSchema, OutputValidation, ToolCall, ToolDefinition,
};
use serde_json::json;

use super::support::{
    build_runner_with, default_retry, response_tool_call, tiny_request, tiny_response, tool_call,
    tool_request, tool_schema,
};

#[tokio::test]
async fn individually_bounded_texts_and_schemas_still_obey_request_aggregate() {
    let input = ChatRequest {
        model: "model".into(),
        messages: vec![Message::User {
            content: "u".repeat(20),
        }],
        tools: vec![ToolDefinition {
            name: "tool".into(),
            description: Some("d".repeat(20)),
            input_schema: json!({"type":"object"}),
        }],
        output_schema: Some(OutputSchema {
            name: "output".into(),
            schema: json!({"type":"string"}),
            strict: true,
            validation: OutputValidation::Runner,
        }),
        max_output_tokens: 1,
        reasoning: None,
        sampling: None,
    };
    let limits = ExecutionLimits {
        max_content_bytes: 64,
        max_schema_bytes: 128,
        max_request_bytes: 40,
        ..ExecutionLimits::default()
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(CompletionResponse {
            model: "model".into(),
            content: Some("not-called".into()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: None,
        })],
        limits,
        default_retry(),
    );
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn request_string_fields_are_each_bounded_before_provider_call() {
    let long = "x".repeat(33);
    let mut cases = Vec::new();

    let mut value = tiny_request();
    value.model = long.clone();
    cases.push(("model", value));

    let mut value = tiny_request();
    value.messages = vec![Message::System {
        content: long.clone(),
    }];
    cases.push(("system content", value));

    let mut value = tiny_request();
    value.messages = vec![Message::User {
        content: long.clone(),
    }];
    cases.push(("user content", value));

    let mut value = tool_request();
    value.tools[0].name = long.clone();
    cases.push(("tool schema name", value));

    let mut value = tool_request();
    value.tools[0].description = Some(long.clone());
    cases.push(("tool description", value));

    let mut value = tool_request();
    value.output_schema = Some(OutputSchema {
        name: long.clone(),
        schema: json!({"type":"string"}),
        strict: true,
        validation: OutputValidation::Runner,
    });
    cases.push(("output schema name", value));

    let mut value = tool_request();
    value.messages = vec![Message::Assistant {
        content: Some(long.clone()),
        tool_calls: vec![tool_call("id")],
    }];
    cases.push(("assistant content", value));

    let mut value = tool_request();
    value.messages = vec![
        Message::Assistant {
            content: None,
            tool_calls: vec![tool_call("id")],
        },
        Message::Tool {
            tool_call_id: "id".into(),
            content: long.clone(),
        },
    ];
    cases.push(("tool result content", value));

    let mut value = tool_request();
    value.messages = vec![
        Message::Assistant {
            content: None,
            tool_calls: vec![tool_call("id")],
        },
        Message::Tool {
            tool_call_id: long.clone(),
            content: "ok".into(),
        },
    ];
    cases.push(("tool result id", value));

    let mut value = tool_request();
    value.messages = vec![Message::Assistant {
        content: None,
        tool_calls: vec![tool_call(&long)],
    }];
    cases.push(("requested call id", value));

    let mut value = tool_request();
    let mut call = tool_call("id");
    call.name = long.clone();
    value.messages = vec![Message::Assistant {
        content: None,
        tool_calls: vec![call],
    }];
    cases.push(("requested call name", value));

    let mut value = tool_request();
    let mut call = tool_call("id");
    call.arguments = long.clone();
    value.messages = vec![Message::Assistant {
        content: None,
        tool_calls: vec![call],
    }];
    cases.push(("requested call arguments", value));

    for (field, input) in cases {
        let limits = ExecutionLimits {
            max_content_bytes: 32,
            ..ExecutionLimits::default()
        };
        let (provider, runner) = build_runner_with(
            [FakeAction::Response(CompletionResponse {
                model: "m".into(),
                content: None,
                tool_calls: Vec::new(),
                finish_reason: FinishReason::Stop,
                usage: None,
            })],
            limits,
            default_retry(),
        );
        let error = runner.complete(&input).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::RequestInvalid, "{field}");
        assert!(provider.requests().is_empty(), "{field}");
    }
}

#[tokio::test]
async fn request_collection_bounds_fail_closed() {
    let mut too_many_messages = tiny_request();
    too_many_messages.messages = vec![
        Message::User {
            content: "a".into(),
        },
        Message::User {
            content: "b".into(),
        },
        Message::User {
            content: "c".into(),
        },
    ];
    let limits = ExecutionLimits {
        max_messages: 2,
        ..ExecutionLimits::default()
    };
    assert_request_invalid_before_call(too_many_messages, limits).await;

    let mut too_many_tools = tiny_request();
    too_many_tools.tools = vec![
        ToolDefinition {
            name: "one".into(),
            description: None,
            input_schema: tool_schema(),
        },
        ToolDefinition {
            name: "two".into(),
            description: None,
            input_schema: tool_schema(),
        },
    ];
    let limits = ExecutionLimits {
        max_tools: 1,
        ..ExecutionLimits::default()
    };
    assert_request_invalid_before_call(too_many_tools, limits).await;

    let mut too_many_calls = tool_request();
    too_many_calls.messages = vec![Message::Assistant {
        content: None,
        tool_calls: vec![tool_call("one"), tool_call("two")],
    }];
    let limits = ExecutionLimits {
        max_tools: 1,
        ..ExecutionLimits::default()
    };
    assert_request_invalid_before_call(too_many_calls, limits).await;
}

#[tokio::test]
async fn response_string_and_collection_bounds_fail_closed() {
    let response_cases = [
        (
            CompletionResponse {
                model: "m".into(),
                content: Some("x".repeat(33)),
                tool_calls: Vec::new(),
                finish_reason: FinishReason::Stop,
                usage: None,
            },
            None,
        ),
        (
            CompletionResponse {
                model: "m".into(),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "x".repeat(33),
                    name: "tool".into(),
                    arguments: r#"{"value":"ok"}"#.into(),
                }],
                finish_reason: FinishReason::ToolCalls,
                usage: None,
            },
            Some(ErrorCode::InvalidOutput),
        ),
        (
            CompletionResponse {
                model: "m".into(),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "id".into(),
                    name: "x".repeat(33),
                    arguments: r#"{"value":"ok"}"#.into(),
                }],
                finish_reason: FinishReason::ToolCalls,
                usage: None,
            },
            Some(ErrorCode::InvalidOutput),
        ),
        (
            CompletionResponse {
                model: "m".into(),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "id".into(),
                    name: "tool".into(),
                    arguments: "x".repeat(33),
                }],
                finish_reason: FinishReason::ToolCalls,
                usage: None,
            },
            Some(ErrorCode::InvalidOutput),
        ),
    ];
    for (index, (output, expected)) in response_cases.into_iter().enumerate() {
        let limits = ExecutionLimits {
            max_content_bytes: 32,
            ..ExecutionLimits::default()
        };
        let (provider, runner) = build_runner_with(
            [FakeAction::Response(output.clone())],
            limits,
            default_retry(),
        );
        let input = if output.tool_calls.is_empty() {
            tiny_request()
        } else {
            tool_request()
        };
        let result = runner.complete(&input).await;
        match expected {
            None => assert!(result.is_ok(), "case {index}"),
            Some(code) => assert_eq!(result.unwrap_err().code, code, "case {index}"),
        }
        assert_eq!(provider.requests().len(), 1, "case {index}");
    }

    let output = CompletionResponse {
        model: "m".into(),
        content: None,
        tool_calls: vec![response_tool_call("one"), response_tool_call("two")],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_tools: 1,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn response_aggregate_and_unknown_finish_reason_are_bounded() {
    let mut output = tiny_response("m");
    output.content = Some("x".repeat(16));
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_response_bytes: 20,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&tiny_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);

    let mut output = tiny_response("m");
    output.finish_reason = FinishReason::Other("x".repeat(33));
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(output)],
        ExecutionLimits {
            max_content_bytes: 32,
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

async fn assert_request_invalid_before_call(input: ChatRequest, limits: ExecutionLimits) {
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
}
