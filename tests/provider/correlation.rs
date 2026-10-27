use kanade_llm::{
    ChatRequest, CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, FinishReason, Message,
    ToolCallRequest, ToolDefinition,
};
use serde_json::json;

use super::support::{
    build_runner, calls_reply, response_call, response_tool_call, run_once, text_reply, tool_call,
    tool_request, tool_schema,
};

fn user(content: &str) -> Message {
    Message::User {
        content: content.into(),
    }
}

fn asks(tool_calls: Vec<ToolCallRequest>) -> Message {
    Message::Assistant {
        content: None,
        tool_calls,
    }
}

fn result(id: &str, content: &str) -> Message {
    Message::Tool {
        tool_call_id: id.into(),
        content: content.into(),
    }
}

fn with_messages(messages: Vec<Message>) -> ChatRequest {
    ChatRequest {
        messages,
        ..tool_request()
    }
}

/// `input` is refused before any request reaches the provider.
async fn assert_refused(input: &ChatRequest, label: &str) {
    let outcome = run_once(input, text_reply(None), ExecutionLimits::default()).await;
    assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0), "{label}");
}

/// The provider is asked once and its reply is rejected.
async fn assert_reply_rejected(input: &ChatRequest, output: CompletionResponse, label: &str) {
    let outcome = run_once(input, output, ExecutionLimits::default()).await;
    assert_eq!(outcome, (Some(ErrorCode::InvalidOutput), 1), "{label}");
}

#[tokio::test]
async fn externally_constructed_tool_calls_and_multicall_results_are_valid() {
    let input = with_messages(vec![
        user("go"),
        asks(vec![
            ToolCallRequest {
                id: "one".into(),
                name: "tool".into(),
                arguments: r#"{"value":"ok"}"#.into(),
            },
            tool_call("two"),
        ]),
        result("one", "first result"),
        result("two", "second result"),
        user("continue"),
    ]);
    let outcome = run_once(&input, text_reply(None), ExecutionLimits::default()).await;
    assert_eq!(outcome, (None, 1));
}

#[tokio::test]
async fn invalid_tool_histories_and_definitions_are_refused_before_the_call() {
    let mut duplicate_tools = tool_request();
    duplicate_tools.tools.push(ToolDefinition {
        name: "tool".into(),
        description: None,
        input_schema: tool_schema(),
    });
    let cases = [
        (
            "requested ids are global even after results",
            with_messages(vec![
                user("go"),
                asks(vec![tool_call("same")]),
                result("same", "done"),
                user("again"),
                asks(vec![tool_call("same")]),
            ]),
        ),
        (
            "missing result",
            with_messages(vec![user("go"), asks(vec![tool_call("missing")])]),
        ),
        (
            "unmatched result",
            with_messages(vec![user("go"), result("unknown", "result")]),
        ),
        (
            "duplicate requested ids",
            with_messages(vec![asks(vec![tool_call("same"), tool_call("same")])]),
        ),
        ("duplicate definitions", duplicate_tools),
    ];
    for (label, input) in cases {
        assert_refused(&input, label).await;
    }
}

#[tokio::test]
async fn historical_tool_calls_are_shape_checked_not_schema_checked() {
    let history = |arguments: String| {
        let mut input = tool_request();
        input.messages.extend([
            asks(vec![ToolCallRequest {
                id: "c1".into(),
                name: "tool".into(),
                arguments,
            }]),
            result("c1", "result"),
        ]);
        input
    };
    // Valid JSON the schema rejects: the model already sent it; not re-judged.
    let input = history(json!({"value": 7}).to_string());
    let words = text_reply(Some("done".into()));
    let outcome = run_once(&input, words, ExecutionLimits::default()).await;
    assert_eq!(outcome, (None, 1));

    for arguments in ["{not json".to_owned(), String::new()] {
        assert_refused(&history(arguments), "unreadable arguments").await;
    }

    let mut unanswered = tool_request();
    unanswered.messages.push(asks(vec![tool_call("c1")]));
    assert_refused(&unanswered, "unanswered").await;
}

#[tokio::test]
async fn invalid_response_tool_calls_are_rejected_without_execution() {
    let historical = with_messages(vec![
        user("go"),
        asks(vec![tool_call("historical")]),
        result("historical", "done"),
        user("continue"),
    ]);
    let mut stop_with_calls = calls_reply(vec![response_tool_call("one")]);
    stop_with_calls.finish_reason = FinishReason::Stop;
    let wrong_arguments = json!({"value": 7}).to_string();
    let cases = [
        (
            "duplicate ids",
            tool_request(),
            calls_reply(vec![response_tool_call("same"), response_tool_call("same")]),
        ),
        (
            "schema-invalid arguments",
            tool_request(),
            calls_reply(vec![response_call("wrong", "tool", wrong_arguments)]),
        ),
        (
            "reused historical id",
            historical,
            calls_reply(vec![response_tool_call("historical")]),
        ),
        ("stop with calls", tool_request(), stop_with_calls),
        (
            "tool_calls without calls",
            tool_request(),
            calls_reply(Vec::new()),
        ),
        (
            "undeclared tool",
            tool_request(),
            calls_reply(vec![response_call(
                "unknown",
                "not_declared",
                r#"{"value":"ok"}"#,
            )]),
        ),
    ];
    for (label, input, output) in cases {
        assert_reply_rejected(&input, output, label).await;
    }
}

#[tokio::test]
async fn retry_policy_stays_transient_only() {
    let (provider, runner) = build_runner([FakeAction::Authentication]);
    assert_eq!(
        runner.complete(&tool_request()).await.unwrap_err().code,
        ErrorCode::ProviderAuthentication
    );
    assert_eq!(provider.requests().len(), 1);
}
