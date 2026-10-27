use kanade_llm::{
    ChatRequest, ErrorCode, ExecutionLimits, FinishReason, Message, ToolCallRequest, ToolDefinition,
};
use serde_json::json;

use super::support::{
    calls_reply, output_schema, response_call, response_tool_call, run_once, text_reply,
    tiny_request, tiny_response, tool_call, tool_request, tool_schema,
};

fn content_limit() -> ExecutionLimits {
    ExecutionLimits {
        max_content_bytes: 32,
        ..ExecutionLimits::default()
    }
}

fn asks(tool_calls: Vec<ToolCallRequest>) -> Message {
    Message::Assistant {
        content: None,
        tool_calls,
    }
}

fn with_messages(base: ChatRequest, messages: Vec<Message>) -> ChatRequest {
    ChatRequest { messages, ..base }
}

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
        output_schema: Some(output_schema("output", json!({"type":"string"}))),
        max_output_tokens: 1,
        reasoning: None,
        sampling: None,
        prompt_cache_key: None,
    };
    let limits = ExecutionLimits {
        max_content_bytes: 64,
        max_schema_bytes: 128,
        max_request_bytes: 40,
        ..ExecutionLimits::default()
    };
    let output = text_reply(Some("not-called".into()));
    let outcome = run_once(&input, output, limits).await;
    assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0));
}

/// A key outside Kanata's bound (1–64 printable ASCII bytes) is refused
/// locally before anything is sent.
#[tokio::test]
async fn a_prompt_cache_key_outside_the_gateway_bound_is_refused_before_sending() {
    let keyed = |key: String| ChatRequest {
        prompt_cache_key: Some(key),
        ..tiny_request()
    };
    for key in [
        String::new(),
        "k".repeat(65),
        "kanade\tchat".into(),
        "kanade-é".into(),
    ] {
        let outcome = run_once(
            &keyed(key.clone()),
            tiny_response("m"),
            ExecutionLimits::default(),
        )
        .await;
        assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0), "{key:?}");
    }
    for key in ["k".repeat(64), "kanade chat".into()] {
        let outcome = run_once(
            &keyed(key.clone()),
            tiny_response("m"),
            ExecutionLimits::default(),
        )
        .await;
        assert_eq!(outcome, (None, 1), "{key:?}");
    }
}

#[tokio::test]
async fn request_string_fields_are_each_bounded_before_provider_call() {
    let long = "x".repeat(33);
    let mut cases = Vec::new();

    let mut value = tiny_request();
    value.model = long.clone();
    cases.push(("model", value));

    let system = Message::System {
        content: long.clone(),
    };
    cases.push((
        "system content",
        with_messages(tiny_request(), vec![system]),
    ));

    let user = Message::User {
        content: long.clone(),
    };
    cases.push(("user content", with_messages(tiny_request(), vec![user])));

    let mut value = tool_request();
    value.tools[0].name = long.clone();
    cases.push(("tool schema name", value));

    let mut value = tool_request();
    value.tools[0].description = Some(long.clone());
    cases.push(("tool description", value));

    let mut value = tool_request();
    value.output_schema = Some(output_schema(&long, json!({"type":"string"})));
    cases.push(("output schema name", value));

    let assistant = Message::Assistant {
        content: Some(long.clone()),
        tool_calls: vec![tool_call("id")],
    };
    cases.push((
        "assistant content",
        with_messages(tool_request(), vec![assistant]),
    ));

    for (field, call_id, content) in [
        ("tool result content", "id", long.as_str()),
        ("tool result id", long.as_str(), "ok"),
    ] {
        let result = Message::Tool {
            tool_call_id: call_id.into(),
            content: content.into(),
        };
        let messages = vec![asks(vec![tool_call("id")]), result];
        cases.push((field, with_messages(tool_request(), messages)));
    }

    let mut long_name = tool_call("id");
    long_name.name = long.clone();
    let mut long_arguments = tool_call("id");
    long_arguments.arguments = long.clone();
    for (field, call) in [
        ("requested call id", tool_call(&long)),
        ("requested call name", long_name),
        ("requested call arguments", long_arguments),
    ] {
        cases.push((field, with_messages(tool_request(), vec![asks(vec![call])])));
    }

    for (field, input) in cases {
        let outcome = run_once(&input, text_reply(None), content_limit()).await;
        assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0), "{field}");
    }
}

#[tokio::test]
async fn request_collection_bounds_fail_closed() {
    let users = ["a", "b", "c"].map(|content| Message::User {
        content: content.into(),
    });
    let too_many_messages = with_messages(tiny_request(), users.into());
    let mut too_many_tools = tiny_request();
    too_many_tools.tools = ["one", "two"]
        .map(|name| ToolDefinition {
            name: name.into(),
            description: None,
            input_schema: tool_schema(),
        })
        .into();
    let too_many_calls = with_messages(
        tool_request(),
        vec![asks(vec![tool_call("one"), tool_call("two")])],
    );
    let max_messages = ExecutionLimits {
        max_messages: 2,
        ..ExecutionLimits::default()
    };
    let max_tools = ExecutionLimits {
        max_tools: 1,
        ..ExecutionLimits::default()
    };
    for (label, input, limits) in [
        ("messages", too_many_messages, max_messages),
        ("tools", too_many_tools, max_tools.clone()),
        ("requested calls", too_many_calls, max_tools),
    ] {
        let outcome = run_once(&input, tiny_response("m"), limits).await;
        assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0), "{label}");
    }
}

#[tokio::test]
async fn response_string_and_collection_bounds_fail_closed() {
    let long = "x".repeat(33);
    let ok = r#"{"value":"ok"}"#;
    let invalid = Some(ErrorCode::InvalidOutput);
    let response_cases = [
        (text_reply(Some(long.clone())), None),
        (calls_reply(vec![response_call(&long, "tool", ok)]), invalid),
        (calls_reply(vec![response_call("id", &long, ok)]), invalid),
        (
            calls_reply(vec![response_call("id", "tool", long.clone())]),
            invalid,
        ),
    ];
    for (index, (output, expected)) in response_cases.into_iter().enumerate() {
        let input = if output.tool_calls.is_empty() {
            tiny_request()
        } else {
            tool_request()
        };
        let outcome = run_once(&input, output, content_limit()).await;
        assert_eq!(outcome, (expected, 1), "case {index}");
    }

    let output = calls_reply(vec![response_tool_call("one"), response_tool_call("two")]);
    let max_tools = ExecutionLimits {
        max_tools: 1,
        ..ExecutionLimits::default()
    };
    let outcome = run_once(&tool_request(), output, max_tools).await;
    assert_eq!(outcome, (invalid, 1));
}

#[tokio::test]
async fn response_aggregate_and_unknown_finish_reason_are_bounded() {
    let mut output = tiny_response("m");
    output.content = Some("x".repeat(16));
    let max_response = ExecutionLimits {
        max_response_bytes: 20,
        ..ExecutionLimits::default()
    };
    let outcome = run_once(&tiny_request(), output, max_response).await;
    assert_eq!(outcome, (Some(ErrorCode::InvalidOutput), 1));

    let mut output = tiny_response("m");
    output.finish_reason = FinishReason::Other("x".repeat(33));
    let outcome = run_once(&tiny_request(), output, content_limit()).await;
    assert_eq!(outcome, (Some(ErrorCode::InvalidOutput), 1));
}
