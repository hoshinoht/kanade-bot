//! Request shapes the Kanata gateway validates: values it would refuse fail
//! locally (never as a capability downgrade), reasoning efforts and `off`,
//! empty content, and the `x-request-id` correlation header.

use std::time::Duration;

use kanade::infrastructure::llm::{
    ChatRequest, Effort, ErrorCode, Message, ModelCapabilities, OutputSchema, OutputValidation,
    Sampling, ToolCallRequest, ToolDefinition, governor::QuestionLimits, wire_body,
};
use serde_json::{Value, json};

use super::{
    http_admission::client,
    http_transport::{declared, models_or, runner, structured},
    stub::{Reply, Stub, completion},
    support::{deep_schema, tiny_request},
};

fn full() -> ModelCapabilities {
    ModelCapabilities {
        structured_output: true,
        sampling_controls: true,
        reasoning_control: true,
        function_tools: true,
        ..ModelCapabilities::minimal()
    }
}

fn refused(request: &ChatRequest, caps: &ModelCapabilities) -> ErrorCode {
    wire_body(request, caps).unwrap_err().code
}

fn with_schema(name: &str, schema: Value) -> ChatRequest {
    let mut request = tiny_request();
    request.output_schema = Some(OutputSchema {
        name: name.into(),
        schema,
        strict: true,
        validation: OutputValidation::Runner,
    });
    request
}

fn sampled(sampling: Sampling) -> ChatRequest {
    let mut request = tiny_request();
    request.sampling = Some(sampling);
    request
}

#[test]
fn values_kanata_would_refuse_fail_locally() {
    let object = json!({"type": "object"});
    let cases = [
        with_schema("has space", object.clone()),
        with_schema("", object.clone()),
        with_schema(&"n".repeat(65), object.clone()),
        with_schema("answer", json!(["not", "an", "object"])),
        with_schema("answer", deep_schema(40)),
        with_schema(
            "answer",
            json!({"type": "object", "description": "x".repeat(70 * 1024)}),
        ),
        sampled(Sampling {
            temperature: Some(2.5),
            ..Sampling::default()
        }),
        sampled(Sampling {
            temperature: Some(f64::NAN),
            ..Sampling::default()
        }),
        sampled(Sampling {
            top_p: Some(0.0),
            ..Sampling::default()
        }),
        sampled(Sampling {
            top_p: Some(1.5),
            ..Sampling::default()
        }),
        sampled(Sampling {
            seed: Some(u64::MAX),
            ..Sampling::default()
        }),
        ChatRequest {
            max_output_tokens: 1_048_577,
            ..tiny_request()
        },
        ChatRequest {
            max_output_tokens: 0,
            ..tiny_request()
        },
        ChatRequest {
            tools: vec![ToolDefinition {
                name: "read schedule".into(),
                description: None,
                input_schema: json!({"type": "object"}),
            }],
            ..tiny_request()
        },
        ChatRequest {
            messages: vec![Message::User {
                content: String::new(),
            }],
            ..tiny_request()
        },
        ChatRequest {
            messages: vec![
                Message::System {
                    content: String::new(),
                },
                Message::User {
                    content: "u".into(),
                },
            ],
            ..tiny_request()
        },
        ChatRequest {
            messages: vec![
                Message::User {
                    content: "u".into(),
                },
                Message::Assistant {
                    content: Some(String::new()),
                    tool_calls: Vec::new(),
                },
                Message::User {
                    content: "again".into(),
                },
            ],
            ..tiny_request()
        },
    ];
    for (index, request) in cases.iter().enumerate() {
        assert_eq!(
            refused(request, &full()),
            ErrorCode::RequestInvalid,
            "{index}"
        );
    }
    // Values for fields the alias does not take are never sent, so not checked.
    let minimal = ModelCapabilities::minimal();
    assert!(
        wire_body(
            &sampled(Sampling {
                temperature: Some(9.0),
                ..Sampling::default()
            }),
            &minimal
        )
        .is_ok()
    );
    assert!(
        wire_body(
            &with_schema("has space", json!({"type": "object"})),
            &minimal
        )
        .is_ok()
    );
    let edge = sampled(Sampling {
        temperature: Some(2.0),
        top_p: Some(1.0),
        seed: Some(i64::MAX as u64),
    });
    assert!(wire_body(&edge, &full()).is_ok());
}

#[tokio::test]
async fn a_locally_invalid_value_is_permanent_and_never_downgrades() {
    let stub = Stub::start(models_or(|_| {
        Reply::Json(
            400,
            json!({"error": {"type": "invalid_request_error", "param": "response_format", "code": "invalid_request"}}),
        )
    }))
    .await;
    let provider = declared(stub.url());
    let mut request = structured();
    request.output_schema.as_mut().unwrap().name = "bad name".into();
    let error = runner(provider.clone())
        .complete(&request)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RequestInvalid);
    assert!(stub.chat_requests().is_empty(), "nothing was sent");
    assert!(
        provider
            .model_capabilities("qwen3:8b")
            .await
            .structured_output,
        "the alias keeps structured output"
    );
}

#[test]
fn off_is_sent_as_none_only_where_published() {
    let mut caps = full();
    let mut request = tiny_request();
    request.reasoning = Some(Effort::Off);
    caps.reasoning_efforts = Some(vec![Effort::Off, Effort::Low]);
    assert_eq!(
        wire_body(&request, &caps).unwrap()["reasoning_effort"],
        "none"
    );
    for efforts in [None, Some(vec![Effort::Low])] {
        caps.reasoning_efforts = efforts;
        assert!(
            wire_body(&request, &caps)
                .unwrap()
                .get("reasoning_effort")
                .is_none()
        );
    }
    caps.reasoning_efforts = Some(vec![Effort::Minimal, Effort::Xhigh, Effort::Max]);
    for (effort, wire) in [
        (Effort::Minimal, "minimal"),
        (Effort::Xhigh, "xhigh"),
        (Effort::Max, "max"),
    ] {
        request.reasoning = Some(effort);
        assert_eq!(
            wire_body(&request, &caps).unwrap()["reasoning_effort"],
            wire
        );
    }
    request.reasoning = Some(Effort::High);
    assert_eq!(refused(&request, &caps), ErrorCode::UnsupportedCapability);
    let parsed: Effort = serde_json::from_value(json!("none")).unwrap();
    assert_eq!(parsed, Effort::Off);
}

#[test]
fn an_empty_tool_result_is_sent_as_a_placeholder() {
    let request = ChatRequest {
        messages: vec![
            Message::User {
                content: "u".into(),
            },
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: "call_1".into(),
                    name: "tool".into(),
                    arguments: "{}".into(),
                }],
            },
            Message::Tool {
                tool_call_id: "call_1".into(),
                content: String::new(),
            },
        ],
        tools: vec![ToolDefinition {
            name: "tool".into(),
            description: None,
            input_schema: json!({"type": "object"}),
        }],
        ..tiny_request()
    };
    let body = wire_body(&request, &full()).unwrap();
    assert_eq!(body["messages"][2]["content"], "(no output)");
}

#[tokio::test]
async fn governed_requests_carry_a_correlation_id() {
    let stub = Stub::start(models_or(|_| {
        Reply::Json(200, completion("qwen3:8b", r#"{"answer":"ok"}"#))
    }))
    .await;
    let client = client(declared(stub.url()));
    let limits = QuestionLimits::new(Duration::from_secs(30));
    let mut question = client.open_question("member", false, limits).await.unwrap();
    question.complete(&structured()).await.unwrap();
    question.complete(&structured()).await.unwrap();
    let ids: Vec<String> = stub
        .chat_requests()
        .iter()
        .map(|request| request.header("x-request-id").unwrap().to_owned())
        .collect();
    let session = question.id().to_owned();
    assert!(session.starts_with("kanade-chat-"));
    assert_eq!(ids, [format!("{session}-1"), format!("{session}-2")]);
    assert!(ids.iter().all(|id| {
        id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }));
    drop(question);
    let next = client
        .open_extraction("run", Duration::from_secs(5), Duration::from_secs(30))
        .await
        .unwrap();
    assert!(next.id().starts_with("kanade-extraction-"));
    assert_ne!(next.id(), session);

    let stub = Stub::start(models_or(|_| {
        Reply::Json(200, completion("qwen3:8b", r#"{"answer":"ok"}"#))
    }))
    .await;
    runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap();
    assert!(stub.chat_requests()[0].header("x-request-id").is_none());
}
