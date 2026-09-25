//! Chat tool rounds: a tools-withheld round may follow rounds that used tools,
//! and chat replies validate tool calls leniently (v4) while every other call
//! kind stays strict.

use super::support::{build_runner, tool_request};
use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, ErrorCode, FakeAction, FinishReason, Message, ToolCall,
    ToolCallRequest, ToolCallValidation, governor::CallKind,
};

fn words() -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: "m".into(),
        content: Some("answered in words".into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })
}

fn calls(calls: Vec<ToolCall>) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: "m".into(),
        content: None,
        tool_calls: calls,
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
    }
}

fn round(id: &str, name: &str) -> [Message; 2] {
    [
        Message::Assistant {
            content: None,
            tool_calls: vec![ToolCallRequest {
                id: id.into(),
                name: name.into(),
                arguments: "{}".into(),
            }],
        },
        Message::Tool {
            tool_call_id: id.into(),
            content: "result".into(),
        },
    ]
}

/// The final round: earlier rounds called tools, this one offers none.
fn withheld() -> ChatRequest {
    let mut request = tool_request();
    request.tools.clear();
    request.messages.extend(round("r1", "tool"));
    request.messages.extend(round("r2", "list_bosses"));
    request
}

#[tokio::test]
async fn a_tools_withheld_round_after_tool_rounds_is_sent() {
    for kind in [CallKind::Chat, CallKind::Extraction] {
        let (provider, runner) = build_runner([words()]);
        let reply = runner.complete_as(&withheld(), kind).await.unwrap();
        assert_eq!(reply.content.as_deref(), Some("answered in words"));
        assert_eq!(provider.requests(), vec![withheld()]);
    }
}

#[tokio::test]
async fn history_is_still_checked_for_ids_pairing_and_name_format() {
    let mut mismatched = withheld();
    if let Some(Message::Tool { tool_call_id, .. }) = mismatched.messages.last_mut() {
        *tool_call_id = "other".into();
    }
    let mut duplicated = withheld();
    duplicated.messages.extend(round("r1", "tool"));
    let mut unnamed = withheld();
    unnamed.messages.extend(round("r3", "bad name"));
    let mut empty_id = withheld();
    empty_id.messages.extend(round("", "tool"));
    for (label, request) in [
        ("mismatched result id", mismatched),
        ("duplicate id", duplicated),
        ("name format", unnamed),
        ("empty id", empty_id),
    ] {
        let (provider, runner) = build_runner([words()]);
        assert_eq!(
            runner
                .complete_as(&request, CallKind::Chat)
                .await
                .unwrap_err()
                .code,
            ErrorCode::RequestInvalid,
            "{label}"
        );
        assert!(provider.requests().is_empty(), "{label}");
    }
}

#[test]
fn only_chat_is_lenient() {
    assert_eq!(
        CallKind::Chat.tool_call_validation(),
        ToolCallValidation::Lenient
    );
    for kind in [
        CallKind::Extraction,
        CallKind::Rescan,
        CallKind::FollowUp,
        CallKind::Rewrite,
        CallKind::PreScreen,
    ] {
        assert_eq!(kind.tool_call_validation(), ToolCallValidation::Strict);
    }
    assert_eq!(ToolCallValidation::default(), ToolCallValidation::Strict);
}

#[tokio::test]
async fn lenient_replies_return_unoffered_and_schema_invalid_calls() {
    let steerable = vec![
        call("c1", "propose_add", r#"{"boss":"hlimbo"}"#),
        call("c2", "tool", r#"{"value":7}"#),
        call("c3", "tool", "{}"),
    ];
    let (_, runner) = build_runner([calls(steerable.clone())]);
    let reply = runner
        .complete_as(&tool_request(), CallKind::Chat)
        .await
        .unwrap();
    assert_eq!(reply.tool_calls, steerable);

    // A withheld round's reply that still calls tools goes to the caller too.
    let (_, runner) = build_runner([calls(vec![call("c9", "tool", "{}")])]);
    let reply = runner
        .complete_as(&withheld(), CallKind::Chat)
        .await
        .unwrap();
    assert_eq!(reply.tool_calls.len(), 1);
}

#[tokio::test]
async fn strict_replies_are_unchanged() {
    for bad in [
        call("c1", "propose_add", "{}"),
        call("c1", "tool", r#"{"value":7}"#),
    ] {
        let (_, runner) = build_runner([calls(vec![bad])]);
        assert_eq!(
            runner.complete(&tool_request()).await.unwrap_err().code,
            ErrorCode::InvalidOutput
        );
    }
}

#[tokio::test]
async fn lenient_replies_still_reject_unreadable_calls() {
    let cases = [
        ("not json", vec![call("c1", "tool", "{not json")]),
        ("not an object", vec![call("c1", "tool", "[1, 2]")]),
        ("scalar", vec![call("c1", "tool", "7")]),
        ("empty id", vec![call("", "tool", "{}")]),
        (
            "duplicate ids",
            vec![call("c1", "tool", "{}"), call("c1", "tool", "{}")],
        ),
        ("name format", vec![call("c1", "bad name", "{}")]),
        ("empty name", vec![call("c1", "", "{}")]),
    ];
    for (label, bad) in cases {
        let (_, runner) = build_runner([calls(bad)]);
        assert_eq!(
            runner
                .complete_as(&tool_request(), CallKind::Chat)
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidOutput,
            "{label}"
        );
    }
    // An id already used in the transcript is a duplicate too.
    let (_, runner) = build_runner([calls(vec![call("r1", "tool", "{}")])]);
    assert_eq!(
        runner
            .complete_as(&withheld(), CallKind::Chat)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidOutput
    );
}
