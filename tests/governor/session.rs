//! Governed sessions: one permit per question, a hard request cap (tool rounds
//! plus one reserved clean retry), one requeue on slot loss, charge/refund
//! classification and the governor's retry gate in front of every retry.

use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, ErrorCode, ExecutionLimits, FakeAction, FakeProvider,
    FinishReason, Message, RetryPolicy, Usage,
    governor::{
        BreakerState, CallKind, Charge, DEFAULT_TOOL_ROUNDS, Governor, GovernorConfig, ModelClient,
        QuestionLimits, Refused, Role, SessionError, SessionFailure,
    },
};
use tokio::time::Instant;

use crate::support::{ALIAS, build, settle, single, snap};

const TIMEOUT: Duration = Duration::from_secs(60);

fn request() -> ChatRequest {
    ChatRequest {
        model: ALIAS.into(),
        messages: vec![Message::User {
            content: "when is the next run?".into(),
        }],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 16,
        reasoning: None,
        sampling: None,
    }
}

fn ok() -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: ALIAS.into(),
        content: Some("soon".into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
        }),
    })
}

/// One permit, no rate waits; `retry_floor` retries allowed per window.
fn config(retry_floor: u32) -> GovernorConfig {
    let mut config = single(1, 6_000);
    config.groups[0].burst = Some(1_000);
    config.policy.retry_floor = retry_floor;
    config
}

fn setup(
    governor: Arc<Governor>,
    actions: impl IntoIterator<Item = FakeAction>,
) -> (Arc<FakeProvider>, Arc<ModelClient<FakeProvider>>) {
    let provider = Arc::new(FakeProvider::new(actions));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(30),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor,
        provider.clone(),
        ExecutionLimits::default(),
        retry,
    )
    .expect("valid client");
    (provider, Arc::new(client))
}

fn rounds(tool_rounds: u8) -> QuestionLimits {
    QuestionLimits {
        tool_rounds,
        timeout: TIMEOUT,
    }
}

fn model(error: &SessionError) -> ErrorCode {
    match &error.failure {
        SessionFailure::Model(error) => error.code,
        other => panic!("expected a model failure, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_question_holds_one_permit_and_reserves_its_clean_retry() {
    assert_eq!(
        QuestionLimits::new(TIMEOUT).tool_rounds,
        DEFAULT_TOOL_ROUNDS
    );
    let governor = build(&config(1));
    let (provider, client) = setup(governor.clone(), [ok(), ok(), ok(), ok()]);
    let mut question = client
        .open_question("member", false, rounds(2))
        .await
        .unwrap();
    assert_eq!(question.max_requests(), 3);
    for _ in 0..2 {
        question.complete(&request()).await.unwrap();
        assert_eq!(snap(&governor).permits.in_use, 1);
        assert_eq!(
            governor
                .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
                .unwrap_err(),
            Refused::Busy
        );
    }
    let exhausted = question.complete(&request()).await.unwrap_err();
    assert_eq!(exhausted.failure, SessionFailure::RequestsExhausted);
    assert_eq!(exhausted.charge, Charge::Refunded);

    question.clean_retry(&request()).await.unwrap();
    assert_eq!(
        question.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::CleanRetryUnavailable
    );
    assert_eq!(question.requests_used(), 3);
    assert_eq!(provider.requests().len(), 3);
    let counters = snap(&governor).counters;
    assert_eq!((counters.requests, counters.retries), (2, 1));
    drop(question);
    assert_eq!(snap(&governor).permits.in_use, 0);
}

#[tokio::test(start_paused = true)]
async fn no_failure_mix_exceeds_the_question_request_cap() {
    let mixes: [Vec<FakeAction>; 4] = [
        vec![FakeAction::Transient; 20],
        vec![
            FakeAction::AdmissionRefused(Some(Duration::from_secs(1))),
            FakeAction::Transient,
            FakeAction::Transient,
            FakeAction::Transient,
        ],
        vec![
            ok(),
            FakeAction::Transient,
            FakeAction::AdmissionRefused(None),
            FakeAction::Transient,
            FakeAction::Transient,
        ],
        vec![FakeAction::Malformed; 20],
    ];
    for actions in mixes {
        let governor = build(&config(10));
        let (provider, client) = setup(governor, actions);
        let mut question = client
            .open_question("member", false, rounds(2))
            .await
            .unwrap();
        for _ in 0..4 {
            let _ = question.complete(&request()).await;
        }
        let _ = question.clean_retry(&request()).await;
        let _ = question.complete(&request()).await;
        assert!(question.requests_used() <= 3);
        assert_eq!(provider.requests().len() as u32, question.requests_used());
    }
}

#[tokio::test(start_paused = true)]
async fn an_upstream_timeout_is_charged_and_ends_the_question() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor.clone(), [ok(), FakeAction::UpstreamTimeout, ok()]);
    let mut question = client
        .open_question("member", false, rounds(8))
        .await
        .unwrap();
    question.complete(&request()).await.unwrap();
    let error = question.complete(&request()).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::UpstreamTimeout);
    assert_eq!(error.charge, Charge::Charged);
    assert!(question.is_ended());
    assert_eq!(
        question.complete(&request()).await.unwrap_err().failure,
        SessionFailure::Ended
    );
    assert_eq!(
        question.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::Ended
    );
    assert_eq!(
        provider.requests().len(),
        2,
        "no retry after a chat timeout"
    );
    let counters = snap(&governor).counters;
    assert_eq!((counters.timeouts, counters.retries), (1, 0));
}

#[tokio::test(start_paused = true)]
async fn an_admission_refusal_requeues_once_after_retry_after_and_jitter() {
    let governor = build(&config(10));
    let (provider, client) = setup(
        governor.clone(),
        [
            FakeAction::AdmissionRefused(Some(Duration::from_secs(2))),
            ok(),
        ],
    );
    let started = Instant::now();
    let task = {
        let client = client.clone();
        tokio::spawn(async move {
            let mut question = client.open_question("member", false, rounds(8)).await?;
            let reply = question.complete(&request()).await;
            Ok::<_, SessionError>((reply, question.requests_used(), question.requeued()))
        })
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(provider.requests().len(), 1, "never retried at once");
    assert_eq!(
        snap(&governor).permits.in_use,
        0,
        "the permit is released while waiting"
    );
    let (reply, used, requeued) = task.await.unwrap().unwrap();
    reply.unwrap();
    // Retry-After 2 s plus a half draw of full jitter over 2 s.
    assert_eq!(started.elapsed(), Duration::from_secs(3));
    assert_eq!((used, requeued), (2, true));
    let counters = snap(&governor).counters;
    assert_eq!(
        (
            counters.admission_refused,
            counters.requests,
            counters.retries
        ),
        (1, 1, 1)
    );
    assert_eq!(snap(&governor).breaker.state, BreakerState::Closed);
}

#[tokio::test(start_paused = true)]
async fn a_second_slot_loss_or_no_time_left_is_refunded_not_retried() {
    let governor = build(&config(10));
    let refusal = || FakeAction::AdmissionRefused(Some(Duration::from_secs(1)));
    let (provider, client) = setup(governor, [refusal(), refusal(), refusal()]);
    let mut question = client
        .open_question("member", false, rounds(8))
        .await
        .unwrap();
    let error = question.complete(&request()).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::AdmissionRefused);
    assert_eq!(error.charge, Charge::Refunded);
    assert_eq!(provider.requests().len(), 2);
    assert!(question.requeued());

    let governor = build(&config(10));
    let (provider, client) = setup(
        governor,
        [FakeAction::AdmissionRefused(Some(Duration::from_secs(5)))],
    );
    let limits = QuestionLimits {
        tool_rounds: 8,
        timeout: Duration::from_secs(4),
    };
    let mut question = client.open_question("member", false, limits).await.unwrap();
    let error = question.complete(&request()).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::AdmissionRefused);
    assert_eq!(provider.requests().len(), 1);
    assert!(!question.requeued());
}

#[tokio::test(start_paused = true)]
async fn transient_retries_use_full_jitter_and_stop_when_the_budget_is_gone() {
    let governor = build(&config(1));
    let (provider, client) = setup(
        governor.clone(),
        [FakeAction::Transient, ok(), FakeAction::Transient, ok()],
    );
    let mut question = client
        .open_question("first", false, rounds(8))
        .await
        .unwrap();
    let started = Instant::now();
    question.complete(&request()).await.unwrap();
    // Half of the 100 ms base, drawn from the injected random.
    assert_eq!(started.elapsed(), Duration::from_millis(50));
    let error = question.complete(&request()).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::ProviderPermanent);
    assert_eq!(error.charge, Charge::Refunded);
    assert_eq!(provider.requests().len(), 3);
    let counters = snap(&governor).counters;
    assert_eq!((counters.retries, counters.retries_denied), (1, 1));
    assert_eq!(
        question.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::Refused(Refused::RetryBudgetExhausted)
    );
}

#[tokio::test(start_paused = true)]
async fn a_down_backend_is_refunded_and_blocks_the_clean_retry() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor.clone(), [FakeAction::BackendUnavailable, ok()]);
    let mut question = client
        .open_question("member", false, rounds(8))
        .await
        .unwrap();
    let error = question.complete(&request()).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::BackendUnavailable);
    assert_eq!(error.charge, Charge::Refunded);
    assert_eq!(snap(&governor).breaker.state, BreakerState::Open);
    assert!(matches!(
        question.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::Refused(Refused::Unavailable { .. })
    ));
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn extraction_is_one_call_at_extraction_priority_and_may_retry_a_timeout() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor.clone(), [FakeAction::UpstreamTimeout, ok()]);
    let mut session = client
        .open_extraction("run 2026-W39", TIMEOUT, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(snap(&governor).holders[0].kind, CallKind::Extraction);
    assert_eq!(session.max_requests(), 4);
    session.complete(&request()).await.unwrap();
    assert_eq!(provider.requests().len(), 2);
    assert_eq!(snap(&governor).counters.retries, 1);
    assert_eq!(
        session.complete(&request()).await.unwrap_err().failure,
        SessionFailure::Ended
    );
    assert_eq!(
        session.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::CleanRetryUnavailable
    );
}

#[tokio::test(start_paused = true)]
async fn sessions_validate_limits_and_their_alias() {
    let governor = build(&config(1));
    let (provider, client) = setup(governor, [ok()]);
    for tool_rounds in [0, 13] {
        let error = client
            .open_question("member", false, rounds(tool_rounds))
            .await
            .unwrap_err();
        assert_eq!(model(&error), ErrorCode::RequestInvalid);
    }
    let mut question = client
        .open_question("member", true, rounds(12))
        .await
        .unwrap();
    assert_eq!(question.max_requests(), 13);
    let mut foreign = request();
    foreign.model = "other-model".into();
    let error = question.complete(&foreign).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::RequestInvalid);
    assert!(provider.requests().is_empty());
}

fn slow() -> FakeAction {
    FakeAction::Delayed {
        delay: Duration::from_secs(10),
        action: Box::new(ok()),
    }
}

const CUT: Duration = Duration::from_secs(1);

#[tokio::test(start_paused = true)]
async fn a_round_cancelled_mid_request_still_counts_toward_the_cap() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor.clone(), [slow(), ok(), ok(), ok()]);
    let mut question = client
        .open_question("member", false, rounds(2))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(CUT, question.complete(&request()))
            .await
            .is_err()
    );
    assert_eq!(question.requests_used(), 1);
    question.complete(&request()).await.unwrap();
    assert_eq!(
        question.complete(&request()).await.unwrap_err().failure,
        SessionFailure::RequestsExhausted
    );
    question.clean_retry(&request()).await.unwrap();
    assert_eq!(question.requests_used(), 3);
    assert_eq!(provider.requests().len(), 3);
    // The abandoned request is breaker-neutral.
    assert_eq!(snap(&governor).breaker.failures, 0);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_extraction_call_ends_the_session() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor, [slow(), ok()]);
    let mut session = client
        .open_extraction("run 2026-W39", TIMEOUT, TIMEOUT)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(CUT, session.complete(&request()))
            .await
            .is_err()
    );
    assert!(session.is_ended());
    assert_eq!(
        session.complete(&request()).await.unwrap_err().failure,
        SessionFailure::Ended
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_clean_retry_spends_the_reserved_request() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor, [ok(), slow(), ok(), ok()]);
    let mut question = client
        .open_question("member", false, rounds(1))
        .await
        .unwrap();
    question.complete(&request()).await.unwrap();
    assert!(
        tokio::time::timeout(CUT, question.clean_retry(&request()))
            .await
            .is_err()
    );
    assert_eq!(question.requests_used(), 2);
    assert_eq!(
        question.complete(&request()).await.unwrap_err().failure,
        SessionFailure::RequestsExhausted
    );
    assert_eq!(
        question.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::CleanRetryUnavailable
    );
    assert_eq!(provider.requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_requeue_that_could_not_be_retried_fails_fast_and_keeps_the_permit() {
    let governor = build(&config(0));
    let (provider, client) = setup(
        governor.clone(),
        [
            FakeAction::AdmissionRefused(Some(Duration::from_secs(2))),
            ok(),
        ],
    );
    let mut question = client
        .open_question("member", false, rounds(8))
        .await
        .unwrap();
    let started = Instant::now();
    let error = question.complete(&request()).await.unwrap_err();
    assert_eq!(model(&error), ErrorCode::AdmissionRefused);
    assert_eq!(error.charge, Charge::Refunded);
    assert_eq!(started.elapsed(), Duration::ZERO, "no pointless wait");
    assert!(!question.requeued());
    assert!(!question.is_ended());
    let snapshot = snap(&governor);
    assert_eq!(snapshot.permits.in_use, 1);
    assert_eq!(snapshot.counters.retries_denied, 1);
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_zero_retry_after_still_waits_a_floor_before_requeueing() {
    let governor = build(&config(10));
    let (provider, client) = setup(
        governor,
        [FakeAction::AdmissionRefused(Some(Duration::ZERO)), ok()],
    );
    let mut question = client
        .open_question("member", false, rounds(8))
        .await
        .unwrap();
    let started = Instant::now();
    question.complete(&request()).await.unwrap();
    // 250 ms floor plus a half draw of jitter over it.
    assert_eq!(started.elapsed(), Duration::from_millis(375));
    assert_eq!(provider.requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_clean_retry_skipped_while_half_open_sheds_nothing() {
    let governor = build(&config(10));
    let (provider, client) = setup(governor.clone(), [FakeAction::BackendUnavailable, ok()]);
    let limits = QuestionLimits {
        tool_rounds: 8,
        timeout: Duration::from_secs(300),
    };
    let mut question = client.open_question("member", false, limits).await.unwrap();
    question.complete(&request()).await.unwrap_err();
    tokio::time::sleep(Duration::from_secs(60)).await;
    let before = snap(&governor);
    assert_eq!(before.breaker.state, BreakerState::HalfOpen);
    assert!(matches!(
        question.clean_retry(&request()).await.unwrap_err().failure,
        SessionFailure::Refused(Refused::Unavailable { .. })
    ));
    assert_eq!(
        snap(&governor).counters.shed_unavailable,
        before.counters.shed_unavailable
    );
    assert_eq!(provider.requests().len(), 1);
}
