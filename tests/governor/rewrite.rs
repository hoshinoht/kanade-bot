//! Rewrite sessions (nudge rewrites): never wait for a permit or a rate token,
//! send exactly one request, and tell "unavailable now" from "misconfigured".

use std::time::Duration;

use kanade_llm::{
    ChatRequest, ErrorCode, FakeAction,
    governor::{CallKind, Outcome, Refused, Role, RoleConfig, SessionError, SessionFailure},
};
use tokio::time::Instant;

use crate::support::{LONG, ask, build, client, reply, roomy, single, snap};

const TIMEOUT: Duration = Duration::from_secs(2);

fn request() -> ChatRequest {
    ask("rewrite: see you at {time}", 64)
}

fn ok() -> FakeAction {
    FakeAction::Response(reply("see you at {time}!"))
}

fn refused(error: &SessionError) -> Refused {
    match &error.failure {
        SessionFailure::Refused(refused) => *refused,
        other => panic!("expected a governor refusal, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_rewrite_sends_one_request_on_its_own_permit() {
    let governor = build(&roomy(1, 10));
    let (provider, client) = client(governor.clone(), [ok()]);
    let mut rewrite = client.open_rewrite("nudge 2026-W39", TIMEOUT).unwrap();
    assert_eq!(rewrite.max_requests(), 1);
    assert!(rewrite.id().starts_with("kanade-rewrite-"));
    assert_eq!(snap(&governor).holders[0].kind, CallKind::Rewrite);
    let reply = rewrite.complete(&request()).await.unwrap();
    assert_eq!(reply.content.as_deref(), Some("see you at {time}!"));
    assert!(rewrite.is_ended());
    assert!(!rewrite.requeued());
    assert_eq!(provider.requests().len(), 1);
    drop(rewrite);
    assert_eq!(snap(&governor).permits.in_use, 0);
}

#[tokio::test(start_paused = true)]
async fn a_busy_group_is_refused_at_once_with_nothing_sent() {
    let governor = build(&roomy(1, 10));
    let (provider, client) = client(governor.clone(), [ok()]);
    let _chat = governor
        .try_acquire(Role::Chat, CallKind::Chat, "question")
        .unwrap();
    let started = Instant::now();
    let error = client.open_rewrite("nudge", TIMEOUT).unwrap_err();
    assert_eq!(refused(&error), Refused::Busy);
    assert!(!error.is_misconfiguration());
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(provider.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_open_breaker_is_unavailable_not_misconfigured() {
    let governor = build(&roomy(2, 10));
    let (provider, client) = client(governor.clone(), [ok()]);
    let chat = governor
        .try_acquire(Role::Chat, CallKind::Chat, "question")
        .unwrap();
    chat.begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::BackendUnavailable);
    let error = client.open_rewrite("nudge", TIMEOUT).unwrap_err();
    assert!(matches!(refused(&error), Refused::Unavailable { .. }));
    assert!(!error.is_misconfiguration());
    assert!(provider.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_empty_rate_bucket_is_refused_at_once_with_nothing_sent() {
    // One token per second, bucket of one.
    let mut config = single(2, 60);
    config.groups[0].burst = Some(1);
    let governor = build(&config);
    let (provider, client) = client(governor.clone(), [ok()]);
    let chat = governor
        .try_acquire(Role::Chat, CallKind::Chat, "question")
        .unwrap();
    chat.begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::Success);
    let mut rewrite = client.open_rewrite("nudge", TIMEOUT).unwrap();
    let started = Instant::now();
    let error = rewrite.complete(&request()).await.unwrap_err();
    assert!(matches!(refused(&error), Refused::RateLimited { .. }));
    assert!(!error.is_misconfiguration());
    assert_eq!(started.elapsed(), Duration::ZERO, "never waits for a token");
    assert!(provider.requests().is_empty());
    assert_eq!(rewrite.requests_used(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_failed_rewrite_is_never_retried_or_requeued() {
    for failure in [
        FakeAction::Transient,
        FakeAction::UpstreamTimeout,
        FakeAction::AdmissionRefused(Some(Duration::from_millis(100))),
    ] {
        let governor = build(&roomy(1, 10));
        let (provider, client) = client(governor.clone(), [failure, ok(), ok()]);
        let mut rewrite = client.open_rewrite("nudge", TIMEOUT).unwrap();
        let started = Instant::now();
        let error = rewrite.complete(&request()).await.unwrap_err();
        assert!(matches!(error.failure, SessionFailure::Model(_)));
        assert!(!error.is_misconfiguration());
        assert_eq!(
            started.elapsed(),
            Duration::ZERO,
            "no backoff or requeue wait"
        );
        assert_eq!(provider.requests().len(), 1);
        assert_eq!(snap(&governor).counters.retries, 0);
        assert_eq!(
            rewrite.complete(&request()).await.unwrap_err().failure,
            SessionFailure::Ended
        );
        assert_eq!(
            rewrite.answer_retry(&request()).await.unwrap_err().failure,
            SessionFailure::AnswerRetryUnavailable
        );
        assert_eq!(provider.requests().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn a_missing_or_ungrouped_rewrite_role_is_misconfigured() {
    let mut config = roomy(1, 10);
    config.roles.remove(&Role::Rewrite);
    let (provider, unrouted) = client(build(&config), [ok()]);
    let error = unrouted.open_rewrite("nudge", TIMEOUT).unwrap_err();
    assert_eq!(refused(&error), Refused::UnknownRole);
    assert!(error.is_misconfiguration());

    let mut config = roomy(1, 10);
    config.roles.insert(
        Role::Rewrite,
        RoleConfig {
            alias: "unlisted-small-model".into(),
            external: false,
        },
    );
    let (_, client) = client(build(&config), [ok()]);
    let error = client.open_rewrite("nudge", TIMEOUT).unwrap_err();
    assert_eq!(refused(&error), Refused::Ungrouped);
    assert!(error.is_misconfiguration());

    let error = client.open_rewrite("nudge", Duration::ZERO).unwrap_err();
    assert!(
        matches!(&error.failure, SessionFailure::Model(e) if e.code == ErrorCode::RequestInvalid)
    );
    assert!(error.is_misconfiguration());
    assert!(provider.requests().is_empty());
}
