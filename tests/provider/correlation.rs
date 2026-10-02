use kanade::infrastructure::llm::{
    CompletionResponse, ErrorCode, FakeAction, FinishReason, Message, ToolCall, ToolCallRequest,
    ToolDefinition,
};
use serde_json::json;

use super::support::{
    build_runner, build_runner_with, default_retry, response_tool_call, tool_call, tool_request,
    tool_schema,
};

#[tokio::test]
async fn externally_constructed_tool_calls_and_multicall_results_are_valid() {
    let mut input = tool_request();
    input.messages = vec![
        Message::User {
            content: "go".into(),
        },
        Message::Assistant {
            content: None,
            tool_calls: vec![
                ToolCallRequest {
                    id: "one".into(),
                    name: "tool".into(),
                    arguments: r#"{"value":"ok"}"#.into(),
                },
                tool_call("two"),
            ],
        },
        Message::Tool {
            tool_call_id: "one".into(),
            content: "first result".into(),
        },
        Message::Tool {
            tool_call_id: "two".into(),
            content: "second result".into(),
        },
        Message::User {
            content: "continue".into(),
        },
    ];
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })]);
    assert!(runner.complete(&input).await.is_ok());
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn requested_tool_ids_are_global_even_after_results() {
    let mut input = tool_request();
    input.messages = vec![
        Message::User {
            content: "go".into(),
        },
        Message::Assistant {
            content: None,
            tool_calls: vec![tool_call("same")],
        },
        Message::Tool {
            tool_call_id: "same".into(),
            content: "done".into(),
        },
        Message::User {
            content: "again".into(),
        },
        Message::Assistant {
            content: None,
            tool_calls: vec![tool_call("same")],
        },
    ];
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })]);
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn missing_and_unmatched_tool_results_are_rejected() {
    let mut missing = tool_request();
    missing.messages = vec![
        Message::User {
            content: "go".into(),
        },
        Message::Assistant {
            content: None,
            tool_calls: vec![tool_call("missing")],
        },
    ];
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })]);
    assert_eq!(
        runner.complete(&missing).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());

    let mut unmatched = tool_request();
    unmatched.messages = vec![
        Message::User {
            content: "go".into(),
        },
        Message::Tool {
            tool_call_id: "unknown".into(),
            content: "result".into(),
        },
    ];
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })]);
    assert_eq!(
        runner.complete(&unmatched).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn requested_tool_ids_and_definitions_cannot_be_duplicated() {
    let mut duplicate_calls = tool_request();
    duplicate_calls.messages = vec![Message::Assistant {
        content: None,
        tool_calls: vec![tool_call("same"), tool_call("same")],
    }];
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })]);
    assert_eq!(
        runner.complete(&duplicate_calls).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());

    let mut duplicate_tools = tool_request();
    duplicate_tools.tools.push(ToolDefinition {
        name: "tool".into(),
        description: None,
        input_schema: tool_schema(),
    });
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })]);
    assert_eq!(
        runner.complete(&duplicate_tools).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn historical_tool_calls_are_shape_checked_not_schema_checked() {
    let words = || {
        FakeAction::Response(CompletionResponse {
            reasoning_content: None,
            reasoning_tokens: None,
            model: "m".into(),
            content: Some("done".into()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: None,
        })
    };
    let history = |arguments: String| {
        let mut input = tool_request();
        input.messages.extend([
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: "c1".into(),
                    name: "tool".into(),
                    arguments,
                }],
            },
            Message::Tool {
                tool_call_id: "c1".into(),
                content: "result".into(),
            },
        ]);
        input
    };
    // Valid JSON the schema rejects: the model already sent it; not re-judged.
    let (provider, runner) = build_runner([words()]);
    runner
        .complete(&history(json!({"value": 7}).to_string()))
        .await
        .unwrap();
    assert_eq!(provider.requests().len(), 1);

    for arguments in ["{not json".to_owned(), String::new()] {
        let (provider, runner) = build_runner([words()]);
        assert_eq!(
            runner.complete(&history(arguments)).await.unwrap_err().code,
            ErrorCode::RequestInvalid
        );
        assert!(provider.requests().is_empty());
    }

    let mut unanswered = tool_request();
    unanswered.messages.push(Message::Assistant {
        content: None,
        tool_calls: vec![tool_call("c1")],
    });
    let (provider, runner) = build_runner([words()]);
    assert_eq!(
        runner.complete(&unanswered).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn response_tool_ids_and_arguments_are_correlated_and_schema_checked() {
    let duplicate = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: vec![response_tool_call("same"), response_tool_call("same")],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner([FakeAction::Response(duplicate)]);
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);

    let wrong_arguments = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: vec![ToolCall {
            id: "wrong".into(),
            name: "tool".into(),
            arguments: json!({"value": 7}).to_string(),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner([FakeAction::Response(wrong_arguments)]);
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn response_ids_cannot_reuse_historical_request_ids() {
    let mut input = tool_request();
    input.messages = vec![
        Message::User {
            content: "go".into(),
        },
        Message::Assistant {
            content: None,
            tool_calls: vec![tool_call("historical")],
        },
        Message::Tool {
            tool_call_id: "historical".into(),
            content: "done".into(),
        },
        Message::User {
            content: "continue".into(),
        },
    ];
    let output = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: vec![response_tool_call("historical")],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner([FakeAction::Response(output)]);
    assert_eq!(
        runner.complete(&input).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn finish_reason_must_agree_with_tool_calls() {
    let stop_with_calls = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: vec![response_tool_call("one")],
        finish_reason: FinishReason::Stop,
        usage: None,
    };
    let (provider, runner) = build_runner([FakeAction::Response(stop_with_calls)]);
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);

    let calls_without_calls = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner([FakeAction::Response(calls_without_calls)]);
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn unknown_response_tools_are_rejected_without_execution() {
    let output = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: "m".into(),
        content: None,
        tool_calls: vec![ToolCall {
            id: "unknown".into(),
            name: "not_declared".into(),
            arguments: r#"{"value":"ok"}"#.into(),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };
    let (provider, runner) = build_runner([FakeAction::Response(output)]);
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::InvalidOutput
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn retry_policy_stays_transient_only() {
    let (provider, runner) = build_runner_with(
        [FakeAction::Authentication],
        kanade::infrastructure::llm::ExecutionLimits::default(),
        default_retry(),
    );
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::ProviderAuthentication
    );
    assert_eq!(provider.requests().len(), 1);
}
