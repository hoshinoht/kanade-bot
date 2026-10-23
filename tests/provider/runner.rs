use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, CompletionRunner, ErrorCode, ExecutionLimits, FakeAction,
    FakeProvider, FinishReason, Message, RetryPolicy, ToolDefinition, Usage,
};

use super::support::{
    build_runner, default_retry, request, response, run_once, tiny_request, tiny_response,
    tool_request, tool_schema,
};

/// The runner's token reservation for one attempt of `request`.
fn reservation(request: &ChatRequest) -> u32 {
    u32::try_from(serde_json::to_vec(request).unwrap().len().div_ceil(4))
        .unwrap()
        .checked_add(request.max_output_tokens)
        .unwrap()
}

fn token_budget(token_budget: u32) -> ExecutionLimits {
    ExecutionLimits {
        token_budget,
        ..ExecutionLimits::default()
    }
}

const OVER_BUDGET: (Option<ErrorCode>, usize) = (Some(ErrorCode::BudgetExceeded), 0);

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
            cached_tokens: None,
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
    let mut limited = request();
    limited.messages[0] = Message::System {
        content: "x".repeat(70_000),
    };
    let outcome = run_once(&limited, response(), ExecutionLimits::default()).await;
    assert_eq!(outcome, (Some(ErrorCode::RequestInvalid), 0));

    let mut with_tool = tiny_request();
    with_tool.tools = vec![ToolDefinition {
        name: "tool".into(),
        description: None,
        input_schema: tool_schema(),
    }];
    let outcome = run_once(&with_tool, tiny_response("m"), token_budget(10)).await;
    assert_eq!(outcome, OVER_BUDGET);
}

#[tokio::test(start_paused = true)]
async fn unknown_transient_attempts_consume_the_full_reservation() {
    let (provider, _) = build_runner([FakeAction::Transient, FakeAction::Transient]);
    let request = tiny_request();
    let limits = token_budget(reservation(&request) * 2 - 1);
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
    let larger = tool_request();
    let small_reservation = reservation(&small);
    assert!(reservation(&larger) > small_reservation);

    let limits = token_budget(small_reservation);
    let outcome = run_once(&small, tiny_response("m"), limits.clone()).await;
    assert_eq!(outcome, (None, 1));
    let outcome = run_once(&larger, tiny_response("m"), limits).await;
    assert_eq!(outcome, OVER_BUDGET);
}

#[tokio::test]
async fn request_content_bytes_change_the_reservation() {
    let mut larger = tiny_request();
    larger.messages[0] = Message::User {
        content: "x".repeat(256),
    };
    let outcome = run_once(&larger, tiny_response("m"), token_budget(10)).await;
    assert_eq!(outcome, OVER_BUDGET);
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

#[tokio::test(start_paused = true)]
async fn a_cut_off_reply_names_its_counts_but_never_the_finish_text() {
    let request = ChatRequest {
        max_output_tokens: 64,
        ..tiny_request()
    };
    let cut = |finish_reason, usage, reasoning_tokens| CompletionResponse {
        content: None,
        finish_reason,
        usage,
        reasoning_tokens,
        ..tiny_response("m")
    };
    let length = cut(
        FinishReason::Length,
        Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 64,
            cached_tokens: None,
        }),
        Some(60),
    );
    let other = cut(FinishReason::Other("finish-secret".into()), None, None);
    for (response, text) in [
        (
            length,
            "Incomplete: reply cut off at the token limit (finish=length, 64 of 64 tokens, 60 reasoning)",
        ),
        (
            other,
            "Incomplete: reply ended without finishing (finish=other, usage not reported, limit 64 tokens)",
        ),
    ] {
        let (provider, runner) = build_runner([FakeAction::Response(response)]);
        let error = runner.complete(&request).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Incomplete);
        assert_eq!(error.to_string(), text);
        assert!(!format!("{error:?}").contains("finish-secret"));
        assert_eq!(provider.requests().len(), 1, "never retried");
    }
}

#[test]
fn completion_runner_result_constructor_is_the_public_failure_boundary() {
    let provider = Arc::new(FakeProvider::new([]));
    let result =
        CompletionRunner::ungoverned(provider, ExecutionLimits::default(), RetryPolicy::default());
    assert!(result.is_ok());
}
