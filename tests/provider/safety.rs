use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionFuture, CompletionResponse, CompletionRunner, ErrorCode,
    ExecutionLimits, FakeAction, FakeProvider, FinishReason, LlmProvider, Message, OutputSchema,
    OutputValidation, ProviderFailure, ProviderFailureKind, RetryPolicy, ToolCall, ToolCallRequest,
    ToolDefinition,
};
use serde_json::json;

use super::support::{
    deep_schema, default_retry, dispose_json_value_iteratively, output_schema, request, run_once,
    tiny_request, tiny_response, wide_value,
};

#[tokio::test]
async fn deeply_nested_values_are_rejected_before_schema_compilation() {
    let mut input = request();
    input.tools.clear();
    input.output_schema = Some(output_schema("deep", deep_schema(128)));
    let output = tiny_response("synthetic-model-v1");
    let outcome = run_once(&input, output, ExecutionLimits::default()).await;
    assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0));
    dispose_json_value_iteratively(input.output_schema.take().unwrap().schema);
}

#[tokio::test]
async fn broad_values_are_rejected_before_an_unbounded_work_stack_is_built() {
    let mut input = tiny_request();
    input.output_schema = Some(output_schema("wide", wide_value(50_000)));
    let limits = ExecutionLimits {
        max_value_nodes: 4,
        ..ExecutionLimits::default()
    };
    let outcome = run_once(&input, tiny_response("m"), limits).await;
    assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0));
    dispose_json_value_iteratively(input.output_schema.take().unwrap().schema);
}

#[tokio::test]
async fn every_public_debug_representation_redacts_nested_sentinels() {
    let sentinel = "provider-debug-secret";
    let nested = json!({"nested":{"sentinel":sentinel}});
    let request = ChatRequest {
        model: sentinel.into(),
        messages: vec![
            Message::System {
                content: sentinel.into(),
            },
            Message::User {
                content: sentinel.into(),
            },
            Message::Assistant {
                content: Some(sentinel.into()),
                tool_calls: vec![ToolCallRequest {
                    id: sentinel.into(),
                    name: sentinel.into(),
                    arguments: sentinel.into(),
                }],
            },
            Message::Tool {
                tool_call_id: sentinel.into(),
                content: sentinel.into(),
            },
        ],
        tools: vec![ToolDefinition {
            name: sentinel.into(),
            description: Some(sentinel.into()),
            input_schema: nested.clone(),
        }],
        output_schema: Some(OutputSchema {
            name: sentinel.into(),
            schema: nested,
            strict: true,
            validation: OutputValidation::Runner,
        }),
        max_output_tokens: 1,
        reasoning: None,
        sampling: None,
        prompt_cache_key: None,
    };
    let response = CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: sentinel.into(),
        content: Some(sentinel.into()),
        tool_calls: vec![ToolCall {
            id: sentinel.into(),
            name: sentinel.into(),
            arguments: sentinel.into(),
        }],
        finish_reason: FinishReason::Other(sentinel.into()),
        usage: None,
    };
    let request_call = match &request.messages[2] {
        Message::Assistant { tool_calls, .. } => tool_calls[0].clone(),
        _ => unreachable!(),
    };
    let values = [
        format!("{request:?}"),
        format!("{:?}", request.messages[0]),
        format!("{:?}", request.messages[1]),
        format!("{:?}", request.messages[2]),
        format!("{:?}", request.messages[3]),
        format!("{:?}", request.tools[0]),
        format!("{:?}", request.output_schema.as_ref().unwrap()),
        format!("{request_call:?}"),
        format!("{response:?}"),
        format!("{:?}", response.tool_calls[0]),
        format!("{:?}", response.finish_reason),
        format!(
            "{:?}",
            ProviderFailure {
                kind: ProviderFailureKind::Permanent,
                reason_code: "provider-debug-secret",
            }
        ),
        format!("{:?}", FakeAction::Response(response.clone())),
        format!(
            "{:?}",
            FakeAction::Delayed {
                delay: Duration::from_millis(1),
                action: Box::new(FakeAction::Response(response.clone())),
            }
        ),
    ];
    for value in values {
        assert!(!value.contains(sentinel), "{value}");
    }
}

struct RawReasonProvider;

impl LlmProvider for RawReasonProvider {
    fn complete(&self, _: &ChatRequest) -> CompletionFuture<'_> {
        Box::pin(async {
            Err(ProviderFailure {
                kind: ProviderFailureKind::Permanent,
                reason_code: "provider-debug-secret",
            })
        })
    }
}

#[tokio::test]
async fn provider_raw_reason_is_redacted_by_the_real_runner() {
    let runner = CompletionRunner::ungoverned(
        Arc::new(RawReasonProvider),
        ExecutionLimits::default(),
        default_retry(),
    )
    .unwrap();
    let error = runner.complete(&tiny_request()).await.unwrap_err();
    assert!(!format!("{error:?}").contains("provider-debug-secret"));
    assert!(!format!("{error}").contains("provider-debug-secret"));
}

#[test]
fn zero_and_extreme_limits_and_retry_policies_return_typed_errors() {
    let mut invalid_limits = Vec::new();
    let updates: [fn(&mut ExecutionLimits); 20] = [
        |limits: &mut ExecutionLimits| limits.max_messages = 0,
        |limits: &mut ExecutionLimits| limits.max_content_bytes = 0,
        |limits: &mut ExecutionLimits| limits.max_tools = 0,
        |limits: &mut ExecutionLimits| limits.max_schema_bytes = 0,
        |limits: &mut ExecutionLimits| limits.max_depth = 0,
        |limits: &mut ExecutionLimits| limits.max_output_bytes = 0,
        |limits: &mut ExecutionLimits| limits.max_request_bytes = 0,
        |limits: &mut ExecutionLimits| limits.max_response_bytes = 0,
        |limits: &mut ExecutionLimits| limits.max_value_nodes = 0,
        |limits: &mut ExecutionLimits| limits.token_budget = 0,
        |limits: &mut ExecutionLimits| limits.max_messages = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_content_bytes = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_tools = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_schema_bytes = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_depth = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_output_bytes = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_request_bytes = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_response_bytes = usize::MAX,
        |limits: &mut ExecutionLimits| limits.max_value_nodes = usize::MAX,
        |limits: &mut ExecutionLimits| limits.token_budget = u32::MAX,
    ];
    for update in updates {
        let mut limits = ExecutionLimits::default();
        update(&mut limits);
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            CompletionRunner::ungoverned(
                Arc::new(FakeProvider::new([])),
                limits.clone(),
                RetryPolicy::default(),
            )
        }));
        let result = outcome.expect("invalid limits must not panic");
        match result {
            Ok(_) => panic!("invalid limits were accepted"),
            Err(error) => assert_eq!(error.code, ErrorCode::RequestInvalid),
        }
        invalid_limits.push(limits);
    }
    assert_eq!(invalid_limits.len(), 20);

    for policy in [
        RetryPolicy {
            total_deadline: Duration::ZERO,
            ..RetryPolicy::default()
        },
        RetryPolicy {
            total_deadline: Duration::MAX,
            ..RetryPolicy::default()
        },
        RetryPolicy {
            max_attempts: 0,
            ..RetryPolicy::default()
        },
        RetryPolicy {
            max_attempts: u8::MAX,
            ..RetryPolicy::default()
        },
        RetryPolicy {
            backoff: Duration::ZERO,
            ..RetryPolicy::default()
        },
        RetryPolicy {
            backoff: Duration::MAX,
            ..RetryPolicy::default()
        },
    ] {
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            CompletionRunner::ungoverned(
                Arc::new(FakeProvider::new([])),
                ExecutionLimits::default(),
                policy,
            )
        }));
        match outcome.expect("invalid retry policy must not panic") {
            Ok(_) => panic!("invalid retry policy was accepted"),
            Err(error) => assert_eq!(error.code, ErrorCode::RequestInvalid),
        }
    }
}
