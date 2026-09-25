use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    CompletionResponse, CompletionRunner, ErrorCode, ExecutionLimits, FakeAction, FakeProvider,
    FinishReason, Message, RetryPolicy, ToolDefinition, Usage,
};

use super::support::{
    build_runner, build_runner_with, default_retry, request, response, tiny_request, tiny_response,
    tool_schema,
};

#[tokio::test]
async fn accepts_openai_style_structured_tool_response() {
    let (provider, runner) = build_runner([FakeAction::Response(response())]);
    let completed = runner.complete(&request()).await.unwrap();
    assert_eq!(completed.finish_reason, FinishReason::ToolCalls);
    assert_eq!(provider.requests(), vec![request()]);
}

#[tokio::test(start_paused = true)]
async fn retries_only_transient_failures_with_a_total_deadline() {
    let (provider, runner) = build_runner([
        FakeAction::Transient,
        FakeAction::Transient,
        FakeAction::Response(response()),
    ]);
    assert!(runner.complete(&request()).await.is_ok());
    assert_eq!(provider.requests().len(), 3);

    let (provider, runner) = build_runner([FakeAction::Permanent]);
    assert_eq!(
        runner.complete(&request()).await.unwrap_err().code,
        ErrorCode::ProviderPermanent
    );
    assert_eq!(provider.requests().len(), 1);

    let (provider, _) = build_runner([FakeAction::Transient, FakeAction::Transient]);
    let runner = CompletionRunner::ungoverned(
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy {
            total_deadline: Duration::from_millis(5),
            max_attempts: 3,
            backoff: Duration::from_millis(10),
        },
    )
    .unwrap();
    assert_eq!(
        runner.complete(&request()).await.unwrap_err().code,
        ErrorCode::DeadlineExceeded
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn timeout_and_cancellation_do_not_leave_background_work() {
    let (provider, runner) = build_runner([FakeAction::Delayed {
        delay: Duration::from_secs(3),
        action: Box::new(FakeAction::Response(response())),
    }]);
    let task = tokio::spawn(async move { runner.complete(&request()).await });
    tokio::task::yield_now().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn missing_usage_is_accepted_but_model_and_usage_overflow_fail_closed() {
    let missing_usage = CompletionResponse {
        usage: None,
        ..response()
    };
    let (_, runner) = build_runner([FakeAction::Response(missing_usage)]);
    assert!(runner.complete(&request()).await.is_ok());

    let wrong_model = CompletionResponse {
        model: "other".into(),
        ..response()
    };
    let (_, runner) = build_runner([FakeAction::Response(wrong_model)]);
    assert_eq!(
        runner.complete(&request()).await.unwrap_err().code,
        ErrorCode::ModelMismatch
    );

    let overflow = CompletionResponse {
        usage: Some(Usage {
            prompt_tokens: u32::MAX,
            completion_tokens: 1,
        }),
        ..response()
    };
    let (_, runner) = build_runner([FakeAction::Response(overflow)]);
    assert_eq!(
        runner.complete(&request()).await.unwrap_err().code,
        ErrorCode::BudgetExceeded
    );
}

#[tokio::test]
async fn request_content_limit_is_rejected_before_provider_call() {
    let (provider, runner) = build_runner([FakeAction::Response(response())]);
    let mut limited = request();
    limited.messages[0] = Message::System {
        content: "x".repeat(70_000),
    };
    assert_eq!(
        runner.complete(&limited).await.unwrap_err().code,
        ErrorCode::RequestInvalid
    );
    assert!(provider.requests().is_empty());

    let mut with_tool = tiny_request();
    with_tool.tools = vec![ToolDefinition {
        name: "tool".into(),
        description: None,
        input_schema: tool_schema(),
    }];
    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits {
            token_budget: 10,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&with_tool).await.unwrap_err().code,
        ErrorCode::BudgetExceeded
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn unknown_transient_attempts_consume_the_full_reservation() {
    let (provider, _) = build_runner([FakeAction::Transient, FakeAction::Transient]);
    let request = tiny_request();
    let reservation = u32::try_from(serde_json::to_vec(&request).unwrap().len().div_ceil(4))
        .unwrap()
        .checked_add(request.max_output_tokens)
        .unwrap();
    let limits = ExecutionLimits {
        token_budget: reservation * 2 - 1,
        ..ExecutionLimits::default()
    };
    let runner = CompletionRunner::ungoverned(provider.clone(), limits, default_retry()).unwrap();
    assert_eq!(
        runner.complete(&request).await.unwrap_err().code,
        ErrorCode::BudgetExceeded
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn request_schema_and_tool_bytes_change_the_reservation() {
    let small = tiny_request();
    let larger = super::support::tool_request();
    let small_reservation = u32::try_from(serde_json::to_vec(&small).unwrap().len().div_ceil(4))
        .unwrap()
        .checked_add(small.max_output_tokens)
        .unwrap();
    let large_reservation = u32::try_from(serde_json::to_vec(&larger).unwrap().len().div_ceil(4))
        .unwrap()
        .checked_add(larger.max_output_tokens)
        .unwrap();
    assert!(large_reservation > small_reservation);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits {
            token_budget: small_reservation,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert!(runner.complete(&small).await.is_ok());
    assert_eq!(provider.requests().len(), 1);

    let (provider, runner) = build_runner_with(
        [FakeAction::Response(tiny_response("m"))],
        ExecutionLimits {
            token_budget: small_reservation,
            ..ExecutionLimits::default()
        },
        default_retry(),
    );
    assert_eq!(
        runner.complete(&larger).await.unwrap_err().code,
        ErrorCode::BudgetExceeded
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test]
async fn request_content_bytes_change_the_reservation() {
    let mut larger = tiny_request();
    larger.messages[0] = Message::User {
        content: "x".repeat(256),
    };
    let limits = ExecutionLimits {
        token_budget: 10,
        ..ExecutionLimits::default()
    };
    let (provider, _) = build_runner([FakeAction::Response(tiny_response("m"))]);
    let runner = CompletionRunner::ungoverned(provider.clone(), limits, default_retry()).unwrap();
    assert_eq!(
        runner.complete(&larger).await.unwrap_err().code,
        ErrorCode::BudgetExceeded
    );
    assert!(provider.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_content_filter_is_told_apart_from_a_length_cut_off_and_never_retried() {
    for (finish_reason, code) in [
        (FinishReason::ContentFilter, ErrorCode::ContentFiltered),
        (FinishReason::Length, ErrorCode::Incomplete),
        (FinishReason::Other("weird".into()), ErrorCode::Incomplete),
    ] {
        let cut = CompletionResponse {
            content: None,
            tool_calls: Vec::new(),
            finish_reason: finish_reason.clone(),
            ..tiny_response("m")
        };
        let (provider, runner) = build_runner([
            FakeAction::Response(cut),
            FakeAction::Response(tiny_response("m")),
        ]);
        assert_eq!(
            runner.complete(&tiny_request()).await.unwrap_err().code,
            code,
            "{finish_reason:?}"
        );
        assert_eq!(provider.requests().len(), 1, "{finish_reason:?}");
    }
}

#[test]
fn completion_runner_result_constructor_is_the_public_failure_boundary() {
    let provider = Arc::new(FakeProvider::new([]));
    let result =
        CompletionRunner::ungoverned(provider, ExecutionLimits::default(), RetryPolicy::default());
    assert!(result.is_ok());
}
