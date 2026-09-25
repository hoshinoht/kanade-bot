use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionRunner, ErrorCode, ExecutionLimits, HttpProviderConfig,
    ModelCapabilities, OpenAiCompatibleProvider, RetryPolicy, shape_request,
};
use serde_json::json;

use super::{
    http_transport::{build, full_capabilities, models_or, runner as default_runner, structured},
    stub::{Recorded, Reply, Stub, completion},
};

const ANSWER: &str = r#"{"answer":"ok"}"#;

fn listings(stub: &Stub) -> usize {
    stub.requests()
        .iter()
        .filter(|request| request.method == "GET")
        .count()
}

fn config(url: String) -> HttpProviderConfig {
    let mut config = HttpProviderConfig::new(url);
    config.declared = full_capabilities();
    config
}

fn answer(request: &Recorded) -> Reply {
    if request.method == "GET" {
        Reply::Hang
    } else {
        Reply::Json(200, completion("qwen3:8b", ANSWER))
    }
}

fn runner_within(
    provider: Arc<OpenAiCompatibleProvider>,
    deadline: Duration,
) -> CompletionRunner<OpenAiCompatibleProvider> {
    let retry = RetryPolicy {
        total_deadline: deadline,
        max_attempts: 1,
        backoff: Duration::from_millis(10),
    };
    CompletionRunner::ungoverned(provider, ExecutionLimits::default(), retry).unwrap()
}

#[tokio::test]
async fn stalled_listing_falls_back_to_declared_capabilities_and_is_negatively_cached() {
    let stub = Stub::start(answer).await;
    let mut config = config(stub.url());
    config.catalog_timeout = Duration::from_millis(200);
    let runner = runner_within(build(config), Duration::from_secs(2));
    let started = Instant::now();
    runner.complete(&structured()).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        stub.chat_requests()[0]
            .body
            .get("response_format")
            .is_some()
    );
    assert_eq!(listings(&stub), 1);

    let started = Instant::now();
    runner.complete(&structured()).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "no second wait"
    );
    assert_eq!(listings(&stub), 1, "failure is cached");
}

#[tokio::test]
async fn listing_fetch_is_bounded_by_the_remaining_deadline() {
    let stub = Stub::start(answer).await;
    let runner = runner_within(build(config(stub.url())), Duration::from_millis(600));
    runner.complete(&structured()).await.unwrap();
    assert_eq!(stub.chat_requests().len(), 1);
}

#[tokio::test]
async fn a_fetch_cut_short_by_the_callers_deadline_is_not_negatively_cached() {
    let stub = Stub::start(|request: &Recorded| {
        if request.method == "GET" {
            Reply::Slow(
                Duration::from_millis(400),
                200,
                json!({"data": [{"id": "qwen3:8b", "kanata": {"structured_output": true}}]}),
            )
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    })
    .await;
    let provider = build(HttpProviderConfig::new(stub.url()));
    runner_within(provider.clone(), Duration::from_millis(600))
        .complete(&structured())
        .await
        .unwrap();
    assert!(
        stub.chat_requests()[0]
            .body
            .get("response_format")
            .is_none(),
        "short budget fell back to minimal"
    );
    assert_eq!(listings(&stub), 1);

    assert!(
        provider
            .model_capabilities("qwen3:8b")
            .await
            .structured_output
    );
    assert_eq!(listings(&stub), 2, "full-budget caller fetched again");
}

#[tokio::test]
async fn one_listing_fetch_per_completion_and_none_within_the_ttl() {
    let stub = Stub::start(models_or(|_| {
        Reply::Json(200, completion("qwen3:8b", ANSWER))
    }))
    .await;
    let runner = default_runner(build(config(stub.url())));
    runner.complete(&structured()).await.unwrap();
    assert_eq!(listings(&stub), 1);
    runner.complete(&structured()).await.unwrap();
    assert_eq!(listings(&stub), 1);

    let stub = Stub::start(models_or(|_| {
        Reply::Json(200, completion("qwen3:8b", ANSWER))
    }))
    .await;
    let mut expiring = config(stub.url());
    expiring.catalog_ttl = Duration::ZERO;
    let runner = default_runner(build(expiring));
    runner.complete(&structured()).await.unwrap();
    runner.complete(&structured()).await.unwrap();
    assert_eq!(listings(&stub), 2, "exactly one per completion");

    let stub = Stub::start(|request: &Recorded| {
        if request.method == "GET" {
            Reply::Json(500, json!({}))
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    })
    .await;
    let runner = default_runner(build(config(stub.url())));
    runner.complete(&structured()).await.unwrap();
    runner.complete(&structured()).await.unwrap();
    assert_eq!(listings(&stub), 1, "failed listing is not refetched");
}

#[tokio::test]
async fn concurrent_lookups_share_one_listing_fetch() {
    let stub = Stub::start(models_or(|_| {
        Reply::Json(200, completion("qwen3:8b", ANSWER))
    }))
    .await;
    let runner = Arc::new(default_runner(build(config(stub.url()))));
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let runner = runner.clone();
            tokio::spawn(async move { runner.complete(&structured()).await })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(listings(&stub), 1);
    assert_eq!(stub.chat_requests().len(), 8);
}

#[tokio::test]
async fn cache_expiry_cannot_change_the_body_after_shaping() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let seen = fetches.clone();
    let stub = Stub::start(move |request: &Recorded| {
        if request.method == "GET" {
            let structured = seen.fetch_add(1, Ordering::SeqCst) == 0;
            Reply::Json(
                200,
                json!({"data": [{"id": "qwen3:8b", "kanata": {
                    "structured_output": structured,
                    "sampling_controls": true,
                    "function_tools": true
                }}]}),
            )
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    })
    .await;
    let mut config = HttpProviderConfig::new(stub.url());
    config.catalog_ttl = Duration::ZERO;
    let runner = default_runner(build(config));
    runner.complete(&structured()).await.unwrap();
    assert_eq!(listings(&stub), 1);
    let body = &stub.chat_requests()[0].body;
    assert!(body.get("response_format").is_some());
    assert_eq!(body["messages"][0]["role"], "user", "no schema instruction");
}

#[tokio::test]
async fn downgrade_retry_is_recounted_and_independent_of_transient_attempts() {
    let rejecting = |request: &Recorded| {
        if request.method == "GET" {
            Reply::Json(200, json!({"data": []}))
        } else if request.body.get("response_format").is_some() {
            Reply::Json(400, json!({"error": {"param": "response_format"}}))
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    };
    let stub = Stub::start(rejecting).await;
    runner_within(build(config(stub.url())), Duration::from_secs(2))
        .complete(&structured())
        .await
        .unwrap();
    assert_eq!(
        stub.chat_requests().len(),
        2,
        "max_attempts=1 still retries once"
    );

    let request = structured();
    let reservation = |request: &ChatRequest| {
        u32::try_from(serde_json::to_vec(request).unwrap().len().div_ceil(4)).unwrap()
            + request.max_output_tokens
    };
    let reduced = ModelCapabilities {
        structured_output: false,
        ..full_capabilities()["qwen3:8b"].clone()
    };
    let shaped = shape_request(&request, &reduced).unwrap().unwrap();
    let both = reservation(&request) + reservation(&shaped);
    for (budget, ok) in [(both, true), (both - 1, false)] {
        let stub = Stub::start(rejecting).await;
        let runner = CompletionRunner::ungoverned(
            build(config(stub.url())),
            ExecutionLimits {
                token_budget: budget,
                ..ExecutionLimits::default()
            },
            super::support::default_retry(),
        )
        .unwrap();
        let result = runner.complete(&request).await;
        if ok {
            assert!(result.is_ok());
            assert_eq!(stub.chat_requests().len(), 2);
        } else {
            assert_eq!(result.unwrap_err().code, ErrorCode::BudgetExceeded);
            assert_eq!(stub.chat_requests().len(), 1, "reshaped retry not sent");
        }
    }
}
