//! A request that waited for a rate token but was never sent leaves no trace:
//! the token is refunded and no request, retry or budget entry is recorded.

use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::governor::{
    BreakerState, CallKind, Governor, Outcome, Permit, Refused, Role,
};

use crate::support::{LONG, build, settle, single, snap};

/// Two permits, one request per second, no burst.
fn slow() -> Arc<Governor> {
    let mut config = single(2, 60);
    config.groups[0].burst = Some(1);
    build(&config)
}

fn hold(governor: &Governor) -> Permit {
    governor
        .try_acquire(Role::Chat, CallKind::Chat, "holder")
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_breaker_that_opens_during_the_wait_refunds_the_token() {
    let governor = slow();
    let a = hold(&governor);
    let b = Arc::new(hold(&governor));
    let first = a.begin_request(LONG).await.unwrap();
    let waiting = {
        let b = b.clone();
        tokio::spawn(async move { b.begin_request(LONG).await.map(drop) })
    };
    settle().await;
    first.finish(Outcome::BackendUnavailable);
    assert_eq!(snap(&governor).breaker.state, BreakerState::Open);
    assert!(matches!(
        waiting.await.unwrap().unwrap_err(),
        Refused::Unavailable { retry_at: Some(_) }
    ));
    let snapshot = snap(&governor);
    assert_eq!(snapshot.counters.requests, 1);
    assert_eq!(snapshot.counters.shed_unavailable, 1);
    // One second refilled the reserved token; the refund makes it available again.
    assert_eq!(snapshot.rate.available, 1);
    assert_eq!(snapshot.retry.capacity, 1);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_wait_refunds_the_token_and_the_retry_budget() {
    let governor = slow();
    let permit = hold(&governor);
    permit
        .begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::TransientFailure);
    let cancelled =
        tokio::time::timeout(Duration::from_millis(500), permit.begin_retry(LONG)).await;
    assert!(
        cancelled.is_err(),
        "the retry was still waiting for its token"
    );
    let snapshot = snap(&governor);
    assert_eq!(snapshot.counters.retries, 0);
    assert_eq!((snapshot.retry.remaining, snapshot.retry.capacity), (1, 1));
    tokio::time::advance(Duration::from_millis(500)).await;
    // Without the refund the bucket would be empty at t = 1 s.
    assert_eq!(snap(&governor).rate.available, 1);
    permit
        .begin_retry(LONG)
        .await
        .expect("budget and token intact")
        .finish(Outcome::Success);
    assert_eq!(snap(&governor).counters.retries, 1);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_probe_wait_frees_the_probe() {
    let governor = slow();
    let permit = hold(&governor);
    permit
        .begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::BackendUnavailable);
    tokio::time::advance(Duration::from_millis(37_500)).await;
    // Drain the refilled bucket so the probe has to wait for its token.
    assert!(permit.try_begin_request().unwrap().is_probe());
    let cancelled =
        tokio::time::timeout(Duration::from_millis(500), permit.begin_request(LONG)).await;
    assert!(cancelled.is_err());
    assert_eq!(snap(&governor).breaker.state, BreakerState::HalfOpen);
    tokio::time::advance(Duration::from_millis(500)).await;
    assert!(permit.try_begin_request().unwrap().is_probe());
}
